//! HTTP composition over shared application services and the single coordinator owner.
use crate::api_read::{self, Result, bad};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
use tf_api::{ApiError as E, Application, Reply, Request};
use tf_domain::{BranchName, WorkspaceId};
use tf_exec::ownership::RuntimeOwner;
use tf_store::api::Receipt;
pub(crate) struct Command {
    request: Request,
    reply: SyncSender<Result<Reply>>,
}
pub(crate) struct Service {
    cursors: std::sync::Mutex<crate::api_preview::Cursors>,
    root: PathBuf,
    workspace: WorkspaceId,
    busy: Arc<AtomicBool>,
    sender: SyncSender<Command>,
    cancel: SyncSender<crate::dispatch::CancelCommand>,
}
pub(crate) fn service(
    root: PathBuf,
    workspace: WorkspaceId,
    busy: Arc<AtomicBool>,
    cancel: SyncSender<crate::dispatch::CancelCommand>,
) -> (Arc<Service>, Receiver<Command>) {
    let (sender, receiver) = mpsc::sync_channel(1);
    (
        Arc::new(Service {
            cursors: Default::default(),
            root,
            workspace,
            busy,
            sender,
            cancel,
        }),
        receiver,
    )
}
fn request_digest(r: &Request) -> Result<String> {
    api_read::digest(&json!([r.method, r.path, r.query, r.body, r.if_match]))
}
fn replay(v: Value) -> Result<Reply> {
    if let Some(status) = v["error_status"].as_u64() {
        return Err(E::new(
            status as u16,
            "TF_API_OPERATION",
            "The retained operation failed; inspect workspace evidence before submitting a new request",
        ));
    }
    Ok(Reply {
        data: v["data"].clone(),
        context: v["context"].as_object().map(|_| v["context"].clone()),
        etag: v["etag"].as_str().map(str::to_owned),
    })
}
impl Application for Service {
    fn capabilities(&self) -> Value {
        json!({"api_version":1,"protocol_major":1,"workspace":self.workspace.to_string(),"operations":["context","datasets","versions","lineage","source","validation","catalog_diff","plan","catalog_sync","build_accept","build_cancel","catalog_lifecycle","branch_lifecycle","external_lifecycle","events","preview"],"engines":["polars"],"limits":{"body_bytes":tf_api::BODY_LIMIT,"in_flight":tf_api::IN_FLIGHT_LIMIT,"page_default":50,"page_max":200,"graph_default":100,"graph_max":500,"graph_expansion_threshold":500,"source_bytes":65536,"event_replay":10000,"event_streams":4,"preview_rows":1000,"preview_bytes":2097152,"preview_cell_bytes":4096},"metadata_reads_import_code":false,"external_reads":"provider_owned_leased","schedules":false,"ui":false})
    }
    fn call(&self, r: Request) -> Result<Reply> {
        if r.path == "/api/v1/previews" && r.method == "POST" {
            return crate::api_preview::read(&self.root, self.workspace, &r, &self.cursors);
        }
        if r.method == "GET" && matches!(r.path.as_str(), "/api/v1/events" | "/api/v1/events/page")
        {
            if r.query.keys().any(|k| k != "after") {
                return Err(E::invalid());
            }
            let after = r
                .query
                .get("after")
                .map(|s| {
                    let n = s.parse::<i64>().map_err(|_| E::invalid())?;
                    if n < 0 || s != &n.to_string() {
                        return Err(E::invalid());
                    }
                    Ok(n)
                })
                .transpose()?;
            let rt = api_read::runtime()?;
            let mut rd = rt.block_on(api_read::reader(&self.root))?;
            let page = rt
                .block_on(rd.event_page(&self.workspace.to_string(), after))
                .map_err(bad)?;
            rt.block_on(rd.close()).map_err(bad)?;
            return Ok(Reply::metadata(page));
        }
        if r.method == "GET" {
            return api_read::read(&self.root, self.workspace, &r);
        }
        api_read::keys(&r, &[])?;
        if r.path == "/api/v1/validations" || r.path == "/api/v1/catalog/diff" {
            let c = api_read::context(&self.root, self.workspace, &r)?;
            let body: Preparation =
                serde_json::from_value(r.body.clone()).map_err(|_| E::invalid())?;
            let root = self.root.to_string_lossy().into_owned();
            let operation = if r.path.ends_with("diff") {
                crate::preparation::Operation::Check
            } else {
                crate::preparation::Operation::Validate
            };
            let report = crate::preparation::execute(operation, body.python.as_ref(), Some(&root))
                .map_err(|_| {
                    E::new(
                        422,
                        "TF_API_VALIDATION",
                        "Workspace validation failed; inspect the CLI validation diagnostics",
                    )
                })?;
            if api_read::context(&self.root, self.workspace, &r)?.fingerprint != c.fingerprint {
                return Err(E::conflict());
            }
            return Ok(c.reply(report.value));
        }
        let key = r.key.ok_or_else(E::invalid)?;
        let digest = request_digest(&r)?;
        let rt = api_read::runtime()?;
        let mut rd = rt.block_on(api_read::reader(&self.root))?;
        let receipt = rt.block_on(rd.api_receipt(key, &digest)).map_err(bad)?;
        rt.block_on(rd.close()).map_err(bad)?;
        match receipt {
            Receipt::Complete(v) => return replay(v),
            Receipt::Conflict | Receipt::Pending => return Err(E::conflict()),
            Receipt::New => {}
        }
        // Cancellation is intrinsically idempotent by build identity and is serviced while
        // workers run through the same authenticated cancellation mailbox as the CLI.
        if let Some(id) = r
            .path
            .strip_prefix("/api/v1/builds/")
            .and_then(|s| s.strip_suffix("/cancel"))
        {
            if r.body != json!({}) {
                return Err(E::invalid());
            }
            let c = api_read::context(&self.root, self.workspace, &r)?;
            if r.if_match.as_deref() != Some(&c.fingerprint) {
                return Err(E::conflict());
            }
            let build = id.parse::<tf_domain::BuildId>().map_err(|_| E::invalid())?;
            let mut rd = rt.block_on(api_read::reader(&self.root))?;
            let report = rt
                .block_on(rd.build_report(self.workspace, build))
                .map_err(|_| E::missing())?;
            rt.block_on(rd.close()).map_err(bad)?;
            if report["plan"]["output"]["name"] != c.branch.as_str() {
                return Err(E::conflict());
            }
            let (tx, rx) = mpsc::sync_channel(1);
            self.cancel
                .try_send(crate::dispatch::CancelCommand {
                    build,
                    reply: tx,
                    api: Some((key, digest)),
                })
                .map_err(|_| E::busy())?;
            let disposition = rx
                .recv_timeout(Duration::from_secs(10))
                .map_err(|_| E::busy())?
                .map_err(|_| E::conflict())?;
            let result =
                json!({"build":build.to_string(),"disposition":format!("{disposition:?}")});
            return Ok(Reply::metadata(result));
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(E::busy());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        if self
            .sender
            .try_send(Command {
                request: r,
                reply: tx,
            })
            .is_err()
        {
            self.busy.store(false, Ordering::Release);
            return Err(E::busy());
        }
        // A disconnected HTTP client cannot withdraw an accepted coordinator command.
        rx.recv_timeout(Duration::from_secs(7200))
            .map_err(|_| E::busy())?
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    #[serde(default)]
    python: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    branch: String,
    targets: Vec<String>,
    #[serde(default)]
    python: Option<String>,
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    boundaries: Vec<String>,
    #[serde(default)]
    exclusions: Vec<String>,
    #[serde(default)]
    refresh_sources: Vec<String>,
    #[serde(default)]
    pins: Vec<String>,
    #[serde(default)]
    fallbacks: Option<Vec<String>>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    parameters: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    git_ref: Option<String>,
    #[serde(default)]
    require_current: bool,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    validation_timeout_seconds: Option<u64>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    #[default]
    Full,
    Selected,
    Between,
}
impl Selection {
    fn into_request(self) -> Result<(crate::build_plan::Request, crate::build_plan::Options)> {
        if self.targets.is_empty() || self.targets.len() > 1000 {
            return Err(E::invalid());
        }
        let req = crate::build_plan::Request {
            python: self.python,
            branch: self.branch.parse().map_err(|_| E::invalid())?,
            mode: match self.mode {
                Mode::Full => tf_plan::scope::Mode::Full,
                Mode::Selected => tf_plan::scope::Mode::Selected,
                Mode::Between => tf_plan::scope::Mode::Between,
            },
            targets: self.targets,
            boundaries: self.boundaries,
            exclusions: self.exclusions,
            refresh_sources: self.refresh_sources,
            pins: self
                .pins
                .into_iter()
                .map(|p| p.parse().map_err(|_| E::invalid()))
                .collect::<Result<_>>()?,
            fallbacks: self
                .fallbacks
                .map(|v| {
                    v.into_iter()
                        .map(|s| s.parse().map_err(|_| E::invalid()))
                        .collect::<Result<Vec<BranchName>>>()
                })
                .transpose()?,
            force: self.force,
            parameters: json!({}),
        };
        let options = crate::build_plan::Options {
            git_ref: self.git_ref,
            require_current: self.require_current,
            timeout_seconds: self.timeout_seconds,
            validation_timeout_seconds: self.validation_timeout_seconds,
            parameters: self.parameters.into_iter().collect(),
            explain: true,
        };
        Ok((req, options))
    }
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Build {
    Plan { plan_id: String },
    Request { request: Box<Selection> },
}
pub(crate) fn drain(
    owner: &mut RuntimeOwner,
    commands: &Receiver<Command>,
    busy: &AtomicBool,
) -> Result<()> {
    if let Ok(command) = commands.try_recv() {
        let result = execute(owner, &command.request);
        let _ = command.reply.try_send(result);
        busy.store(false, Ordering::Release);
    }
    Ok(())
}
fn execute(owner: &mut RuntimeOwner, r: &Request) -> Result<Reply> {
    let root = owner.workspace_root().to_owned();
    let wid = owner.workspace_id().map_err(bad)?;
    if (r.query.contains_key("plan") && r.path != "/api/v1/builds")
        || r.query.contains_key("version")
    {
        return Err(E::conflict());
    }
    let key = r.key.ok_or_else(E::invalid)?;
    let digest = request_digest(r)?;
    let c = api_read::context(&root, wid, r)?;
    if r.if_match.as_deref() != Some(&c.fingerprint) {
        return Err(E::conflict());
    }
    let rt = api_read::runtime()?;
    let receipt = rt.block_on(async {
        let mut s = owner.open_store().await.map_err(bad)?;
        let result = s
            .repository()
            .map_err(bad)?
            .reserve_api(key, &digest)
            .await
            .map_err(bad);
        s.close().await.map_err(bad)?;
        result
    })?;
    match receipt {
        Receipt::Complete(v) => return replay(v),
        Receipt::New => {}
        _ => return Err(E::conflict()),
    }
    let result = mutate(owner, r, &rt).map(|v| c.reply(v));
    let stored = match &result {
        Ok(v) => json!({"data":v.data,"context":v.context,"etag":v.etag}),
        Err(e) => json!({"error_status":e.status}),
    };
    rt.block_on(async {
        let mut s = owner.open_store().await.map_err(bad)?;
        s.repository()
            .map_err(bad)?
            .finish_api(key, &digest, &stored)
            .await
            .map_err(bad)?;
        s.close().await.map_err(bad)
    })?;
    result
}
fn mutate(owner: &mut RuntimeOwner, r: &Request, rt: &tokio::runtime::Runtime) -> Result<Value> {
    let root = owner.workspace_root().to_string_lossy().into_owned();
    match r.path.as_str() {
        "/api/v1/plans" => {
            let selection: Selection =
                serde_json::from_value(r.body.clone()).map_err(|_| E::invalid())?;
            if r.query.get("branch").map(String::as_str) != Some(selection.branch.as_str()) {
                return Err(E::conflict());
            }
            let (request, options) = selection.into_request()?;
            let p = rt
                .block_on(crate::build_plan::prepare_inner(owner, request, options))
                .map_err(|_| {
                    E::new(
                        422,
                        "TF_API_PLAN",
                        "The requested plan could not be prepared",
                    )
                })?;
            crate::inspection_cli::plan_value("plan", &p).map_err(bad)
        }
        "/api/v1/builds" => {
            let b: Build = serde_json::from_value(r.body.clone()).map_err(|_| E::invalid())?;
            let id = match b {
                Build::Plan { plan_id } => {
                    if r.query.get("plan") != Some(&plan_id.to_string()) {
                        return Err(E::conflict());
                    }
                    plan_id.parse().map_err(|_| E::invalid())?
                }
                Build::Request { request } => {
                    if r.query.contains_key("plan")
                        || r.query.get("branch").map(String::as_str)
                            != Some(request.branch.as_str())
                    {
                        return Err(E::conflict());
                    }
                    let (request, options) = request.into_request()?;
                    rt.block_on(crate::build_plan::prepare_inner(owner, request, options))
                        .map_err(|_| {
                            E::new(
                                422,
                                "TF_API_PLAN",
                                "The requested plan could not be prepared",
                            )
                        })?
                        .id
                        .parse()
                        .map_err(bad)?
                }
            };
            let accepted = rt
                .block_on(crate::build_plan::accept_inner(owner, id))
                .map_err(|_| E::conflict())?;
            Ok(json!({"build":accepted.build.to_string(),"plan":id.to_string(),"state":"QUEUED"}))
        }
        "/api/v1/catalog/sync" => {
            let b: Preparation =
                serde_json::from_value(r.body.clone()).map_err(|_| E::invalid())?;
            Ok(crate::preparation::execute_owned(
                crate::preparation::Operation::Sync,
                b.python.as_ref(),
                Some(&root),
                Some(owner),
            )
            .map_err(|_| E::conflict())?
            .value)
        }
        "/api/v1/catalog" | "/api/v1/branches" | "/api/v1/externals" => lifecycle(owner, r, &root),
        _ => Err(E::missing()),
    }
}
// Translate closed JSON fields to the existing typed CLI adapter in-process, never a shell.
// There is no arbitrary argv endpoint and no competing lifecycle policy.
fn lifecycle(owner: &mut RuntimeOwner, r: &Request, root: &String) -> Result<Value> {
    let family = match r.path.as_str() {
        "/api/v1/catalog" => "catalog",
        "/api/v1/branches" => "branch",
        "/api/v1/externals" => "external",
        _ => return Err(E::missing()),
    };
    let body = r.body.as_object().ok_or_else(E::invalid)?;
    let operation = body
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(E::invalid)?;
    let (pos, options, flags): (&[&str], &[&str], &[&str]) = match (family, operation) {
        ("catalog", "rename") => (
            &["reference", "new_path"],
            &["python"],
            &["keep_alias", "yes"],
        ),
        ("catalog", "remove") => (&["reference"], &["python"], &["yes"]),
        ("branch", "create") => (&["name"], &[], &["dry_run"]),
        ("branch", "rename") => (&["name", "new_name"], &[], &["dry_run"]),
        ("branch", "delete") => (&["name"], &[], &["dry_run", "yes"]),
        ("external", "add") => (
            &[],
            &["workspace", "dataset", "as", "branch"],
            &["no_fallback"],
        ),
        ("external", "remove") => (&["alias"], &["python"], &["yes"]),
        _ => return Err(E::invalid()),
    };
    if body.keys().any(|k| {
        k != "operation"
            && !pos.contains(&k.as_str())
            && !options.contains(&k.as_str())
            && !flags.contains(&k.as_str())
            && !(family == "external" && operation == "add" && k == "fallback")
    }) {
        return Err(E::invalid());
    }
    let mut args = vec!["transflow".into(), family.into(), operation.into()];
    for key in options {
        if let Some(v) = body.get(*key) {
            args.push(format!(
                "--{}={}",
                key.replace('_', "-"),
                v.as_str().ok_or_else(E::invalid)?
            ));
        }
    }
    for key in flags {
        if let Some(v) = body.get(*key)
            && v.as_bool().ok_or_else(E::invalid)?
        {
            args.push(format!("--{}", key.replace('_', "-")));
        }
    }
    if family == "external"
        && operation == "add"
        && let Some(value) = body.get("fallback")
    {
        for name in value.as_array().ok_or_else(E::invalid)? {
            args.push(format!(
                "--fallback={}",
                name.as_str().ok_or_else(E::invalid)?
            ));
        }
    }
    if !pos.is_empty() {
        args.push("--".into());
        for key in pos {
            args.push(
                body.get(*key)
                    .and_then(Value::as_str)
                    .ok_or_else(E::invalid)?
                    .into(),
            );
        }
    }
    let matches = crate::command()
        .try_get_matches_from(args)
        .map_err(|_| E::invalid())?;
    let args = matches.subcommand_matches(family).ok_or_else(E::invalid)?;
    match family {
        "catalog" => Ok(crate::catalog::execute_owned(args, Some(root), Some(owner))
            .map_err(|_| E::conflict())?
            .value),
        "branch" => Ok(crate::branch::execute_owned(args, Some(root), Some(owner))
            .map_err(|_| E::conflict())?
            .value),
        _ => Ok(
            crate::external::execute_owned(args, Some(root), Some(owner))
                .map_err(|_| E::conflict())?
                .value,
        ),
    }
}

/// Shared cancellation acknowledgement with durable HTTP retry protection.
pub(crate) fn cancel_owned(
    owner: &mut RuntimeOwner,
    rt: &tokio::runtime::Runtime,
    command: &crate::dispatch::CancelCommand,
) -> Result<tf_domain::execution::CancelRequest> {
    use tf_domain::execution::CancelRequest;
    if let Some((id, digest)) = &command.api {
        let receipt = rt.block_on(async {
            let mut s = owner.open_store().await.map_err(bad)?;
            let result = s
                .repository()
                .map_err(bad)?
                .reserve_api(*id, digest)
                .await
                .map_err(bad);
            s.close().await.map_err(bad)?;
            result
        })?;
        match receipt {
            Receipt::New => {}
            Receipt::Complete(v) => {
                return match v["data"]["disposition"].as_str() {
                    Some("Requested") => Ok(CancelRequest::Requested),
                    Some("AlreadyRequested") => Ok(CancelRequest::AlreadyRequested),
                    Some("TooLate") => Ok(CancelRequest::TooLate),
                    _ => Err(E::conflict()),
                };
            }
            _ => return Err(E::conflict()),
        }
    }
    let result = crate::build_transport::cancel_owned(owner, rt, command.build).map_err(bad);
    if let Some((id, digest)) = &command.api {
        let stored = match &result {
            Ok(disposition) => {
                json!({"data":{"build":command.build.to_string(),"disposition":format!("{disposition:?}")},"context":null,"etag":null})
            }
            Err(e) => json!({"error_status":e.status}),
        };
        rt.block_on(async {
            let mut s = owner.open_store().await.map_err(bad)?;
            s.repository()
                .map_err(bad)?
                .finish_api(*id, digest, &stored)
                .await
                .map_err(bad)?;
            s.close().await.map_err(bad)
        })?;
    }
    result
}
