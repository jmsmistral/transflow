//! Explicit local reachability collection. Dry-run never acquires a writer or creates state.
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tf_catalog::{capture::SourceSnapshot, workspace::Workspace};
use tf_domain::{RequestId, SourceSnapshotId};
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
use tf_protocol::canonical::{ContentDigest, DigestKind};
use tf_store::{
    artifacts::ArtifactStore,
    gc::{GRACE_US, Inventory},
    gc_files::{Directory, Identity},
    retention::RetentionPolicy,
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
    Retention(#[from] tf_store::retention::RetentionError),
    #[error(transparent)]
    Artifact(#[from] tf_store::artifacts::ArtifactError),
    #[error(
        "GC could not verify a local namespace or durable deletion intent; all remaining claims stay fenced"
    )]
    Io(#[from] std::io::Error),
    #[error(
        "GC requires a supported root snapshot, clock and bounded report; inspect pending recovery or unknown root state"
    )]
    Context,
    #[error(
        "GC --apply requires --yes. Review a dry run first; stop the coordinator before standalone apply"
    )]
    Approval,
}
pub(crate) struct Report {
    pub value: Value,
}
impl Report {
    pub fn human(&self) -> String {
        let mut text = format!(
            "Garbage collection {}\nRetention: latest 20 per branch or 30 days, plus live roots; quarantine/orphan grace 7 days; completed logs 30 days.\n",
            if self.value["applied"] == true {
                "applied"
            } else {
                "dry run"
            }
        );
        for item in self.value["items"].as_array().into_iter().flatten() {
            text.push_str(&format!(
                "{} {} {} — {} bytes — {}\n",
                item["action"].as_str().unwrap_or("retain"),
                item["kind"].as_str().unwrap_or("unknown"),
                item["id"].as_str().unwrap_or("unknown"),
                item["bytes"].as_str().unwrap_or("0"),
                item["reason"].as_str().unwrap_or("unknown")
            ));
        }
        text.push_str(&format!(
            "Estimated collectible bytes: {}\n",
            self.value["collectible_bytes"].as_str().unwrap_or("0")
        ));
        if self.value["applied"] != true {
            text.push_str("No durable metadata or payloads changed. Review, then repeat with --apply --yes.\n");
        }
        text
    }
}
pub(crate) fn command() -> clap::Command {
    clap::Command::new("gc")
        .disable_help_flag(true)
        .about("Explain reachability and estimated space; dry run by default")
        .arg(
            clap::Arg::new("dry-run")
                .long("dry-run")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with("apply"),
        )
        .arg(
            clap::Arg::new("apply")
                .long("apply")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("yes")
                .long("yes")
                .action(clap::ArgAction::SetTrue)
                .requires("apply"),
        )
}
struct Item {
    kind: &'static str,
    key: String,
    parent: PathBuf,
    action: &'static str,
    reason: String,
    bytes: u64,
    identity: Option<Identity>,
    claim: Option<RequestId>,
}
fn directory(root: &Path, parent: &Path) -> Result<Option<Directory>, Error> {
    match Directory::open(root, parent) {
        Ok(d) => Ok(Some(d)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn extra_sources(root: &Path) -> Result<BTreeSet<SourceSnapshotId>, Error> {
    let mut roots = BTreeSet::new();
    if let Some(value) = tf_catalog::graph_cache::latest(root).map_err(|_| Error::Context)? {
        roots.insert(
            value["source_snapshot_id"]
                .as_str()
                .ok_or(Error::Context)?
                .parse()
                .map_err(|_| Error::Context)?,
        );
    }
    Ok(roots)
}
fn verify_source(root: &Path, key: &str, expected: tf_domain::WorkspaceId) -> Result<(), Error> {
    let id = key.parse().map_err(|_| Error::Context)?;
    let capture = SourceSnapshot::open(&root.join(".transflow/runtime/source-snapshots"), id)
        .map_err(|_| Error::Context)?;
    let config = capture
        .read(Path::new("workspace.toml"), 1024 * 1024)
        .map_err(|_| Error::Context)?;
    let config = tf_catalog::workspace::WorkspaceConfig::parse(
        std::str::from_utf8(&config).map_err(|_| Error::Context)?,
    )?;
    let live = Workspace::load(root, Some(root))?;
    if config.id() != expected || live.config().id() != expected {
        return Err(Error::Context);
    }
    Ok(())
}
fn item_state(inventory: &Inventory, kind: &str, key: &str, now: i64) -> (Option<RequestId>, bool) {
    let item = inventory.items.get(&(kind.into(), key.into()));
    let claim = item
        .filter(|v| v["state"] == "CLAIMED")
        .and_then(|v| v["claim_id"].as_str())
        .and_then(|s| s.parse().ok());
    let grace = item
        .and_then(|v| v["first_seen_us"].as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .is_some_and(|at| now.saturating_sub(at) >= GRACE_US);
    (claim, grace)
}
fn physical(
    root: &Path,
    parent: &Path,
    key: &str,
    claim: Option<RequestId>,
) -> Result<(Option<Identity>, u64), Error> {
    let Some(dir) = directory(root, parent)? else {
        return Ok((None, 0));
    };
    if let Some(id) = claim
        && dir.retired(id)?
    {
        return Ok((None, dir.inspect(&format!(".gc-{id}"))?.1));
    }
    match dir.inspect(key) {
        Ok((id, bytes)) => Ok((Some(id), bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((None, 0)),
        Err(e) => Err(e.into()),
    }
}
fn items(root: &Path, inventory: &Inventory, now: i64) -> Result<Vec<Item>, Error> {
    if !inventory.legacy_claims.is_empty() {
        return Err(Error::Context);
    }
    let extra = extra_sources(root)?;
    let mut output = vec![];
    let mut objects = BTreeSet::new();
    if let Some(dir) = directory(root, Path::new("objects"))? {
        for prefix in dir.entries()? {
            if output.len() + objects.len() > 2000 {
                return Err(Error::Context);
            }
            if prefix.len() != 2
                || !prefix
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                output.push(Item {
                    kind: "unknown",
                    key: prefix,
                    parent: "objects".into(),
                    action: "retain",
                    reason: "Unknown object namespace; no automatic deletion".into(),
                    bytes: 0,
                    identity: None,
                    claim: None,
                });
                continue;
            }
            if let Some(dir) = directory(root, &Path::new("objects").join(&prefix))? {
                for key in dir.entries()? {
                    if output.len() + objects.len() > 2000 {
                        return Err(Error::Context);
                    }
                    if key.len() == 64
                        && key.starts_with(&prefix)
                        && ContentDigest::from_hex(DigestKind::Artifact, &key).is_ok()
                    {
                        objects.insert(key);
                    } else if !key.starts_with(".gc-") {
                        output.push(Item {
                            kind: "unknown",
                            key,
                            parent: Path::new("objects").join(&prefix),
                            action: "retain",
                            reason: "Unknown object entry; no automatic deletion".into(),
                            bytes: 0,
                            identity: None,
                            claim: None,
                        });
                    }
                }
            }
        }
    }
    objects.extend(inventory.artifacts.keys().cloned());
    for key in objects {
        if output.len() >= 2000 {
            return Err(Error::Context);
        }
        let parent = Path::new("objects").join(&key[..2]);
        let (claim, grace) = item_state(inventory, "artifact", &key, now);
        let (identity, bytes) = physical(root, &parent, &key, claim)?;
        let rooted = inventory.roots.artifacts.contains(&key);
        let state = inventory.artifacts.get(&key).map(|(_, s, _)| s.as_str());
        let (action, reason) = if claim.is_some() {
            if rooted {
                return Err(Error::Context);
            }
            ("recover", "Resume the exact durable collection intent")
        } else if rooted {
            (
                "retain",
                "Live head, retained version/transitive input, pin, reader, active build or pending schedule evidence",
            )
        } else if identity.is_none() {
            (
                "retain",
                if state == Some("EXPIRED") {
                    "Data expired; immutable metadata retained"
                } else {
                    "Payload missing; no unverified deletion"
                },
            )
        } else if state == Some("EXPIRED") {
            (
                "observe",
                "Reappeared bytes require a new verified observation and grace period",
            )
        } else if state.is_none()
            || inventory
                .artifacts
                .get(&key)
                .is_some_and(|(_, _, published)| !published && !grace)
            || !grace
                && inventory
                    .items
                    .contains_key(&("artifact".into(), key.clone()))
        {
            (
                "observe",
                "Digest verification and durable first-seen grace required; filesystem age is not authority",
            )
        } else {
            (
                "collect",
                "Outside retained reachability; claim rechecks all current roots",
            )
        };
        let reason = if rooted {
            inventory.reasons.get(&key).cloned().unwrap_or_else(|| {
                "Retained local input dependency, active build or pending schedule evidence".into()
            })
        } else {
            reason.into()
        };
        output.push(Item {
            kind: "artifact",
            key,
            parent,
            action,
            reason,
            bytes,
            identity,
            claim,
        });
    }
    if let Some(dir) = directory(root, Path::new("source-snapshots"))? {
        let mut sources: BTreeSet<_> = dir
            .entries()?
            .into_iter()
            .filter(|s| !s.starts_with(".gc-"))
            .collect();
        sources.extend(
            inventory
                .items
                .keys()
                .filter(|(kind, _)| kind == "source")
                .map(|(_, key)| key.clone()),
        );
        for key in sources {
            if output.len() >= 2000 {
                return Err(Error::Context);
            }
            let Ok(id) = key.parse::<SourceSnapshotId>() else {
                output.push(Item {
                    kind: "unknown",
                    key,
                    parent: "source-snapshots".into(),
                    action: "retain",
                    reason: "Unknown source entry; no automatic deletion".into(),
                    bytes: 0,
                    identity: None,
                    claim: None,
                });
                continue;
            };
            let (claim, grace) = item_state(inventory, "source", &key, now);
            let (identity, bytes) = physical(root, Path::new("source-snapshots"), &key, claim)?;
            let rooted = inventory.roots.sources.contains(&id) || extra.contains(&id);
            let (action, reason) = if claim.is_some() {
                if rooted {
                    return Err(Error::Context);
                }
                (
                    "recover",
                    "Resume the exact durable source collection intent",
                )
            } else if rooted {
                (
                    "retain",
                    "Retained version, live operation/pin/schedule or last valid descriptor graph",
                )
            } else if identity.is_none() {
                ("retain", "Source payload already absent; metadata remains")
            } else if grace {
                (
                    "collect",
                    "Verified source outside reachability after durable seven-day grace",
                )
            } else {
                (
                    "observe",
                    "Strict capture verification and durable first-seen grace required",
                )
            };
            output.push(Item {
                kind: "source",
                key,
                parent: "source-snapshots".into(),
                action,
                reason: reason.into(),
                bytes,
                identity,
                claim,
            });
        }
    }
    if let Some(dir) = directory(root, Path::new("attempts"))? {
        let mut logs: BTreeSet<_> = dir
            .entries()?
            .into_iter()
            .filter(|s| !s.starts_with(".gc-"))
            .collect();
        logs.extend(
            inventory
                .items
                .keys()
                .filter(|(kind, _)| kind == "logs")
                .map(|(_, key)| key.clone()),
        );
        for key in logs {
            if output.len() >= 2000 {
                return Err(Error::Context);
            }
            let (claim, _) = item_state(inventory, "logs", &key, now);
            let (identity, bytes) = physical(root, Path::new("attempts"), &key, claim)?;
            let eligible = inventory.logs.contains(&key);
            let (action, reason) = if claim.is_some() {
                if !eligible {
                    return Err(Error::Context);
                }
                (
                    "recover",
                    "Resume the exact completed-attempt log collection intent",
                )
            } else if eligible && identity.is_some() {
                (
                    "collect",
                    "Completed attempt older than the independent 30-day log retention",
                )
            } else {
                (
                    "retain",
                    "Active, recent, missing or unrecognized attempt; no filesystem-age deletion",
                )
            };
            output.push(Item {
                kind: "logs",
                key,
                parent: "attempts".into(),
                action,
                reason: reason.into(),
                bytes,
                identity,
                claim,
            });
        }
    }
    if output.len() > 2000 {
        return Err(Error::Context);
    }
    Ok(output)
}
fn report(items: &[Item], applied: bool) -> Result<Report, Error> {
    let bytes = items
        .iter()
        .filter(|i| matches!(i.action, "collect" | "recover"))
        .try_fold(0u64, |n, i| n.checked_add(i.bytes))
        .ok_or(Error::Context)?;
    let deleted = items
        .iter()
        .filter(|i| i.action == "deleted")
        .try_fold(0u64, |n, i| n.checked_add(i.bytes))
        .ok_or(Error::Context)?;
    let redactor = tf_domain::diagnostic::Redactor::default();
    let projected=items.iter().map(|i|Ok(json!({"kind":i.kind,"id":redactor.text(&i.key).map_err(|_|Error::Context)?.as_str(),"action":i.action,"bytes":i.bytes.to_string(),"reason":i.reason}))).collect::<Result<Vec<_>,Error>>()?;
    let value = json!({"kind":"gc","applied":applied,"collectible_bytes":bytes.to_string(),"deleted_bytes":deleted.to_string(),"items":projected});
    if serde_json::to_vec(&value)
        .map_err(|_| Error::Context)?
        .len()
        > 512 * 1024
    {
        return Err(Error::Context);
    }
    Ok(Report { value })
}
pub(crate) fn execute(args: &clap::ArgMatches, explicit: Option<&String>) -> Result<Report, Error> {
    let apply = args.get_flag("apply");
    if apply && !args.get_flag("yes") {
        return Err(Error::Approval);
    }
    let workspace = Workspace::load(&std::env::current_dir()?, explicit.map(Path::new))?;
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
    let policy = RetentionPolicy::default();
    let db = workspace.root().join(".transflow/runtime/catalog.sqlite");
    if !apply {
        let inventory = runtime.block_on(async {
            let mut reader = tf_store::Reader::open_existing(&db).await?;
            let result = reader.gc_inventory(now, policy).await;
            reader.close().await?;
            result
        })?;
        return report(&items(workspace.root(), &inventory, now)?, false);
    }
    let mut owner = RuntimeOwner::acquire(
        workspace.root(),
        workspace.config().id(),
        CoordinatorMode::MetadataOnly,
    )?;
    runtime.block_on(async {
        let mut owned = owner.open_store().await?;
        let inventory = owned.repository()?.gc_inventory(now, policy).await?;
        let mut entries = items(workspace.root(), &inventory, now)?;
        report(&entries, false)?;
        for item in &mut entries {
            if !matches!(item.action, "observe" | "collect" | "recover") {
                continue;
            }
            let dir = directory(workspace.root(), &item.parent)?.ok_or(Error::Context)?;
            if item.action == "observe" {
                if item.kind == "artifact" {
                    let digest = ContentDigest::from_hex(DigestKind::Artifact, &item.key)
                        .map_err(|_| Error::Context)?;
                    let object = ArtifactStore::open(workspace.root())?.verify(digest)?;
                    owned.repository()?.observe_orphan(&object, now).await?;
                } else {
                    verify_source(workspace.root(), &item.key, workspace.config().id())?;
                    owned.repository()?.observe_source(&item.key, now).await?;
                }
                item.action = "observed";
                continue;
            }
            if item.identity.is_some() {
                if item.kind == "artifact" {
                    let digest = ContentDigest::from_hex(DigestKind::Artifact, &item.key)
                        .map_err(|_| Error::Context)?;
                    ArtifactStore::open(workspace.root())?.verify(digest)?;
                } else if item.kind == "source" {
                    let id = item.key.parse().map_err(|_| Error::Context)?;
                    if extra_sources(workspace.root())?.contains(&id) {
                        return Err(Error::Context);
                    }
                    verify_source(workspace.root(), &item.key, workspace.config().id())?;
                }
            }
            let id = if let Some(id) = item.claim {
                id
            } else {
                let id = tf_exec::discovery::random_id()?
                    .parse()
                    .map_err(|_| Error::Context)?;
                owned
                    .repository()?
                    .begin_gc(item.kind, &item.key, id, now, policy)
                    .await?;
                id
            };
            dir.remove(&item.key, id, item.identity, |_| {
                owned
                    .repository()
                    .map(|_| ())
                    .map_err(|_| std::io::Error::other("GC runtime owner identity changed"))
            })?;
            owned
                .repository()?
                .finish_gc(item.kind, &item.key, id, now)
                .await?;
            item.action = "deleted";
        }
        owned.close().await?;
        report(&entries, true)
    })
}
