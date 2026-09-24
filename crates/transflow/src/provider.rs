//! Metadata-only provider service. All mutations run in the existing runtime owner's loop.
use crate::build_plan::{Error, failure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, SyncSender},
    },
};
use tf_catalog::workspace::Workspace;
use tf_domain::{
    BranchName, BranchSelector, DatasetKey, FallbackPermission, VersionId, WorkspaceId,
    input::{InputBinding, InputBindingKey, InputRole},
};
use tf_exec::ownership::{self, CoordinatorMode, RuntimeOwner};
use tf_store::input_resolution::ReadRequest;
use tf_store::retention::LeaseKind;
/// Private versioned service requests reject extra fields, especially execution instructions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    /// Resolve and lease one exact origin. Selection is provider-owned.
    Resolve {
        binding: Box<Selection>,
        pin: Option<String>,
        lease: String,
        operation: String,
    },
    /// Renew one copy lease generation.
    Renew {
        lease: String,
        operation: String,
        fence: i64,
    },
    /// Release one copy lease generation.
    Release {
        lease: String,
        operation: String,
        fence: i64,
    },
    /// Read retained provenance only; does not copy any ancestor data.
    Provenance { dataset: String, version: String },
}
/// Captured consumer selector; fallback is a registration override, never a build override.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    /// Qualified consumer identity.
    pub consumer_workspace: String,
    /// Consumer output UUID.
    pub consumer_dataset: String,
    /// Local binding alias.
    pub alias: String,
    /// Origin dataset UUID in the requested provider workspace.
    pub dataset: String,
    /// Consumer output branch (CURRENT maps this into the provider).
    pub output: String,
    /// Default branch captured by the explicit registration.
    pub registered: String,
    /// Original closed branch selector.
    pub declared: Value,
    /// Strict binding wins over every fallback.
    pub stop: bool,
    /// Data or validation-only binding.
    pub role: String,
    /// Explicit registration tail, including an empty tail.
    pub fallback: Option<Vec<String>>,
}
impl Selection {
    pub(crate) fn binding(
        &self,
        workspace: WorkspaceId,
        policies: &tf_domain::input::BranchPolicySnapshot,
    ) -> Result<InputBinding, Error> {
        let declared = match self.declared["kind"].as_str() {
            Some("omitted") => BranchSelector::Omitted,
            Some("current") => BranchSelector::Current,
            Some("named") => BranchSelector::Named(
                self.declared["name"]
                    .as_str()
                    .ok_or_else(|| failure("missing provider branch"))?
                    .parse()
                    .map_err(failure)?,
            ),
            _ => return Err(failure("invalid provider branch selector")),
        };
        let fallback = self
            .fallback
            .as_ref()
            .map(|v| {
                v.iter()
                    .map(|n| n.parse::<BranchName>().map_err(failure))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let role = match self.role.as_str() {
            "data" => InputRole::Data,
            "validation" => InputRole::Validation,
            _ => return Err(failure("invalid provider input role")),
        };
        Ok(InputBinding {
            key: InputBindingKey::new(
                DatasetKey::new(
                    self.consumer_workspace.parse().map_err(failure)?,
                    self.consumer_dataset.parse().map_err(failure)?,
                ),
                self.alias.clone(),
            )
            .map_err(failure)?,
            dataset: DatasetKey::new(workspace, self.dataset.parse().map_err(failure)?),
            role,
            policy: policies
                .external(
                    &self.output.parse().map_err(failure)?,
                    &self.registered.parse().map_err(failure)?,
                    declared,
                    if self.stop {
                        FallbackPermission::Prohibited
                    } else {
                        FallbackPermission::Allowed
                    },
                    fallback.as_deref(),
                )
                .map_err(failure)?,
        })
    }
}
/// An authenticated request queued to the coordinator, with bounded acknowledgement.
pub struct Command {
    pub(crate) request: Request,
    pub(crate) reply: SyncSender<Result<Value, String>>,
}
/// Bounded control mailbox shared by the listener and owning coordinator.
pub type Mailbox = Arc<Mutex<Receiver<Command>>>;
pub(crate) fn drain(
    owner: &mut RuntimeOwner,
    rt: &tokio::runtime::Runtime,
    mailbox: &Mailbox,
) -> Result<(), Error> {
    // Never retain the channel lock across service work.
    for _ in 0..16 {
        let command = mailbox.lock().map_err(failure)?.try_recv();
        let Ok(command) = command else { break };
        let result = rt
            .block_on(service(owner, command.request))
            .map_err(|e| e.to_string());
        let _ = command.reply.try_send(result);
    }
    Ok(())
}
pub(crate) async fn service(owner: &mut RuntimeOwner, request: Request) -> Result<Value, Error> {
    owner.validate_paths().map_err(failure)?;
    let workspace =
        Workspace::load(owner.workspace_root(), Some(owner.workspace_root())).map_err(failure)?;
    let identity = owner.workspace_id().map_err(failure)?;
    if workspace.config().id() != identity {
        return Err(failure(
            "Provider workspace UUID changed; repair the explicit provider locator",
        ));
    }
    if let Request::Resolve {
        binding, pin: None, ..
    } = &request
    {
        let bytes = tf_catalog::registry_write::RegistryFile::open(workspace.root())
            .and_then(|f| f.read())
            .map_err(failure)?;
        let registry = tf_catalog::RegistrySnapshot::parse(
            identity,
            std::str::from_utf8(&bytes).map_err(failure)?,
        )
        .map_err(failure)?;
        registry.resolve_output(&format!("dataset:{}",binding.dataset)).map_err(|_|failure("Provider dataset is absent or tombstoned; update the explicit external registration or use a retained exact pin"))?;
    }
    let mut owned = owner.open_store().await.map_err(failure)?;
    let store = owned.repository().map_err(failure)?;
    let result = match request {
        Request::Resolve {
            binding,
            pin,
            lease,
            operation,
        } => {
            let read = store
                .resolve_export(
                    identity,
                    binding.binding(
                        identity,
                        &workspace.config().input_policies().map_err(failure)?,
                    )?,
                    pin.map(|v| v.parse::<VersionId>().map_err(failure))
                        .transpose()?,
                    ReadRequest {
                        lease: lease.parse().map_err(failure)?,
                        operation: operation.parse().map_err(failure)?,
                        kind: LeaseKind::Copy,
                        now_us: crate::preparation::now().map_err(failure)?,
                        ttl_us: 900_000_000,
                    },
                )
                .await
                .map_err(failure)?;
            let metadata = store
                .export_metadata(read.binding().dataset, read.version())
                .await
                .map_err(failure)?;
            Ok(
                json!({"protocol":1,"workspace":identity.to_string(),"metadata":metadata,"provenance":read.provenance().value(),"semantic":read.semantic_value(),"lease":lease,"operation":operation,"fence":0}),
            )
        }
        Request::Renew {
            lease,
            operation,
            fence,
        } => {
            let ticket = store
                .copy_lease(
                    lease.parse().map_err(failure)?,
                    operation.parse().map_err(failure)?,
                    fence,
                )
                .await
                .map_err(failure)?;
            store
                .renew_read(
                    &ticket,
                    crate::preparation::now().map_err(failure)?,
                    900_000_000,
                )
                .await
                .map_err(failure)?;
            Ok(
                json!({"fence":fence.checked_add(1).ok_or_else(||failure("copy lease fence exhausted"))?}),
            )
        }
        Request::Release {
            lease,
            operation,
            fence,
        } => {
            let ticket = store
                .copy_lease(
                    lease.parse().map_err(failure)?,
                    operation.parse().map_err(failure)?,
                    fence,
                )
                .await
                .map_err(failure)?;
            store.release_read(&ticket).await.map_err(failure)?;
            Ok(json!({"released":true}))
        }
        Request::Provenance { dataset, version } => store
            .export_metadata(
                DatasetKey::new(identity, dataset.parse().map_err(failure)?),
                version.parse().map_err(failure)?,
            )
            .await
            .map_err(failure),
    };
    owned.close().await.map_err(failure)?;
    result
}
/// A client creates a transient metadata-only owner only when the provider is unowned.
/// An existing live coordinator always handles its own database; a missing/busy endpoint fails.
pub(crate) fn call(root: &Path, workspace: WorkspaceId, request: Request) -> Result<Value, Error> {
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        let config=tf_catalog::workspace::WorkspaceConfig::parse(&tf_catalog::workspace::read_authoring(&root.join("workspace.toml")).map_err(failure)?).map_err(failure)?;
        if config.id()!=workspace {return Err(failure("Provider locator points to a different workspace UUID"));}
        if ownership::discover(&root,workspace).map_err(failure)?.is_some(){
            let reply=crate::build_transport::call(&root,workspace,json!({"operation":"provider","provider":request}),false)?;
            return Ok(reply["result"].clone());
        }
        let mut owner=RuntimeOwner::acquire(&root,workspace,CoordinatorMode::MetadataOnly).map_err(|_|failure("Provider is unavailable or busy without a metadata endpoint; start transflow serve or retry after its current operation"))?;
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(failure)?;
        rt.block_on(service(&mut owner,request))
    }).join().map_err(|_|failure("provider metadata service stopped"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_protocol_cannot_request_execution_or_schedules() {
        for request in [
            json!({"action":"build","target":"raw/items"}),
            json!({"action":"activate_schedule","schedule":"daily"}),
            json!({"action":"provenance","dataset":"d","version":"v","execute":true}),
            json!({"action":"renew","lease":"l","operation":"o","fence":0,"import":true}),
        ] {
            assert!(serde_json::from_value::<Request>(request).is_err());
        }
    }
}
