//! Consumer replica installation. Origin metadata is durable before the provider pin is released.
use crate::{
    build_plan::{Error, failure},
    provider::{self, Request, Selection},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::mpsc, thread, time::Duration};
use tf_catalog::{RegistrySnapshot, Resolved, candidate::CandidateInput, workspace::Workspace};
use tf_domain::{
    BranchName, DatasetKey, VersionId,
    input::{BranchPolicySnapshot, InputBindingKey},
};
use tf_protocol::canonical::{ContentDigest, DigestKind};
use tf_store::{
    Store, artifacts::ArtifactStore, input_resolution::ReadRequest, planning::PlannedRead,
    retention::ReadTarget,
};

pub(crate) fn selection(
    registry: &RegistrySnapshot,
    input: &CandidateInput,
    key: &InputBindingKey,
    output: &BranchName,
) -> Result<(DatasetKey, Selection), Error> {
    let value = &input.declaration;
    let reference = if value["ref"]["form"] == "string" {
        &value["ref"]["value"]
    } else {
        &value["ref"]["path"]
    };
    let Resolved::External(registration) = registry
        .resolve(
            reference
                .as_str()
                .ok_or_else(|| failure("missing external reference"))?,
        )
        .map_err(failure)?
    else {
        return Err(failure("input is not an explicit external registration"));
    };
    let s = Selection {
        consumer_workspace: key.consumer().workspace_id().to_string(),
        consumer_dataset: key.consumer().dataset_id().to_string(),
        alias: key.alias().into(),
        dataset: registration.key().dataset_id().to_string(),
        output: output.as_str().into(),
        registered: registration.default_branch().as_str().into(),
        declared: value["branch"].clone(),
        stop: value["stop_branch_fallback"]
            .as_bool()
            .ok_or_else(|| failure("missing fallback permission"))?,
        role: value["role"]
            .as_str()
            .ok_or_else(|| failure("missing input role"))?
            .into(),
        fallback: registration
            .fallback_override()
            .map(|v| v.iter().map(ToString::to_string).collect()),
    };
    Ok((registration.key(), s))
}
struct CopyLease {
    stop: mpsc::SyncSender<()>,
    thread: Option<thread::JoinHandle<Result<i64, Error>>>,
}
impl CopyLease {
    fn start(
        root: PathBuf,
        workspace: tf_domain::WorkspaceId,
        lease: String,
        operation: String,
    ) -> Self {
        let (stop, rx) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            let mut fence = 0;
            while matches!(
                rx.recv_timeout(Duration::from_secs(30)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                let v = provider::call(
                    &root,
                    workspace,
                    Request::Renew {
                        lease: lease.clone(),
                        operation: operation.clone(),
                        fence,
                    },
                )?;
                fence = v["fence"]
                    .as_i64()
                    .ok_or_else(|| failure("invalid copy lease renewal"))?;
            }
            Ok(fence)
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
    fn finish(mut self) -> Result<i64, Error> {
        let _ = self.stop.try_send(());
        self.thread
            .take()
            .ok_or_else(|| failure("copy lease missing"))?
            .join()
            .map_err(|_| failure("copy lease renewal stopped"))?
    }
}
impl Drop for CopyLease {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn digest(metadata: &Value) -> Result<ContentDigest, Error> {
    ContentDigest::from_hex(
        DigestKind::Artifact,
        metadata["artifact"]
            .as_str()
            .ok_or_else(|| failure("missing foreign artifact"))?,
    )
    .map_err(failure)
}
fn offline(
    selection: &Selection,
    key: DatasetKey,
    version: VersionId,
    metadata: &Value,
) -> Result<(Value, Value), Error> {
    let binding = selection.binding(
        key.workspace_id(),
        &BranchPolicySnapshot::new(vec![], BTreeMap::new()).map_err(failure)?,
    )?;
    let p = json!({"alias":selection.alias,"workspace":key.workspace_id().to_string(),"dataset":key.dataset_id().to_string(),"version":version.to_string(),"artifact":metadata["artifact"],"declared_branch":selection.declared,"starting_branch":binding.policy.start().as_str(),"resolved_branch":metadata["origin_branch"]["name"],"role":selection.role,"resolution":{"kind":"exact_pin","selector_override":true,"output_branch":selection.output,"policy_origin":"retained_exact","stop_branch_fallback":selection.stop,"candidates":[],"attempted":[],"fallback_index":null,"branch_id":metadata["origin_branch"]["id"],"provider_freshness":"not_resolved"}});
    let semantic = json!({"consumer_workspace":selection.consumer_workspace,"consumer_dataset":selection.consumer_dataset,"alias":selection.alias,"origin_workspace":key.workspace_id().to_string(),"origin_dataset":key.dataset_id().to_string(),"role":selection.role,"declared":selection.declared,"starting_branch":p["starting_branch"],"resolved_branch":p["resolved_branch"],"version":version.to_string(),"artifact":metadata["artifact"]});
    Ok((p, semantic))
}
/// Resolve only the selected foreign input. A fresh request always contacts the provider.
pub(crate) async fn prepare(
    store: &mut Store,
    workspace: &Workspace,
    origin: DatasetKey,
    selection: Selection,
    pin: Option<VersionId>,
    mut read: ReadRequest,
) -> Result<PlannedRead, Error> {
    let key = InputBindingKey::new(
        DatasetKey::new(
            selection.consumer_workspace.parse().map_err(failure)?,
            selection.consumer_dataset.parse().map_err(failure)?,
        ),
        selection.alias.clone(),
    )
    .map_err(failure)?;
    let local = ArtifactStore::open(workspace.root()).map_err(failure)?;
    let retained = if let Some(version) = pin {
        store
            .replica_metadata(origin, version)
            .await
            .map_err(failure)?
    } else {
        None
    };
    let (version, provenance, semantic, artifact) = if let (Some(version), Some(metadata)) =
        (pin, retained)
    {
        let artifact = digest(&metadata)?;
        // Pin before byte verification so collection cannot race an offline reader.
        store
            .acquire_read(
                read.lease,
                read.operation,
                read.kind,
                ReadTarget::Artifact(artifact),
                crate::preparation::now().map_err(failure)?,
                read.ttl_us,
            )
            .await
            .map_err(failure)?;
        local.verify(artifact).map_err(failure)?;
        let (p, s) = offline(&selection, origin, version, &metadata)?;
        (version, p, s, artifact)
    } else {
        let root=workspace.local().provider(origin.workspace_id()).ok_or_else(||failure("Provider locator is missing. Restore its explicit external registration locator, or use an already retained exact pin/replay"))?;
        let root = if root.is_absolute() {
            root.to_path_buf()
        } else {
            workspace.root().join(root)
        };
        let lease = crate::build_plan::id::<tf_domain::RequestId>()?.to_string();
        let operation = read.operation.to_string();
        let response = provider::call(
            &root,
            origin.workspace_id(),
            Request::Resolve {
                binding: Box::new(selection.clone()),
                pin: pin.map(|v| v.to_string()),
                lease: lease.clone(),
                operation: operation.clone(),
            },
        )?;
        let renewing = CopyLease::start(
            root.clone(),
            origin.workspace_id(),
            lease.clone(),
            operation.clone(),
        );
        let metadata = &response["metadata"];
        let version: VersionId = metadata["version"]
            .as_str()
            .ok_or_else(|| failure("provider version missing"))?
            .parse()
            .map_err(failure)?;
        if response["protocol"] != 1
            || response["workspace"] != origin.workspace_id().to_string()
            || pin.is_some_and(|p| p != version)
        {
            return Err(failure(
                "Provider protocol, identity or exact version mismatch",
            ));
        }
        store.begin_replica(origin,version,metadata).await.map_err(|_|failure("Provider origin metadata conflicts with retained identity, or exceeds supported bounds"))?;
        let artifact = digest(metadata)?;
        let source = ArtifactStore::open(&root).map_err(failure)?;
        let verified = local
            .copy_from(&source, artifact, crate::build_plan::id()?)
            .map_err(failure)?;
        if verified.manifest() != &metadata["manifest"] {
            return Err(failure(
                "Provider manifest differs from the verified copied bytes",
            ));
        }
        let fence = renewing.finish()?;
        // Confirm a live generation after all hashing/copying, before committing local visibility.
        let renewed = provider::call(
            &root,
            origin.workspace_id(),
            Request::Renew {
                lease: lease.clone(),
                operation: operation.clone(),
                fence,
            },
        )?;
        let fence = renewed["fence"]
            .as_i64()
            .ok_or_else(|| failure("copy renewal missing fence"))?;
        read.now_us = crate::preparation::now().map_err(failure)?;
        store
            .finish_replica(
                origin,
                version,
                &verified,
                ReadRequest {
                    lease: read.lease,
                    operation: read.operation,
                    kind: read.kind,
                    now_us: read.now_us,
                    ttl_us: read.ttl_us,
                },
            )
            .await
            .map_err(failure)?;
        provider::call(
            &root,
            origin.workspace_id(),
            Request::Release {
                lease,
                operation,
                fence,
            },
        )?;
        let mut p = response["provenance"].clone();
        p["resolution"]["provider_freshness"] = json!(if pin.is_some() {
            "not_resolved"
        } else {
            "resolved_at_planning"
        });
        (version, p, response["semantic"].clone(), artifact)
    };
    Ok(PlannedRead {
        consumer: key.consumer().dataset_id().to_string(),
        alias: key.alias().into(),
        dataset: origin.dataset_id().to_string(),
        version: version.to_string(),
        artifact: artifact.hex(),
        lease: read.lease.to_string(),
        semantic,
        provenance,
    })
}
