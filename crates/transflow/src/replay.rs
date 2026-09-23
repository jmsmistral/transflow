//! New execution from retained source, parameters, scope and exact boundary pins.
use crate::build_plan::{self, Error, failure};
use serde_json::json;
use std::path::Path;
use tf_domain::{BranchName, BuildId, RequestId, WorkspaceId};
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
fn id() -> Result<RequestId, Error> {
    tf_exec::discovery::random_id()
        .map_err(failure)?
        .parse()
        .map_err(failure)
}
pub(crate) async fn prepare(
    mut owner: RuntimeOwner,
    original: BuildId,
    destination: BranchName,
) -> Result<build_plan::Completion<build_plan::Accepted>, Error> {
    crate::recovery::recover(&mut owner).await?;
    let workspace = owner.workspace_id().map_err(failure)?;
    let now = crate::preparation::now().map_err(failure)?;
    let mut owned = owner.open_store().await.map_err(failure)?;
    let store = owned.repository().map_err(failure)?;
    let mut plan = store.original_plan(original).await.map_err(failure)?;
    plan.id = id()?.to_string();
    plan.created_us = now;
    plan.expires_us = now
        .checked_add(900_000_000)
        .ok_or_else(|| failure("replay clock overflow"))?;
    plan.context["replay_of"] = json!(original.to_string());
    plan.context["force"] = json!(true);
    plan.output = store
        .plan_guard(workspace, destination.as_str(), None)
        .await
        .map_err(failure)?;
    plan.guards.clear();
    for w in &mut plan.writes {
        w.job = id()?.to_string();
        let g = store
            .plan_guard(
                workspace,
                destination.as_str(),
                Some(w.dataset.parse().map_err(failure)?),
            )
            .await
            .map_err(failure)?;
        w.generation = g.generation;
        plan.guards.push(g);
    }
    // Reacquire original boundaries only: later heads never enter the replay plan.
    let mut leases = vec![];
    let prepared = async {
        for r in &mut plan.reads {
            let lease_id = id()?;
            let lease = store
                .acquire_read(
                    lease_id,
                    plan.id.parse().map_err(failure)?,
                    tf_store::retention::LeaseKind::Plan,
                    tf_store::retention::ReadTarget::Version(r.version.parse().map_err(failure)?),
                    now,
                    900_000_000,
                )
                .await
                .map_err(failure)?;
            let artifact = lease.artifact();
            leases.push(lease);
            if artifact.hex() != r.artifact {
                return Err(failure("retained replay input identity changed"));
            }
            r.lease = lease_id.to_string();
        }
        store.save_draft(&plan).await.map_err(failure)?;
        Ok::<_, Error>(())
    }
    .await;
    if prepared.is_err() {
        for lease in &leases {
            store.release_read(lease).await.map_err(failure)?;
        }
    }
    owned.close().await.map_err(failure)?;
    prepared?;
    build_plan::accept(owner, plan.id.parse().map_err(failure)?).await
}
pub(crate) fn execute(
    root: &Path,
    workspace: WorkspaceId,
    original: BuildId,
    destination: BranchName,
    json_mode: bool,
) -> Result<crate::build_cli::Report, Error> {
    if tf_exec::ownership::discover(root, workspace)
        .map_err(failure)?
        .is_some()
    {
        return crate::build_transport::replay(root, workspace, original, &destination, json_mode);
    }
    let owner =
        RuntimeOwner::acquire(root, workspace, CoordinatorMode::Temporary).map_err(failure)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    let c = rt.block_on(prepare(owner, original, destination))?;
    let accepted = c.result?;
    let c = rt.block_on(crate::build_cli::execute_build(
        c.owner,
        accepted.build,
        json_mode,
    ))?;
    c.result.map_err(failure)?;
    Ok(crate::build_cli::report(
        "replay",
        rt.block_on(crate::build_cli::snapshot(root, workspace, accepted.build))?,
        true,
    ))
}
