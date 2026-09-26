//! Import copies use the same verified object installer and atomic head/event publication.
use crate::{
    build_plan::{Error, failure, id},
    preparation,
};
use serde_json::{Value, json};
use std::path::Path;
use tf_catalog::{capture::SourceSnapshot, local_files::FileSelection, workspace::Workspace};
use tf_domain::{BranchName, DatasetId, DatasetKey, DatasetPath, RequestId};
use tf_exec::{
    environment::{self, EnvironmentRequest, ManagedEnvironment},
    ownership::RuntimeOwner,
};
use tf_store::{
    artifacts::ArtifactStore, import_publication::ImportPublication, imports::PreparedFiles,
};

pub(crate) struct Context<'a> {
    pub request: RequestId,
    pub dataset: DatasetId,
    pub path: &'a DatasetPath,
    pub branch: &'a BranchName,
    pub registered: bool,
    pub selection: &'a FileSelection,
    pub environment: &'a ManagedEnvironment,
    pub python: &'a Path,
    pub environment_request: &'a EnvironmentRequest,
}
pub(crate) fn publish(
    owner: &mut RuntimeOwner,
    rt: &tokio::runtime::Runtime,
    workspace: &Workspace,
    capture: &SourceSnapshot,
    files: &PreparedFiles,
    c: Context<'_>,
) -> Result<Value, Error> {
    let session = owner.registration().session().map_err(failure)?;
    let head = rt.block_on(async {
        let mut owned = owner.open_store().await.map_err(failure)?;
        let repo = owned.repository().map_err(failure)?;
        repo.register_dataset(
            workspace.config().id(),
            c.dataset,
            preparation::now().map_err(failure)?,
        )
        .await
        .map_err(failure)?;
        let head = repo
            .plan_guard(workspace.config().id(), c.branch.as_str(), Some(c.dataset))
            .await
            .map_err(failure)?;
        owned.close().await.map_err(failure)?;
        Ok::<_, Error>(head)
    })?;
    let (directory, names) = files.publication_files().map_err(failure)?;
    let artifacts = ArtifactStore::open(workspace.root()).map_err(failure)?;
    let candidate = artifacts.prepare(&directory,&names,json!({"engine":"import","version":env!("CARGO_PKG_VERSION"),"compression":"preserved","row_group_size":"0"}),id()?, |_| Ok(())).map_err(failure)?;
    c.selection.verify_root().map_err(failure)?;
    files.verify_sources().map_err(failure)?;
    capture
        .verify_working_copy(workspace, false)
        .map_err(failure)?;
    let artifact = candidate.install(|_| Ok(())).map_err(failure)?;
    // Installation/verification may be long; never publish bytes or an authoring context
    // that changed after the earlier checks. An installed orphan is safe on refusal/crash.
    let artifact = artifacts.verify(artifact.digest()).map_err(failure)?;
    c.selection.verify_root().map_err(failure)?;
    files.verify_sources().map_err(failure)?;
    files.verify_copies().map_err(failure)?;
    capture
        .verify_working_copy(workspace, false)
        .map_err(failure)?;
    if environment::inspect(workspace.root(), c.python, c.environment_request).map_err(failure)?
        != *c.environment
    {
        return Err(failure("The import environment changed before publication"));
    }
    owner.validate_paths().map_err(failure)?;
    let request = ImportPublication {
        dataset: DatasetKey::new(workspace.config().id(), c.dataset),
        import: c.request,
        version: id()?,
        session,
        head,
        new_branch: id()?,
        source: capture.id().map_err(failure)?,
        source_digest: capture.digest().into(),
        manifest: json!({"files":capture.manifest_files(),"import_files":files.files(),"source_root":c.selection.root(),"source_files":c.selection.files(),"source_snapshot_limitation":true}),
        git: json!(capture.git()),
        environment: json!({"fingerprint":c.environment.fingerprint,"interpreter":c.environment.interpreter,"runtime_version":c.environment.runtime_version}),
        at_us: preparation::now().map_err(failure)?,
    };
    let receipt = rt.block_on(async {
        let mut owned = owner.open_store().await.map_err(failure)?;
        let receipt = owned
            .repository()
            .map_err(failure)?
            .commit_import(&request, &artifact)
            .await
            .map_err(failure)?;
        owned.close().await.map_err(failure)?;
        Ok::<_, Error>(receipt)
    })?;
    Ok(
        json!({"kind":"import_publication","status":"published","published":true,"import_id":c.request.to_string(),"workspace_id":workspace.config().id().to_string(),"dataset_id":c.dataset.to_string(),"path":c.path.as_str(),"branch":c.branch.as_str(),"source_snapshot_id":request.source.to_string(),"schema_normalization":"complete","source_snapshot_limitation":true,"registered":c.registered,"file_count":names.len().to_string(),"row_count":files.rows().map_err(failure)?.to_string(),"byte_count":files.bytes().map_err(failure)?.to_string(),"version_id":receipt.version.to_string(),"artifact_digest":artifact.digest().hex(),"generation":receipt.generation.to_string()}),
    )
}
