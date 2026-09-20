//! Explicit branch commands require no Python imports, environment preparation or Git mutation.
use serde_json::{Value, json};
use std::{
    fs::File,
    io::Read,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use tf_catalog::workspace::{Workspace, WorkspaceConfig, read_authoring};
use tf_domain::{BranchId, BranchName, BranchSelector, FallbackPermission};
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
use tf_store::{
    Reader,
    branch_lifecycle::{BranchChange, BranchRecord},
};
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Config(#[from] tf_catalog::workspace::ConfigError),
    #[error(transparent)]
    Ownership(#[from] tf_exec::ownership::OwnershipError),
    #[error(transparent)]
    Store(#[from] tf_store::StoreError),
    #[error(transparent)]
    Branch(#[from] tf_store::branch_lifecycle::Error),
    #[error("Branch files could not be accessed")]
    Io(#[from] std::io::Error),
    #[error(
        "Branch arguments, clock or authored configuration changed; inspect the workspace and retry"
    )]
    Context,
    #[error(
        "Branch report exceeds its bound; use a smaller list page or simplify the authored policy"
    )]
    Limit,
}
pub(crate) struct Report {
    pub value: Value,
    pub blocked: bool,
}
impl Report {
    pub fn human(&self) -> Result<String, tf_domain::diagnostic::DiagnosticError> {
        let redactor = tf_domain::diagnostic::Redactor::default();
        let safe = |value: &Value| redactor.text(value.as_str().unwrap_or("unknown"));
        let mut text = format!(
            "Data branch {}{}\n",
            self.value["operation"].as_str().unwrap_or("operation"),
            if self.value["applied"] == true {
                " applied"
            } else {
                ""
            }
        );
        if !self.value["requested_name"].is_null() {
            text.push_str(&format!(
                "Requested: {}\n",
                safe(&self.value["requested_name"])?.as_str()
            ));
        }
        if !self.value["new_name"].is_null() {
            text.push_str(&format!(
                "New name: {}\n",
                safe(&self.value["new_name"])?.as_str()
            ));
        }
        for e in self.value["entries"].as_array().into_iter().flatten() {
            text.push_str(&format!(
                "{}  {}  heads: {}{}\n",
                safe(&e["name"])?.as_str(),
                safe(&e["id"])?.as_str(),
                safe(&e["heads"])?.as_str(),
                if e["deleted"] == true {
                    " (deleted)"
                } else {
                    ""
                }
            ));
            text.push_str("  Fallbacks:\n");
            for name in e["fallbacks"].as_array().into_iter().flatten() {
                text.push_str(&format!("    {}\n", safe(name)?.as_str()));
            }
        }
        // Impact strings are constructed from safe fixed labels plus sanitized authored names.
        for i in self.value["impacts"].as_array().into_iter().flatten() {
            text.push_str(&format!("Blocked: {}\n", i.as_str().unwrap_or("unknown")));
        }
        if self.value["confirmation_required"] == true {
            text.push_str("Preview only. Repeat with --yes to tombstone this data branch.\n");
        }
        if let Some(cursor) = self.value["next_cursor"].as_str() {
            text.push_str(&format!("Next cursor: {cursor}\n"));
        }
        text.push_str("Git branches and authored workspace policy are unchanged.\n");
        Ok(text)
    }
}
async fn reader(root: &Path) -> Result<Option<Reader>, Error> {
    let p = root.join(".transflow/runtime/catalog.sqlite");
    match std::fs::symlink_metadata(&p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(_) => Ok(Some(Reader::open_existing(&p).await?)),
    }
}
fn entry(r: &BranchRecord, c: &WorkspaceConfig) -> Result<Value, Error> {
    let p = c
        .input_policies()
        .map_err(|_| Error::Context)?
        .local(
            &r.name,
            BranchSelector::Named(r.name.clone()),
            FallbackPermission::Allowed,
            None,
        )
        .map_err(|_| Error::Context)?;
    Ok(
        json!({"id":r.id.to_string(),"name":r.name.as_str(),"revision":r.revision.to_string(),"deleted":r.deleted,"heads":r.heads.to_string(),"fallbacks":p.candidates().iter().skip(1).map(BranchName::as_str).collect::<Vec<_>>()}),
    )
}
fn policy_impacts(c: &WorkspaceConfig, n: &BranchName) -> Result<Vec<String>, Error> {
    let p = c.input_policies().map_err(|_| Error::Context)?;
    let mut out = vec![];
    if c.default_branch() == n.as_str() {
        out.push("workspace.toml: default_data_branch".into());
    }
    if p.defaults().contains(n) {
        out.push("workspace.toml: branching.default_fallbacks".into());
    }
    for (start, tail) in p.rules() {
        if start == n || tail.contains(n) {
            let safe = tf_domain::diagnostic::Redactor::default()
                .text(start.as_str())
                .map_err(|_| Error::Limit)?;
            out.push(format!(
                "workspace.toml: branching.fallbacks for {}",
                safe.as_str()
            ));
        }
    }
    Ok(out)
}
fn bounded(v: &Value) -> Result<(), Error> {
    tf_protocol::validate_document("BranchResultV1", v).map_err(|_| Error::Context)?;
    if serde_json::to_vec(v).map_err(|_| Error::Context)?.len() > 512 * 1024 {
        Err(Error::Limit)
    } else {
        Ok(())
    }
}
fn id() -> Result<BranchId, Error> {
    let mut b = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    Ok(BranchId::from_bytes(b))
}
pub(crate) fn execute(args: &clap::ArgMatches, explicit: Option<&String>) -> Result<Report, Error> {
    let (operation, args) = args.subcommand().ok_or(Error::Context)?;
    let workspace = Workspace::load(&std::env::current_dir()?, explicit.map(Path::new))?;
    let path = workspace.root().join("workspace.toml");
    let authored = read_authoring(&path)?;
    let config = WorkspaceConfig::parse(&authored)?;
    if config.id() != workspace.config().id() {
        return Err(Error::Context);
    }
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Context)?
            .as_micros(),
    )
    .map_err(|_| Error::Context)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut value = json!({"kind":"branch","operation":operation,"applied":false,"workspace_id":config.id().to_string(),"entries":[],"impacts":[],"next_cursor":null,"confirmation_required":false,"policy_source":"workspace.toml","git_independent":true,"requested_name":null,"new_name":null});
    runtime.block_on(async {
        let mut read = reader(workspace.root()).await?;
        if operation == "list" {
            let limit = *args.get_one::<u16>("limit").ok_or(Error::Context)?;
            let after = args
                .get_one::<String>("cursor")
                .map(|s| s.parse().map_err(|_| Error::Context))
                .transpose()?;
            if let Some(r) = read.as_mut() {
                let mut rows = r.branches(config.id(), after, limit + 1).await?;
                if rows.len() > usize::from(limit) {
                    rows.pop();
                    value["next_cursor"] = rows.last().map(|r| r.id.to_string()).into();
                }
                value["entries"] = rows
                    .iter()
                    .map(|r| entry(r, &config))
                    .collect::<Result<Vec<_>, _>>()?
                    .into();
            }
            if let Some(r) = read {
                r.close().await?;
            }
            bounded(&value)?;
            return Ok(Report {
                value,
                blocked: false,
            });
        }
        let name: BranchName = args
            .get_one::<String>("name")
            .ok_or(Error::Context)?
            .parse()
            .map_err(|_| Error::Context)?;
        if name.as_str().len() > 4096 {
            return Err(Error::Context);
        }
        let current = if let Some(r) = read.as_mut() {
            r.branch(config.id(), &name).await?
        } else {
            None
        };
        if operation != "create" && current.as_ref().is_none_or(|r| r.deleted) {
            return Err(Error::Branch(tf_store::branch_lifecycle::Error::Conflict));
        }
        if operation == "create" && current.as_ref().is_some_and(|r| r.deleted) {
            return Err(Error::Branch(tf_store::branch_lifecycle::Error::Conflict));
        }
        let renamed = if operation == "rename" {
            let n: BranchName = args
                .get_one::<String>("new-name")
                .ok_or(Error::Context)?
                .parse()
                .map_err(|_| Error::Context)?;
            if n.as_str().len() > 4096 {
                return Err(Error::Context);
            }
            if let Some(r) = read.as_mut()
                && r.branch(config.id(), &n).await?.is_some()
            {
                return Err(Error::Branch(tf_store::branch_lifecycle::Error::Conflict));
            }
            Some(n)
        } else {
            None
        };
        value["requested_name"] = name.as_str().into();
        value["new_name"] = renamed.as_ref().map(BranchName::as_str).into();
        let mut impacts = vec![];
        if let Some(n) = &renamed {
            impacts.extend(policy_impacts(&config, n)?);
        }
        if let Some(row) = current.as_ref() {
            value["entries"] = json!([entry(row, &config)?]);
            if operation != "create" {
                impacts.extend(policy_impacts(&config, &name)?);
                if let Some(r) = read.as_mut() {
                    impacts.extend(r.branch_references(row, now).await?);
                }
            }
        }
        if let Some(r) = read {
            r.close().await?;
        }
        // Bound the eventual report before any mutation, including the destination policy.
        let proposed = BranchRecord {
            id: BranchId::from_bytes([0; 16]),
            name: renamed.clone().unwrap_or_else(|| name.clone()),
            revision: i64::MAX,
            deleted: operation == "delete",
            heads: i64::MAX,
        };
        let mut preview = value.clone();
        preview["entries"] = json!([entry(&proposed, &config)?]);
        bounded(&preview)?;
        impacts.sort();
        impacts.dedup();
        let dry = args.get_flag("dry-run");
        let apply = !dry && (operation != "delete" || args.get_flag("yes"));
        value["confirmation_required"] = (operation == "delete" && !apply).into();
        value["impacts"] = json!(impacts);
        bounded(&value)?;
        if !impacts.is_empty() || !apply {
            return Ok(Report {
                value,
                blocked: !impacts.is_empty(),
            });
        }
        let mut owner =
            RuntimeOwner::acquire(workspace.root(), config.id(), CoordinatorMode::Temporary)?;
        // Never mutate policy files; refuse if the author changed the context during preview.
        if read_authoring(&path)? != authored {
            return Err(Error::Context);
        }
        let mut store = owner.open_store().await?;
        let repo = store.repository()?;
        repo.register_workspace(
            config.id(),
            workspace.root().to_str().ok_or(Error::Context)?,
            now,
        )
        .await?;
        if operation == "create" {
            repo.create_branch(config.id(), id()?, &name, now).await?;
        } else {
            let row = current.as_ref().ok_or(Error::Context)?;
            let change = if operation == "rename" {
                BranchChange::Rename(
                    args.get_one::<String>("new-name")
                        .ok_or(Error::Context)?
                        .parse()
                        .map_err(|_| Error::Context)?,
                )
            } else {
                BranchChange::Delete
            };
            repo.change_branch(config.id(), row, change, now).await?;
        }
        store.close().await?;
        let mut read = reader(workspace.root()).await?.ok_or(Error::Context)?;
        let selected = if operation == "rename" {
            args.get_one::<String>("new-name")
                .ok_or(Error::Context)?
                .parse()
                .map_err(|_| Error::Context)?
        } else {
            name
        };
        if let Some(row) = read.branch(config.id(), &selected).await? {
            value["entries"] = json!([entry(&row, &config)?]);
        } else {
            return Err(Error::Context);
        }
        read.close().await?;
        value["applied"] = true.into();
        value["confirmation_required"] = false.into();
        bounded(&value)?;
        Ok(Report {
            value,
            blocked: false,
        })
    })
}
