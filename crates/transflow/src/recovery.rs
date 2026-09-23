//! Startup recovery under the real runtime lock, before accepting any new execution.
use crate::build_plan::{Error, failure};
use tf_exec::ownership::RuntimeOwner;

/// Fence previous sessions before touching orphan workers; preserve committed winners,
/// untouched queues, captured source and diagnostics. Never guess a retry for interrupted code.
pub async fn recover(owner: &mut RuntimeOwner) -> Result<(), Error> {
    owner.validate_paths().map_err(failure)?;
    let root = owner.workspace_root().to_owned();
    let workspace = owner.workspace_id().map_err(failure)?;
    let session = owner.registration().session().map_err(failure)?;
    let now = crate::preparation::now().map_err(failure)?;
    let mut store = owner.open_store().await.map_err(failure)?;
    store
        .repository()
        .map_err(failure)?
        .register_workspace(
            workspace,
            root.to_str()
                .ok_or_else(|| failure("invalid workspace path"))?,
            now,
        )
        .await
        .map_err(failure)?;
    store
        .repository()
        .map_err(failure)?
        .recover_publications(workspace, session, now)
        .await
        .map_err(failure)?;
    store.close().await.map_err(failure)?;
    tokio::task::spawn_blocking(move || cleanup_workers(&root))
        .await
        .map_err(failure)??;
    if crate::reconcile::recover(owner, now)
        .await
        .map_err(failure)?
        .iter()
        .any(|r| matches!(r, crate::reconcile::RecoveryOutcome::Conflict(_)))
    {
        return Err(failure(
            "Registry recovery conflicts with current authoring files; inspect the retained journal",
        ));
    }
    Ok(())
}

fn cleanup_workers(root: &std::path::Path) -> Result<(), Error> {
    // Records are written and synced before the supervisor authorizes user imports.
    let attempts = root.join(".transflow/runtime/attempts");
    if attempts.try_exists().map_err(failure)? {
        if std::fs::canonicalize(&attempts).map_err(failure)? != attempts {
            return Err(failure("unsafe attempt directory"));
        }
        for item in std::fs::read_dir(&attempts).map_err(failure)? {
            let item = item.map_err(failure)?;
            let attempt = item
                .file_name()
                .to_str()
                .ok_or_else(|| failure("invalid attempt directory"))?
                .parse::<tf_domain::AttemptId>()
                .map_err(failure)?;
            if !item.file_type().map_err(failure)?.is_dir() {
                return Err(failure("unsafe attempt directory"));
            }
            for helper in std::fs::read_dir(item.path()).map_err(failure)? {
                let helper = helper.map_err(failure)?;
                if helper.file_type().map_err(failure)?.is_dir() {
                    tf_exec::recovery::stop(&helper.path(), attempt).map_err(failure)?;
                } else {
                    return Err(failure("unsafe helper directory"));
                }
            }
        }
    }
    Ok(())
}
