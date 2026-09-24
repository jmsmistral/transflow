//! Explicit foreign identity registration. Provider inspection reads authoring metadata only.
use crate::preparation::{self, Error};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tf_catalog::{
    ExternalRegistration, RegistrySnapshot,
    candidate::{CandidateIdentity, RegistryProposal},
    capture::SourceSnapshot,
    local_write::LocatorWrite,
    registry_write::{RegistryFile, RegistryWriteError, WriteContext},
    workspace::{Workspace, WorkspaceConfig, read_authoring},
};
use tf_domain::{BranchName, DatasetPath, DatasetScope};
use tf_exec::{
    discovery::{self, Scratch},
    ownership::{CoordinatorMode, RuntimeOwner},
};

pub(crate) fn command() -> clap::Command {
    use clap::{Arg, ArgAction, Command};
    Command::new("external")
        .disable_help_flag(true)
        .subcommand_required(true)
        .about("Explicit foreign dataset registrations; provider code is never imported")
        .subcommand(
            Command::new("add")
                .disable_help_flag(true)
                .arg(Arg::new("workspace").long("workspace").required(true))
                .arg(Arg::new("dataset").long("dataset").required(true))
                .arg(Arg::new("as").long("as").required(true))
                .arg(Arg::new("branch").long("branch"))
                .arg(
                    Arg::new("fallback")
                        .long("fallback")
                        .action(ArgAction::Append)
                        .conflicts_with("no-fallback"),
                )
                .arg(
                    Arg::new("no-fallback")
                        .long("no-fallback")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("list")
                .disable_help_flag(true)
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .default_value("50")
                        .value_parser(clap::value_parser!(u16).range(1..=100)),
                )
                .arg(Arg::new("cursor").long("cursor")),
        )
        .subcommand(
            Command::new("show")
                .disable_help_flag(true)
                .arg(Arg::new("alias").required(true)),
        )
        .subcommand(
            Command::new("remove")
                .disable_help_flag(true)
                .arg(Arg::new("alias").required(true))
                .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue))
                .arg(Arg::new("python").long("python")),
        )
}
struct Provider {
    root: PathBuf,
    config_text: String,
    config: WorkspaceConfig,
    registry: RegistrySnapshot,
}
impl Provider {
    fn read(root: &Path) -> Result<Self, Error> {
        let root = root.canonicalize()?;
        let config_text = read_authoring(&root.join("workspace.toml"))?;
        let config = WorkspaceConfig::parse(&config_text)?;
        let bytes = RegistryFile::open(&root)
            .and_then(|f| f.read())
            .map_err(|_| Error::Context)?;
        let registry = RegistrySnapshot::parse(
            config.id(),
            std::str::from_utf8(&bytes).map_err(|_| Error::Context)?,
        )?;
        let p = Self {
            root,
            config_text,
            config,
            registry,
        };
        p.verify()?;
        Ok(p)
    }
    fn verify(&self) -> Result<(), Error> {
        if read_authoring(&self.root.join("workspace.toml"))? != self.config_text {
            return Err(Error::Context);
        }
        RegistryFile::open(&self.root)
            .and_then(|f| f.verify(self.registry.raw_digest()))
            .map_err(|_| Error::Context)
    }
}
fn alias(text: &str) -> Result<DatasetPath, Error> {
    let path = if text.starts_with("external/") {
        text.to_owned()
    } else {
        format!("external/{}", text.replace('.', "/"))
    };
    if path.split('/').count() < 3 {
        return Err(Error::Context);
    }
    DatasetPath::parse_for_scope(&path, DatasetScope::Foreign).map_err(|_| Error::Context)
}
fn entry(w: &Workspace, e: &ExternalRegistration) -> Result<Value, Error> {
    let status = match w.local().provider(e.key().workspace_id()) {
        None => "unconfigured",
        Some(path) => match Provider::read(path) {
            Err(_) => "unavailable",
            Ok(p) if p.config.id() != e.key().workspace_id() => "identity_mismatch",
            Ok(p) if p.registry.by_id(e.key().dataset_id()).is_err() => "dataset_unavailable",
            Ok(_) => "available",
        },
    };
    let redactor = tf_domain::diagnostic::Redactor::default();
    let safe = |s: &str| {
        redactor
            .text(s)
            .map(|t| t.as_str().to_owned())
            .map_err(|_| Error::Context)
    };
    let fallback = e
        .fallback_override()
        .map(|v| {
            v.iter()
                .map(|b| safe(b.as_str()))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    Ok(
        json!({"registration_id":e.id().to_string(),"reference":e.alias().as_str(),"c_reference":format!("C.{}",e.alias().as_str().replace('/',".")),"provider_workspace_id":e.key().workspace_id().to_string(),"provider_dataset_id":e.key().dataset_id().to_string(),"provider_display_path":e.provider_display_path().as_str(),"default_branch":safe(e.default_branch().as_str())?,"fallback_override":fallback,"tombstone":e.is_tombstone(),"locator_status":status,"replica_status":"not_inspected"}),
    )
}
pub(crate) struct Report {
    pub value: Value,
    pub blocked: bool,
}
impl Report {
    pub fn human(&self) -> String {
        let mut text = format!(
            "External {}{}\n",
            self.value["operation"].as_str().unwrap_or("operation"),
            if self.value["applied"] == true {
                " applied"
            } else {
                ""
            }
        );
        for e in self.value["entries"].as_array().into_iter().flatten() {
            text.push_str(&format!("{} ({})\n  Provider: {} / {}\n  Default branch: {}; locator: {}; replica: not inspected{}\n",e["reference"].as_str().unwrap_or(""),e["c_reference"].as_str().unwrap_or(""),e["provider_workspace_id"].as_str().unwrap_or(""),e["provider_dataset_id"].as_str().unwrap_or(""),e["default_branch"].as_str().unwrap_or(""),e["locator_status"].as_str().unwrap_or(""),if e["tombstone"]==true { "; removed" } else { "" }));
        }
        for e in self.value["entries"].as_array().into_iter().flatten() {
            let policy = match e["fallback_override"].as_array() {
                None => "inherit provider policy at planning".to_owned(),
                Some(names) if names.is_empty() => "explicitly disabled".to_owned(),
                Some(names) => names
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" -> "),
            };
            text.push_str(&format!(
                "Fallbacks for {}: {policy}\n",
                e["reference"].as_str().unwrap_or("")
            ));
        }
        if self.value["operation"] == "remove" {
            text.push_str(&format!(
                "Current consumer input references: {}\n",
                self.value["impact_count"].as_str().unwrap_or("unknown")
            ));
        }
        for b in self.value["blockers"].as_array().into_iter().flatten() {
            text.push_str(&format!("Blocked: {}\n", b.as_str().unwrap_or("")));
        }
        if self.value["operation"] == "remove" && self.value["applied"] == false {
            text.push_str("Preview only. Repeat with --yes to retain a removal tombstone.\n");
        }
        if let Some(c) = self.value["next_cursor"].as_str() {
            text.push_str(&format!("Next cursor: {c}\n"));
        }
        text
    }
}
fn bounded(value: &Value) -> Result<(), Error> {
    if serde_json::to_vec(value).map_err(|_| Error::Context)?.len() > 512 * 1024 {
        Err(Error::ReportLimit)
    } else {
        Ok(())
    }
}
pub(crate) fn execute(args: &clap::ArgMatches, explicit: Option<&String>) -> Result<Report, Error> {
    let (op, args) = args.subcommand().ok_or(Error::Context)?;
    let workspace = Workspace::load(&std::env::current_dir()?, explicit.map(Path::new))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let apply = op == "add" || (op == "remove" && args.get_flag("yes"));
    let mut owner = if apply {
        Some(RuntimeOwner::acquire(
            workspace.root(),
            workspace.config().id(),
            CoordinatorMode::Temporary,
        )?)
    } else {
        None
    };
    if let Some(o) = owner.as_mut()
        && runtime
            .block_on(crate::reconcile::recover(o, preparation::now()?))?
            .iter()
            .any(|v| matches!(v, crate::reconcile::RecoveryOutcome::Conflict(_)))
    {
        return Err(Error::Context);
    }
    let registry = RegistrySnapshot::parse(
        workspace.config().id(),
        &read_authoring(&workspace.root().join(".transflow/catalog.toml"))?,
    )?;
    let mut value = json!({"kind":"external","operation":op,"applied":false,"registry_fingerprint":registry.fingerprint().hex(),"entries":[],"total":"0","next_cursor":null,"impact_count":"0","blockers":[]});
    if op == "list" || op == "show" {
        let all: Vec<_> = registry.external_history().collect();
        if op == "show" {
            let path = alias(args.get_one::<String>("alias").ok_or(Error::Context)?)?;
            let e = all
                .iter()
                .find(|e| e.alias() == &path)
                .ok_or(Error::Context)?;
            value["entries"] = json!([entry(&workspace, e)?]);
            value["total"] = "1".into();
        } else {
            let offset = if let Some(c) = args.get_one::<String>("cursor") {
                let (f, n) = c.split_once(':').ok_or(Error::Context)?;
                if f != registry.fingerprint().hex() {
                    return Err(Error::Context);
                }
                n.parse::<usize>().map_err(|_| Error::Context)?
            } else {
                0
            };
            if offset > all.len() {
                return Err(Error::Context);
            }
            let end = (offset + usize::from(*args.get_one::<u16>("limit").ok_or(Error::Context)?))
                .min(all.len());
            value["entries"] = json!(
                all[offset..end]
                    .iter()
                    .map(|e| entry(&workspace, e))
                    .collect::<Result<Vec<_>, _>>()?
            );
            value["total"] = all.len().to_string().into();
            value["next_cursor"] =
                json!((end < all.len()).then(|| format!("{}:{end}", registry.fingerprint().hex())));
        }
        bounded(&value)?;
        return Ok(Report {
            value,
            blocked: false,
        });
    }
    // Read-only previews capture outside the workspace; mutations retain their guarded source.
    let scratch = Scratch::create()?;
    let mut capture =
        SourceSnapshot::registration(&workspace, if apply { None } else { Some(scratch.path()) })?;
    let request = discovery::random_id()?
        .parse()
        .map_err(|_| Error::Context)?;
    let mut blockers = Vec::<String>::new();
    let mut inspected_environment = None;
    let (replacement, path, provider) = if op == "add" {
        let p = Provider::read(Path::new(
            args.get_one::<String>("workspace").ok_or(Error::Context)?,
        ))?;
        let dataset = p
            .registry
            .resolve_output(args.get_one::<String>("dataset").ok_or(Error::Context)?)?;
        let path = alias(args.get_one::<String>("as").ok_or(Error::Context)?)?;
        // --as is deliberately dot-separated, unlike inspection's canonical string reference.
        if args
            .get_one::<String>("as")
            .is_some_and(|s| s.contains('/'))
        {
            return Err(Error::Context);
        }
        let branch: BranchName = args
            .get_one::<String>("branch")
            .map(String::as_str)
            .unwrap_or(p.config.default_branch())
            .parse()
            .map_err(|_| Error::Context)?;
        let fallback = if args.get_flag("no-fallback") {
            Some(vec![])
        } else {
            args.get_many::<String>("fallback")
                .map(|v| {
                    v.map(|s| s.parse::<BranchName>().map_err(|_| Error::Context))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
        };
        if registry.external_history().any(|e| {
            e.alias().as_str().split('/').nth(1) == path.as_str().split('/').nth(1)
                && e.key().workspace_id() != p.config.id()
        }) {
            return Err(Error::External(
                "This provider alias belongs to a different workspace UUID; choose a distinct provider alias",
            ));
        }
        let replacement = if let Some(e) = registry.external_history().find(|e| e.alias() == &path)
        {
            if e.is_tombstone()
                || e.key() != dataset.key()
                || e.default_branch() != &branch
                || e.fallback_override() != fallback.as_deref()
            {
                return Err(Error::External(
                    "This alias already reserves another identity or selector, or was removed; choose a distinct alias",
                ));
            }
            read_authoring(&workspace.root().join(".transflow/catalog.toml"))?
        } else {
            registry.render_external_add(
                discovery::random_id()?
                    .parse()
                    .map_err(|_| Error::Context)?,
                &path,
                dataset,
                &branch,
                fallback.as_deref(),
            )?
        };
        (replacement, path, Some(p))
    } else {
        let path = alias(args.get_one::<String>("alias").ok_or(Error::Context)?)?;
        let e = registry
            .external_history()
            .find(|e| e.alias() == &path)
            .ok_or(Error::Context)?;
        if !e.is_tombstone()
            && capture
                .discovery_files()
                .as_array()
                .is_some_and(|v| !v.is_empty())
        {
            let inspected =
                preparation::inspect(&workspace, args.get_one::<String>("python"), apply)?;
            if inspected.registry.raw_digest() != registry.raw_digest() {
                return Err(Error::Context);
            }
            let count = inspected
                .graph
                .candidate()
                .definitions()
                .iter()
                .flat_map(|d| &d.inputs)
                .filter(|i| i.identity == CandidateIdentity::Registered(e.key()))
                .count();
            value["impact_count"] = count.to_string().into();
            if count > 0 {
                blockers.push(
                    "Update current consumer input references before removing this registration"
                        .into(),
                );
            }
            capture = inspected.capture;
            inspected_environment = Some((
                inspected.python,
                inspected.environment_request,
                inspected.env,
            ));
        }
        if let Some(o) = owner.as_mut() {
            let mut s = runtime.block_on(o.open_store())?;
            blockers.extend(
                runtime.block_on(
                    s.repository()?
                        .catalog_lifecycle_blockers(preparation::now()?),
                )?,
            );
            runtime.block_on(s.close())?;
        } else {
            let db = workspace.root().join(".transflow/runtime/catalog.sqlite");
            if db.try_exists()? {
                let mut r = runtime.block_on(tf_store::Reader::open_existing(&db))?;
                blockers
                    .extend(runtime.block_on(r.catalog_lifecycle_blockers(preparation::now()?))?);
                runtime.block_on(r.close())?;
            }
        }
        (registry.render_external_remove(e.id())?, path, None)
    };
    let updated = RegistrySnapshot::parse(workspace.config().id(), &replacement)?;
    let e = updated
        .external_history()
        .find(|e| e.alias() == &path)
        .ok_or(Error::Context)?;
    let shown = if op == "remove" {
        registry
            .external_history()
            .find(|old| old.id() == e.id())
            .ok_or(Error::Context)?
    } else {
        e
    };
    value["entries"] = json!([entry(&workspace, shown)?]);
    value["total"] = "1".into();
    value["blockers"] = json!(blockers);
    bounded(&value)?;
    if apply && blockers.is_empty() {
        if let Some((python, request, expected)) = inspected_environment
            && tf_exec::environment::inspect(workspace.root(), &python, &request)? != expected
        {
            return Err(Error::Context);
        }
        let proposal = RegistryProposal::lifecycle(&registry, capture.id()?, replacement)?;
        if let Some(p) = provider {
            p.verify()?;
            // A standalone locator is harmless after interruption. Identity is never inferred from it.
            LocatorWrite::prepare(workspace.root(), p.config.id(), &p.root, request)
                .map_err(|_| Error::Context)?
                .apply(|| {
                    owner
                        .as_ref()
                        .ok_or(RegistryWriteError::Conflict)?
                        .validate_paths()
                        .map_err(|_| RegistryWriteError::Conflict)?;
                    capture.verify_working_copy(&workspace, false)?;
                    p.verify().map_err(|_| RegistryWriteError::Conflict)
                })
                .map_err(|_| Error::Context)?;
        }
        let done = runtime.block_on(crate::reconcile::reconcile(
            owner.take().ok_or(Error::Context)?,
            crate::reconcile::ReconcileRequest {
                capture,
                proposal,
                context: WriteContext::WorkingTree,
                id: request,
                at_us: preparation::now()?,
            },
        ))?;
        done.outcome?;
        value["applied"] = true.into();
        value["registry_fingerprint"] = updated.fingerprint().hex().into();
        value["entries"] = json!([entry(
            &Workspace::load(workspace.root(), Some(workspace.root()))?,
            e
        )?]);
    }
    bounded(&value)?;
    Ok(Report {
        value,
        blocked: !blockers.is_empty(),
    })
}
