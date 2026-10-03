//! Persistent coordinator composition of bounded observation and guarded scheduled builds.
use crate::build_plan::{self, Error, failure};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tf_domain::{BranchName, WorkspaceId, input::BranchPolicySnapshot};
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
use tf_protocol::schedule::{BuildMode, Definition, FallbackPolicy, Source};
use tf_store::schedule_dispatch::Pending;

pub(crate) fn policy(value: &FallbackPolicy) -> Result<BranchPolicySnapshot, Error> {
    let names = |v: &[String]| {
        v.iter()
            .map(|s| s.parse::<BranchName>().map_err(failure))
            .collect::<Result<Vec<_>, _>>()
    };
    BranchPolicySnapshot::new(
        names(&value.default)?,
        value
            .rules
            .iter()
            .map(|(k, v)| Ok((k.parse().map_err(failure)?, names(v)?)))
            .collect::<Result<BTreeMap<_, _>, Error>>()?,
    )
    .map_err(failure)
}
/// One ID-keyset page per pass; no schedule is silently omitted when more pages remain.
pub(crate) async fn advance(
    owner: RuntimeOwner,
    after: String,
    busy: Arc<AtomicBool>,
) -> Result<(build_plan::Completion<()>, String), Error> {
    tokio::task::spawn_blocking(move || {
        let mut owner = owner;
        let mut next = after;
        let result = (|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(failure)?;
            rt.block_on(pass(&mut owner, &mut next, &busy))
        })();
        (build_plan::Completion { owner, result }, next)
    })
    .await
    .map_err(failure)
}
async fn pass(
    owner: &mut RuntimeOwner,
    after: &mut String,
    busy: &AtomicBool,
) -> Result<(), Error> {
    if owner.registration().mode() != CoordinatorMode::Persistent {
        return Err(failure(
            "Schedule activation requires the persistent coordinator",
        ));
    }
    let workspace = owner.workspace_id().map_err(failure)?;
    let now = crate::preparation::now().map_err(failure)?;
    let mut owned = owner.open_store().await.map_err(failure)?;
    let store = owned.repository().map_err(failure)?;
    store
        .reconcile_schedule_dispatches(workspace, now)
        .await
        .map_err(failure)?;
    let mut reader = store.reader().await.map_err(failure)?;
    let page = reader.schedules(workspace, after).await.map_err(failure)?;
    reader.close().await.map_err(failure)?;
    *after = page["next_cursor"].as_str().unwrap_or("").into();
    for row in page["schedules"]
        .as_array()
        .ok_or_else(|| failure("Missing schedule page"))?
    {
        if row["needs_review"] == true {
            continue;
        }
        let id = row["id"]
            .as_str()
            .ok_or_else(|| failure("Missing schedule identity"))?
            .parse()
            .map_err(failure)?;
        let epoch = row["trigger_epoch"]
            .as_str()
            .ok_or_else(|| failure("Missing schedule epoch"))?;
        let clock = store.schedule_clock(workspace, id).await.map_err(failure)?;
        // Clock resolution is bounded and performed outside a SQLite write transaction.
        let plan =
            match tf_schedule::prepare(&clock.leaves, &clock.evaluation, &tf_schedule::SystemClock)
            {
                Ok(plan) => plan,
                Err(_) => {
                    store
                        .fail_schedule_observation(workspace, id, &clock.etag, now)
                        .await
                        .map_err(failure)?;
                    continue;
                }
            };
        store
            .accept_schedule_clock(&clock, &plan)
            .await
            .map_err(failure)?;
        store
            .scan_schedule_events(workspace, id, epoch, 100, now)
            .await
            .map_err(failure)?;
        // One intended tick per acceptance keeps bounded catch-up occurrences independent.
        store
            .deliver_schedule_ticks(workspace, id, epoch, now)
            .await
            .map_err(failure)?;
        store
            .queue_schedule_occurrence(workspace, id, epoch, now)
            .await
            .map_err(failure)?;
    }
    let pending = store
        .next_schedule_dispatch(workspace, now)
        .await
        .map_err(failure)?;
    owned.close().await.map_err(failure)?;
    // A command queued during observation keeps priority. Empty observation does not report busy.
    if let Some(pending) = pending
        && busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        let result = async {
            if let Err(error) = dispatch(owner, workspace, &pending).await {
                let safe = tf_domain::diagnostic::Redactor::default()
                    .text(&error.to_string())
                    .map_err(failure)?;
                let mut owned = owner.open_store().await.map_err(failure)?;
                owned
                    .repository()
                    .map_err(failure)?
                    .fail_schedule_dispatch(
                        workspace,
                        &pending,
                        safe.as_str(),
                        crate::preparation::now().map_err(failure)?,
                    )
                    .await
                    .map_err(failure)?;
                owned.close().await.map_err(failure)?;
            }
            Ok::<_, Error>(())
        }
        .await;
        busy.store(false, Ordering::Release);
        result?;
    }
    Ok(())
}
async fn dispatch(
    owner: &mut RuntimeOwner,
    workspace: WorkspaceId,
    pending: &Pending,
) -> Result<(), Error> {
    let mut owned = owner.open_store().await.map_err(failure)?;
    let ancestry = owned
        .repository()
        .map_err(failure)?
        .check_schedule_dispatch_ancestry(pending)
        .await;
    owned.close().await.map_err(failure)?;
    ancestry.map_err(|_|failure("Automatic schedule dispatch was stopped by its causal ancestry or consecutive-build limit; inspect the trigger and use an explicit manual request"))?;
    let definition=Definition::decode(&json!({"format_version":1,"name":"Scheduled dispatch","description":"","trigger":{"kind":"manual"},"build":pending.execution["build"],"policies":pending.execution["policies"]})).map_err(failure)?;
    let build = definition.build;
    let registry_capture = &build.source;
    let refs = |v: &[tf_protocol::schedule::Dataset]| -> Result<Vec<String>, Error> {
        v.iter()
            .map(|d| {
                let key = d.key().map_err(failure)?;
                if key.workspace_id() != workspace {
                    return Err(failure(
                        "External scope identity requires an explicit registered alias",
                    ));
                }
                Ok(format!("dataset:{}", key.dataset_id()))
            })
            .collect()
    };
    // Resolve foreign explicit boundary identities through the selected registry later in preparation.
    let parameters: serde_json::Map<String, Value> = build
        .parameters
        .iter()
        .map(|(k, v)| (format!("dataset:{k}"), v.clone()))
        .collect();
    let request = build_plan::Request {
        python: None,
        branch: build.data_branch.parse().map_err(failure)?,
        mode: match build.build_mode {
            BuildMode::Full => tf_plan::scope::Mode::Full,
            BuildMode::Selected => tf_plan::scope::Mode::Selected,
            BuildMode::Between => tf_plan::scope::Mode::Between,
            BuildMode::Connecting => tf_plan::scope::Mode::Connecting,
        },
        targets: refs(&build.targets)?,
        boundaries: build
            .boundaries
            .iter()
            .map(|d| {
                let key = d.key().map_err(failure)?;
                Ok(if key.workspace_id() == workspace {
                    format!("dataset:{}", key.dataset_id())
                } else {
                    format!("external:{}/{}", key.workspace_id(), key.dataset_id())
                })
            })
            .collect::<Result<Vec<_>, Error>>()?,
        exclusions: refs(&build.exclusions)?,
        refresh_sources: refs(&build.refresh_sources)?,
        pins: vec![],
        fallbacks: Some(
            build
                .fallback_branches
                .iter()
                .map(|n| n.parse().map_err(failure))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        force: build.force,
        parameters: json!(parameters),
    };
    let options = build_plan::Options {
        git_ref: match registry_capture {
            Source::GitRef { reference } => Some(reference.clone()),
            _ => None,
        },
        require_current: build.require_current,
        timeout_seconds: Some(build.timeout_seconds.into()),
        validation_timeout_seconds: Some(build.validation_timeout_seconds.into()),
        scheduled: Some(pending.clone()),
        ..Default::default()
    };
    let plan = build_plan::prepare_inner(owner, request, options).await?;
    build_plan::accept_inner(owner, plan.id.parse().map_err(failure)?).await?;
    Ok(())
}
