//! Typed schedule editor metadata and scope previews, independent of the foreground selection.
use crate::api_read::{self, Result, bad};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
use tf_api::{ApiError as E, Reply, Request};
use tf_domain::{ScheduleId, WorkspaceId};
use tf_exec::ownership::RuntimeOwner;
use tf_protocol::schedule::{Definition, Trigger};

fn definition(v: &Value) -> Result<Definition> {
    Definition::decode(v).map_err(|_| {
        E::new(
            422,
            "TF_API_SCHEDULE_DEFINITION",
            "Check the schedule name, source, branches, targets and condition limits",
        )
    })
}
pub(crate) fn clock(r: &Request) -> Result<Reply> {
    if !r.query.is_empty() {
        return Err(E::invalid());
    }
    let d = definition(&r.body)?;
    Ok(Reply::metadata(clock_definition(&d)?))
}
pub(crate) fn clock_definition(d: &Definition) -> Result<Value> {
    let now = crate::preparation::now().map_err(bad)?;
    let mut leaves = Vec::new();
    visit(&d.trigger, &mut |t| {
        if let Trigger::Cron {
            id,
            expression,
            timezone,
            duplicate_time,
        } = t
        {
            leaves.push(tf_domain::schedule_clock::Leaf {
                id: id.clone(),
                expression: expression.clone(),
                timezone: timezone.clone(),
                duplicate_time: duplicate_time.clone(),
            });
        }
    });
    let mut previews = Vec::new();
    for leaf in leaves {
        let fires = tf_schedule::preview(&leaf, now).map_err(|_| {
            E::new(
                422,
                "TF_API_SCHEDULE_CLOCK",
                "Use five numeric cron fields and a supported IANA timezone",
            )
        })?;
        previews.push(json!({"id":leaf.id,"fires":fires.into_iter().map(|f|json!({"at_us":f.at_us.to_string(),"local":f.local,"timezone":f.timezone,"tzdb_version":f.tzdb_version})).collect::<Vec<_>>()}));
    }
    Ok(json!({"leaves":previews}))
}
fn visit(t: &Trigger, f: &mut impl FnMut(&Trigger)) {
    match t {
        Trigger::And { children } | Trigger::Or { children } => {
            for c in children {
                visit(c, f)
            }
        }
        _ => f(t),
    }
}
pub(crate) fn read(root: &Path, workspace: WorkspaceId, r: &Request) -> Result<Reply> {
    api_read::keys(
        r,
        if r.path.ends_with("/roles") {
            &["after"]
        } else {
            &[]
        },
    )?;
    let c = api_read::context(root, workspace, r)?;
    if c.value["selection"]["kind"] != "retained_current" {
        return Err(E::conflict());
    }
    if r.path == "/api/v1/schedules/defaults" {
        let capture = c.capture().map_err(|_| {
            E::new(
                422,
                "TF_API_SCHEDULE_SOURCE",
                "Validate the workspace before creating a schedule so its source can be saved",
            )
        })?;
        let config = capture
            .read(Path::new("workspace.toml"), 16 * 1024 * 1024)
            .map_err(bad)?;
        let config = tf_catalog::workspace::WorkspaceConfig::parse(
            std::str::from_utf8(&config).map_err(bad)?,
        )
        .map_err(bad)?;
        let p = config.input_policies().map_err(bad)?;
        let source = if let Some(git) = capture.git() {
            if let Some(reference) = git.requested_ref() {
                json!({"kind":"git_ref","ref":reference})
            } else if let Some(branch) = git.branch() {
                json!({"kind":"git_ref","ref":format!("refs/heads/{branch}")})
            } else {
                json!({"kind":"git_ref","ref":git.commit().ok_or_else(E::missing)?})
            }
        } else {
            json!({"kind":"fixed_snapshot","snapshot_id":capture.id().map_err(bad)?.to_string()})
        };
        let target = c
            .registry
            .datasets()
            .find(|d| !d.is_tombstone())
            .ok_or_else(E::missing)?
            .key();
        let policy = json!({"default":p.defaults().iter().map(ToString::to_string).collect::<Vec<_>>(),"rules":p.rules().iter().map(|(k,v)|(k.to_string(),json!(v.iter().map(ToString::to_string).collect::<Vec<_>>()))).collect::<BTreeMap<_,_>>()});
        let value = json!({"format_version":1,"name":"New schedule","description":"","trigger":{"kind":"manual"},"build":{"source":source,"data_branch":c.branch.to_string(),"fallback_branches":c.value["fallback_policy"].as_array().ok_or_else(E::internal)?.iter().skip(1).cloned().collect::<Vec<_>>(),"input_fallback_policy":policy,"provider_fallback_policies":{},"targets":[{"workspace_id":workspace.to_string(),"dataset_id":target.dataset_id().to_string()}],"build_mode":"full","boundaries":[],"exclusions":[],"refresh_sources":[],"parameters":{},"force":false,"require_current":false,"timeout_seconds":3600,"validation_timeout_seconds":3600},"policies":{"max_attempts":1,"retryable_classes":[],"abort_on_failure":true,"overlap_policy":"coalesce_latest","max_pending":100,"allow_overlapping_builds":false,"misfire_policy":"coalesce_latest","max_catch_up":10,"token_window_seconds":86400,"acknowledge_no_expiry":false,"max_consecutive_builds":5,"minimum_delay_seconds":1}});
        definition(&value)?;
        return Ok(c.reply(value));
    }
    let id = r
        .path
        .strip_prefix("/api/v1/schedules/")
        .and_then(|v| v.strip_suffix("/roles"))
        .ok_or_else(E::missing)?
        .parse::<ScheduleId>()
        .map_err(|_| E::invalid())?;
    let rt = api_read::runtime()?;
    let row = rt
        .block_on(async {
            let mut reader = api_read::reader(root).await?;
            let result = reader.schedule(workspace, id).await.map_err(bad);
            reader.close().await.map_err(bad)?;
            result
        })?
        .ok_or_else(E::missing)?;
    let d = definition(&row["definition"])?;
    let mut roles: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut add = |d: &tf_protocol::schedule::Dataset, role| {
        let identity = format!("dataset:{}:{}", d.workspace_id, d.dataset_id);
        let v = roles.entry(identity).or_default();
        if !v.contains(&role) {
            v.push(role)
        }
    };
    for t in &d.build.targets {
        add(t, "target")
    }
    for t in &d.build.exclusions {
        add(t, "excluded")
    }
    for t in &d.build.boundaries {
        add(t, "boundary")
    }
    visit(&d.trigger, &mut |t| match t {
        Trigger::DatasetPublished { dataset, .. } | Trigger::DatasetHeadChanged { dataset, .. } => {
            add(dataset, "trigger")
        }
        Trigger::BuildSucceeded { targets, .. } => {
            for d in targets {
                add(d, "trigger")
            }
        }
        _ => {}
    });
    let entries = tf_catalog::browse::entries(&c.registry);
    let total = roles.len();
    let after = r.query.get("after").map(String::as_str).unwrap_or("");
    if !after.is_empty() {
        let parts = after.split(':').collect::<Vec<_>>();
        if parts.len() != 3
            || parts[0] != "dataset"
            || tf_domain::DatasetKey::from_text(parts[1], parts[2]).is_err()
        {
            return Err(E::invalid());
        }
    }
    let mut iter = roles
        .into_iter()
        .filter(|(identity, _)| identity.as_str() > after);
    let nodes = iter
        .by_ref()
        .take(1000)
        .map(|(identity, roles)| {
            let paths = entries
                .iter()
                .filter(|e| {
                    format!(
                        "dataset:{}:{}",
                        e["workspace_id"].as_str().unwrap_or(""),
                        e["dataset_id"].as_str().unwrap_or("")
                    ) == identity
                })
                .filter_map(|e| e["path"].as_str())
                .collect::<Vec<_>>();
            json!({"identity":identity,"roles":roles,"paths":paths})
        })
        .collect::<Vec<_>>();
    let next_cursor = if iter.next().is_some() {
        nodes.last().map(|n| n["identity"].clone())
    } else {
        None
    };
    Ok(c.reply(json!({"schedule_id":id.to_string(),"etag":row["etag"],"nodes":nodes,"basis":"saved_definition","total":total.to_string(),"next_cursor":next_cursor})))
}
pub(crate) fn preview(owner: &mut RuntimeOwner, r: &Request) -> Result<Reply> {
    let workspace = owner.workspace_id().map_err(bad)?;
    let c = api_read::context(owner.workspace_root(), workspace, r)?;
    if r.if_match.as_deref() != Some(&c.fingerprint) {
        return Err(E::conflict());
    }
    let d = definition(&r.body)?;
    let (request, mut options) =
        crate::schedule_dispatch::template_request(workspace, &d.build).map_err(bad)?;
    options.schedule_preview = Some(d.build);
    options.explain = true;
    let rt = api_read::runtime()?;
    let plan=rt.block_on(crate::build_plan::prepare_inner(owner,request,options)).map_err(|_|E::new(422,"TF_API_SCHEDULE_SCOPE","The schedule scope could not be prepared; check its saved source, targets, boundaries and frozen input policies"))?;
    Ok(c.reply(crate::inspection_cli::plan_value("plan", &plan).map_err(bad)?))
}
