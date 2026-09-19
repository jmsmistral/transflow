//! Registry browsing and explicit lifecycle edits. Historical versions are never deleted.
use crate::preparation::{self, Error};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
use tf_catalog::{
    RegistrySnapshot, browse,
    candidate::{CandidateCatalog, CandidateIdentity, RegistryProposal},
    capture::{CaptureLimits, SourceSnapshot},
    editor::{EditorCache, EditorError, Overlay},
    registry_write::WriteContext,
    workspace::{Workspace, read_authoring},
};
use tf_domain::{DatasetId, DatasetPath, DatasetScope};
use tf_exec::{
    environment,
    ownership::{CoordinatorMode, RuntimeOwner},
};

pub(crate) struct Report {
    pub value: Value,
    pub blocked: bool,
}
impl Report {
    pub fn human(&self) -> Result<String, tf_domain::diagnostic::DiagnosticError> {
        let v = &self.value;
        let mut text = format!(
            "Catalogue {}{}\n",
            v["operation"].as_str().unwrap_or("inspection"),
            if v["applied"] == true { " applied" } else { "" }
        );
        for entry in v["entries"].as_array().into_iter().flatten() {
            text.push_str(&format!(
                "{}  {}  {}{}\n",
                entry["path"].as_str().unwrap_or(""),
                entry["dataset_id"].as_str().unwrap_or(""),
                entry["kind"].as_str().unwrap_or(""),
                if entry["tombstone"] == true {
                    " (tombstone)"
                } else {
                    ""
                }
            ));
            text.push_str(&format!(
                "  Retained versions: {}; branch: {}\n",
                entry["version_count"]
                    .as_str()
                    .unwrap_or("provider not inspected"),
                entry["branch_context"].as_str().unwrap_or("unknown")
            ));
            if entry["alias_count"]
                .as_str()
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0)
                > 8
            {
                text.push_str("  Alias display is limited to the first 8 names.\n");
            }
            if let Some(producer) = entry["producer"].as_object() {
                text.push_str(&format!(
                    "  Retained producer (may be stale): {}:{}\n",
                    producer["path"].as_str().unwrap_or(""),
                    producer["line"]
                ));
            }
        }
        if let Some(cursor) = v["next_cursor"].as_str() {
            text.push_str(&format!("Next cursor: {cursor}\n"));
        }
        if matches!(v["operation"].as_str(), Some("rename" | "remove")) {
            text.push_str(&format!(
                "Impacted declarations/references: {}\n",
                v["impact"]["total"].as_str().unwrap_or("0")
            ));
            for item in v["impact"]["items"].as_array().into_iter().flatten() {
                text.push_str(&format!(
                    "  {}:{}  {}\n",
                    item["path"].as_str().unwrap_or(""),
                    item["line"],
                    item["reference"].as_str().unwrap_or("")
                ));
            }
            if v["impact"]["total"]
                .as_str()
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0)
                > 64
            {
                text.push_str("Impact display is limited to the first 64 entries.\n");
            }
            if v["alias_retained"] == true {
                text.push_str("The old path remains an explicit alias.\n");
            } else if v["operation"] == "rename" {
                text.push_str("Old-path strings and C expressions may break; update the reported Python declarations.\n");
            }
            for blocker in v["blockers"].as_array().into_iter().flatten() {
                text.push_str(&format!("Blocked: {}\n", blocker.as_str().unwrap_or("")));
            }
            if !self.blocked && v["applied"] != true {
                text.push_str(
                    "Preview only. Repeat with --yes to apply this explicit registry change.\n",
                );
            }
        }
        let redactor = tf_domain::diagnostic::Redactor::default();
        text.lines()
            .map(|line| {
                redactor
                    .text(line)
                    .map(|text| format!("{}\n", text.as_str()))
            })
            .collect()
    }
}
fn bounded(value: &Value) -> Result<(), Error> {
    if serde_json::to_vec(value).map_err(|_| Error::Context)?.len() > 512 * 1024 {
        return Err(Error::ReportLimit);
    }
    Ok(())
}
fn names_path(reference: &Value, path: &str) -> bool {
    (reference["form"] == "string" && reference["value"] == path)
        || (reference["form"] == "bound" && reference["path"] == path)
}
fn base(registry: &RegistrySnapshot, operation: &str) -> Value {
    json!({"kind":"catalog","operation":operation,"applied":false,"registry_fingerprint":registry.fingerprint().hex(),"entries":[],"next_cursor":null,"total":"0","impact":{"total":"0","items":[]},"blockers":[],"previous_path":null,"new_path":null,"alias_retained":false})
}
fn retained(workspace: &Workspace) -> Result<BTreeMap<DatasetId, Value>, Error> {
    let mut locations = BTreeMap::new();
    if let Some(value) = tf_catalog::graph_cache::latest(workspace.root())? {
        let id = value["source_snapshot_id"]
            .as_str()
            .ok_or(Error::Context)?
            .parse()
            .map_err(|_| Error::Context)?;
        let capture = SourceSnapshot::open(
            &workspace.root().join(".transflow/runtime/source-snapshots"),
            id,
        )?;
        let bytes = capture.read(Path::new(".transflow/catalog.toml"), 16 * 1024 * 1024)?;
        let registry = RegistrySnapshot::parse(
            workspace.config().id(),
            std::str::from_utf8(&bytes).map_err(|_| Error::Context)?,
        )?;
        let candidate = CandidateCatalog::prepare(&registry, id, &value["discovery"])?;
        for def in candidate.definitions() {
            if let CandidateIdentity::Registered(key) = def.output {
                locations.insert(key.dataset_id(),json!({"path":def.location.path,"line":def.location.line,"source_snapshot_id":id.to_string(),"stale":true}));
            }
        }
    }
    Ok(locations)
}
async fn reader(workspace: &Workspace) -> Result<Option<tf_store::Reader>, Error> {
    let path = workspace.root().join(".transflow/runtime/catalog.sqlite");
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(_) => Ok(Some(tf_store::Reader::open_existing(&path).await?)),
    }
}
async fn enrich(workspace: &Workspace, entries: &mut [Value]) -> Result<(), Error> {
    let producers = retained(workspace)?;
    let mut reader = reader(workspace).await?;
    for entry in entries {
        entry["producer"] = Value::Null;
        entry["version_count"] = Value::Null;
        entry["recent_versions"] = json!([]);
        entry["branch_context"] = tf_domain::diagnostic::Redactor::default()
            .text(workspace.config().default_branch())
            .map_err(|_| Error::Context)?
            .as_str()
            .into();
        if entry["origin"] == "local" {
            let id = entry["dataset_id"]
                .as_str()
                .ok_or(Error::Context)?
                .parse()
                .map_err(|_| Error::Context)?;
            entry["producer"] = producers.get(&id).cloned().unwrap_or(Value::Null);
            entry["version_count"] = "0".into();
            if let Some(reader) = reader.as_mut() {
                let metadata = reader.catalog_metadata(workspace.config().id(), id).await?;
                entry["version_count"] = metadata["version_count"].clone();
                entry["recent_versions"] = metadata["recent_versions"].clone();
            }
        } else {
            entry["branch_context"] = "provider not inspected".into();
        }
    }
    if let Some(reader) = reader {
        reader.close().await?;
    }
    Ok(())
}
pub(crate) fn execute(args: &clap::ArgMatches, explicit: Option<&String>) -> Result<Report, Error> {
    let (operation, args) = args.subcommand().ok_or(Error::Context)?;
    let workspace = Workspace::load(&std::env::current_dir()?, explicit.map(Path::new))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    if matches!(operation, "list" | "show") {
        let registry = RegistrySnapshot::parse(
            workspace.config().id(),
            &read_authoring(&workspace.root().join(".transflow/catalog.toml"))?,
        )?;
        let mut value = base(&registry, operation);
        let page = if operation == "list" {
            browse::page(
                &registry,
                args.get_one::<String>("cursor").map(String::as_str),
                usize::from(*args.get_one::<u16>("limit").ok_or(Error::Context)?),
            )?
        } else {
            json!({"entries":[browse::show(&registry,args.get_one::<String>("reference").ok_or(Error::Context)?)?],"total":"1","next_cursor":null})
        };
        for key in ["entries", "total", "next_cursor"] {
            value[key] = page[key].clone();
        }
        runtime.block_on(enrich(
            &workspace,
            value["entries"].as_array_mut().ok_or(Error::Context)?,
        ))?;
        bounded(&value)?;
        return Ok(Report {
            value,
            blocked: false,
        });
    }
    let apply = args.get_flag("yes");
    let mut owner = if apply {
        Some(RuntimeOwner::acquire(
            workspace.root(),
            workspace.config().id(),
            CoordinatorMode::Temporary,
        )?)
    } else {
        None
    };
    if let Some(owner) = owner.as_mut()
        && runtime
            .block_on(crate::reconcile::recover(owner, preparation::now()?))?
            .iter()
            .any(|r| matches!(r, crate::reconcile::RecoveryOutcome::Conflict(_)))
    {
        return Err(Error::Context);
    }
    let inspected = preparation::inspect(&workspace, args.get_one::<String>("python"), apply)?;
    let registry = &inspected.registry;
    let dataset =
        registry.resolve_output(args.get_one::<String>("reference").ok_or(Error::Context)?)?;
    let key = dataset.key();
    let old_path = dataset.path().as_str();
    let keep = operation == "rename" && args.get_flag("keep-alias");
    let replacement = if operation == "rename" {
        let new_path = DatasetPath::parse_for_scope(
            args.get_one::<String>("new-path").ok_or(Error::Context)?,
            DatasetScope::Local,
        )
        .map_err(|_| Error::Context)?;
        registry.render_rename(key.dataset_id(), &new_path, keep)?
    } else {
        registry.render_remove(key.dataset_id())?
    };
    let mut impact = Vec::new();
    for def in inspected.graph.candidate().definitions() {
        if def.output == CandidateIdentity::Registered(key)
            && (operation == "remove" || names_path(&def.declaration["output"]["ref"], old_path))
        {
            impact.push(json!({"path":def.location.path,"line":def.location.line,"reference":format!("Output({old_path}); C.{}",old_path.replace('/',".")),"role":"producer"}));
        }
        for input in &def.inputs {
            if input.identity == CandidateIdentity::Registered(key)
                && (operation == "remove" || names_path(&input.declaration["ref"], old_path))
            {
                impact.push(json!({"path":def.location.path,"line":def.location.line,"reference":format!("Input alias {}: {old_path}; C.{}",input.alias,old_path.replace('/',".")),"role":"input"}));
            }
        }
    }
    let mut blockers = Vec::new();
    if operation == "remove" && !impact.is_empty() {
        blockers.push("Remove or update all current producer/input references before tombstoning this identity".to_owned());
    }
    if let Some(owner) = owner.as_mut() {
        let mut store = runtime.block_on(owner.open_store())?;
        blockers.extend(
            runtime.block_on(
                store
                    .repository()?
                    .catalog_lifecycle_blockers(preparation::now()?),
            )?,
        );
        runtime.block_on(store.close())?;
    } else if let Some(mut reader) = runtime.block_on(reader(&workspace))? {
        blockers.extend(runtime.block_on(reader.catalog_lifecycle_blockers(preparation::now()?))?);
        runtime.block_on(reader.close())?;
    }
    let blocked = !blockers.is_empty();
    let mut value = base(registry, operation);
    value["entries"] = json!([browse::show(registry, old_path)?]);
    value["total"] = "1".into();
    value["previous_path"] = old_path.into();
    value["new_path"] = if operation == "rename" {
        json!(args.get_one::<String>("new-path"))
    } else {
        Value::Null
    };
    value["alias_retained"] = keep.into();
    value["impact"] = json!({"total":impact.len().to_string(),"items":impact.into_iter().take(64).collect::<Vec<_>>()});
    value["blockers"] = json!(blockers);
    bounded(&value)?;
    if apply && !blocked {
        if environment::inspect(
            workspace.root(),
            &inspected.python,
            &inspected.environment_request,
        )? != inspected.env
        {
            return Err(Error::Context);
        }
        let updated = RegistrySnapshot::parse(workspace.config().id(), &replacement)?;
        let mut future = value.clone();
        future["entries"] = json!([browse::show(
            &updated,
            &format!("dataset:{}", key.dataset_id())
        )?]);
        bounded(&future)?;
        let proposal = RegistryProposal::lifecycle(registry, inspected.capture.id()?, replacement)?;
        let completion = runtime.block_on(crate::reconcile::reconcile(
            owner.take().ok_or(Error::Context)?,
            crate::reconcile::ReconcileRequest {
                capture: inspected.capture,
                proposal,
                context: WriteContext::WorkingTree,
                id: inspected.request,
                at_us: preparation::now()?,
            },
        ))?;
        completion.outcome?;
        let final_capture = SourceSnapshot::capture(&workspace, CaptureLimits::default())?;
        if RegistrySnapshot::parse(
            workspace.config().id(),
            &String::from_utf8(
                final_capture.read(Path::new(".transflow/catalog.toml"), 16 * 1024 * 1024)?,
            )
            .map_err(|_| Error::Context)?,
        )?
        .fingerprint()
            != updated.fingerprint()
        {
            return Err(Error::Context);
        }
        EditorCache::open(workspace.root())?.refresh(
            &Overlay::render(&updated.sdk_projection(final_capture.id()?)?)?,
            inspected.request,
            |_| {
                completion
                    .owner
                    .validate_paths()
                    .map_err(|_| EditorError::Conflict)?;
                final_capture
                    .verify_working_copy(&workspace, false)
                    .map_err(|_| EditorError::Conflict)
            },
        )?;
        value["entries"] = json!([browse::show(
            &updated,
            &format!("dataset:{}", key.dataset_id())
        )?]);
        value["registry_fingerprint"] = updated.fingerprint().hex().into();
        value["applied"] = true.into();
    }
    runtime.block_on(enrich(
        &workspace,
        value["entries"].as_array_mut().ok_or(Error::Context)?,
    ))?;
    bounded(&value)?;
    Ok(Report { value, blocked })
}
