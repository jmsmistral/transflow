//! Provider-owned input bytes. Only immutable origin metadata is stored locally.
use crate::{
    build_plan::{Error, failure},
    provider::{self, Request, Selection},
};
use serde_json::{Value, json};
use std::path::PathBuf;
use tf_catalog::{RegistrySnapshot, Resolved, candidate::CandidateInput, workspace::Workspace};
use tf_domain::{BranchName, DatasetKey, VersionId, WorkspaceId, input::InputBindingKey};
use tf_protocol::canonical::{ContentDigest, DigestKind};
use tf_store::{Store, input_resolution::ReadRequest, planning::PlannedRead};
mod lease;
pub(crate) use lease::Reads;
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

fn digest(metadata: &Value) -> Result<ContentDigest, Error> {
    ContentDigest::from_hex(
        DigestKind::Artifact,
        metadata["artifact"]
            .as_str()
            .ok_or_else(|| failure("Missing provider artifact"))?,
    )
    .map_err(failure)
}
fn locator(workspace: &Workspace, origin: WorkspaceId) -> Result<PathBuf, Error> {
    let root = workspace.local().provider(origin).ok_or_else(|| failure("Provider locator is missing; restore the external workspace locator. Direct reads and replay require the provider's exact version"))?;
    Ok(if root.is_absolute() {
        root.to_path_buf()
    } else {
        workspace.root().join(root)
    })
}
fn validate(
    response: &Value,
    origin: DatasetKey,
    pin: Option<VersionId>,
) -> Result<VersionId, Error> {
    let m = &response["metadata"];
    let version: VersionId = m["version"]
        .as_str()
        .ok_or_else(|| failure("Missing provider version"))?
        .parse()
        .map_err(failure)?;
    if response["protocol"] != 2
        || response["workspace"] != origin.workspace_id().to_string()
        || m["workspace"] != origin.workspace_id().to_string()
        || m["dataset"] != origin.dataset_id().to_string()
        || pin.is_some_and(|p| p != version)
    {
        return Err(failure(
            "Provider direct-read protocol, identity or exact version mismatch",
        ));
    }
    tf_protocol::validate_document("ArtifactManifestV1", &m["manifest"]).map_err(failure)?;
    if tf_protocol::canonical::artifact_digest(&m["manifest"]).map_err(failure)? != digest(m)? {
        return Err(failure("Provider manifest digest mismatch"));
    }
    Ok(version)
}
/// Planning validates metadata, not a full data scan. The provider pin expires with the draft.
pub(crate) async fn prepare(
    store: &mut Store,
    workspace: &Workspace,
    origin: DatasetKey,
    selection: Selection,
    pin: Option<VersionId>,
    read: ReadRequest,
) -> Result<PlannedRead, Error> {
    let root = locator(workspace, origin.workspace_id())?;
    let (ticket, response) = lease::Ticket::resolve(
        root,
        origin.workspace_id(),
        selection,
        pin,
        read.lease.to_string(),
        read.operation.to_string(),
    )?;
    let version = validate(&response, origin, pin)?;
    let metadata = &response["metadata"];
    store
        .record_foreign_metadata(origin, version, metadata)
        .await
        .map_err(failure)?;
    let mut provenance = response["provenance"].clone();
    provenance["resolution"]["provider_freshness"] = json!(if pin.is_some() {
        "not_resolved"
    } else {
        "resolved_at_planning"
    });
    provenance["resolution"]["storage"] = json!("provider_direct");
    let semantic = response["semantic"].clone();
    let planned = PlannedRead {
        consumer: semantic["consumer_dataset"]
            .as_str()
            .ok_or_else(|| failure("Missing consumer"))?
            .into(),
        alias: semantic["alias"]
            .as_str()
            .ok_or_else(|| failure("Missing alias"))?
            .into(),
        dataset: origin.dataset_id().to_string(),
        version: version.to_string(),
        artifact: digest(metadata)?.hex(),
        lease: read.lease.to_string(),
        semantic,
        provenance,
    };
    ticket.retain_for_draft();
    Ok(planned)
}
/// Reconstruct only the exact recorded selection; current heads never enter acceptance/replay.
fn exact_selection(read: &PlannedRead) -> Result<Selection, Error> {
    let text = |v: &Value, key: &str| {
        v[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| failure("Incomplete external input identity"))
    };
    Ok(Selection {
        consumer_workspace: text(&read.semantic, "consumer_workspace")?,
        consumer_dataset: read.consumer.clone(),
        alias: read.alias.clone(),
        dataset: read.dataset.clone(),
        output: text(&read.provenance, "starting_branch")?,
        registered: text(&read.provenance, "starting_branch")?,
        declared: read.provenance["declared_branch"].clone(),
        stop: true,
        role: text(&read.provenance, "role")?,
        fallback: Some(vec![]),
    })
}
/// Resolve an exact historical version and keep the provider lease while verifying/using it.
fn acquire(
    workspace: &Workspace,
    read: &PlannedRead,
    operation: &str,
) -> Result<(lease::Ticket, Value), Error> {
    let origin = DatasetKey::new(
        read.provenance["workspace"]
            .as_str()
            .ok_or_else(|| failure("Missing provider identity"))?
            .parse()
            .map_err(failure)?,
        read.dataset.parse().map_err(failure)?,
    );
    let root = locator(workspace, origin.workspace_id())?;
    let version = read.version.parse().map_err(failure)?;
    let (ticket, response) = lease::Ticket::resolve(
        root,
        origin.workspace_id(),
        exact_selection(read)?,
        Some(version),
        crate::build_plan::id::<tf_domain::RequestId>()?.to_string(),
        operation.into(),
    )?;
    validate(&response, origin, Some(version))?;
    if digest(&response["metadata"])?.hex() != read.artifact {
        return Err(failure("Exact provider input changed"));
    }
    Ok((ticket, response["metadata"].clone()))
}
/// Release an abandoned/replaced draft pin. Failed release leaves only a bounded expiring pin.
pub(crate) fn release_draft(workspace: &Workspace, read: &PlannedRead, operation: &str) {
    if read.provenance["workspace"] == workspace.config().id().to_string() {
        return;
    }
    if let Some(origin) = read.provenance["workspace"]
        .as_str()
        .and_then(|s| s.parse().ok())
        && let Ok(root) = locator(workspace, origin)
    {
        let _ = provider::call(
            &root,
            origin,
            Request::Release {
                lease: read.lease.clone(),
                operation: operation.into(),
                fence: 0,
            },
        );
    }
}

/// Release all successfully acquired draft pins if later preparation fails.
pub(crate) struct DraftPins<'a> {
    workspace: &'a Workspace,
    operation: String,
    reads: Vec<PlannedRead>,
    retained: bool,
}
impl<'a> DraftPins<'a> {
    pub(crate) fn new(workspace: &'a Workspace, operation: &str) -> Self {
        Self {
            workspace,
            operation: operation.into(),
            reads: vec![],
            retained: false,
        }
    }
    pub(crate) fn add(&mut self, read: &PlannedRead) {
        self.reads.push(read.clone());
    }
    pub(crate) fn retain(mut self) {
        self.retained = true;
    }
}
impl Drop for DraftPins<'_> {
    fn drop(&mut self) {
        if !self.retained {
            for read in &self.reads {
                release_draft(self.workspace, read, &self.operation);
            }
        }
    }
}
/// Replay obtains new draft protection for the exact original version, without changing provenance.
pub(crate) async fn pin_replay(
    store: &mut Store,
    workspace: &Workspace,
    read: &mut PlannedRead,
    operation: &str,
) -> Result<(), Error> {
    let (ticket, metadata) = acquire(workspace, read, operation)?;
    let origin = DatasetKey::new(
        read.provenance["workspace"]
            .as_str()
            .ok_or_else(|| failure("Missing origin"))?
            .parse()
            .map_err(failure)?,
        read.dataset.parse().map_err(failure)?,
    );
    store
        .record_foreign_metadata(origin, read.version.parse().map_err(failure)?, &metadata)
        .await
        .map_err(failure)?;
    read.lease = ticket.id().into();
    ticket.retain_for_draft();
    Ok(())
}
