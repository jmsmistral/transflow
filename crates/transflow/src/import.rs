//! Explicit preparation only; publication consumes this staging in later services.
use crate::preparation;
use serde_json::{Value, json};
use std::path::Path;
use tf_catalog::{
    DatasetKind, RegistryErrorKind, RegistrySnapshot,
    candidate::RegistryProposal,
    capture::{CaptureLimits, SourceSnapshot},
    editor::{EditorCache, EditorError, Overlay},
    local_files::FileSelection,
    registry_write::WriteContext,
    workspace::{Workspace, read_authoring},
};
use tf_domain::{BranchName, DatasetId, DatasetPath, DatasetScope};
use tf_exec::{
    discovery, environment,
    ownership::{CoordinatorMode, RuntimeOwner},
};
use tf_store::imports::PreparedFiles;
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Preparation(#[from] preparation::Error),
    #[error(transparent)]
    Selection(#[from] tf_catalog::local_files::SelectionError),
    #[error(transparent)]
    Copy(#[from] tf_store::imports::ImportError),
    #[error(
        "Import publication is not implemented yet; use --prepare-only to copy and validate staging without publishing data"
    )]
    Publication,
    #[error(
        "Import requires an absent local path or existing imported identity; producing, foreign and tombstoned paths cannot be replaced"
    )]
    Target,
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Preparation(e.into())
    }
}
impl From<tf_catalog::workspace::ConfigError> for Error {
    fn from(e: tf_catalog::workspace::ConfigError) -> Self {
        Self::Preparation(e.into())
    }
}
impl From<tf_catalog::RegistryError> for Error {
    fn from(e: tf_catalog::RegistryError) -> Self {
        Self::Preparation(e.into())
    }
}
impl From<tf_exec::ownership::OwnershipError> for Error {
    fn from(e: tf_exec::ownership::OwnershipError) -> Self {
        Self::Preparation(e.into())
    }
}
impl From<tf_catalog::capture::CaptureError> for Error {
    fn from(e: tf_catalog::capture::CaptureError) -> Self {
        Self::Preparation(e.into())
    }
}
impl From<tf_catalog::candidate::CandidateError> for Error {
    fn from(e: tf_catalog::candidate::CandidateError) -> Self {
        Self::Preparation(e.into())
    }
}
impl From<crate::reconcile::ReconcileError> for Error {
    fn from(e: crate::reconcile::ReconcileError) -> Self {
        Self::Preparation(e.into())
    }
}
impl From<EditorError> for Error {
    fn from(e: EditorError) -> Self {
        Self::Preparation(e.into())
    }
}
impl From<environment::EnvironmentError> for Error {
    fn from(e: environment::EnvironmentError) -> Self {
        Self::Preparation(e.into())
    }
}
pub(crate) fn execute(args: &clap::ArgMatches, explicit: Option<&String>) -> Result<Value, Error> {
    if !args.get_flag("prepare-only") {
        return Err(Error::Publication);
    }
    let workspace = Workspace::load(&std::env::current_dir()?, explicit.map(Path::new))?;
    let mut owner = RuntimeOwner::acquire(
        workspace.root(),
        workspace.config().id(),
        CoordinatorMode::Temporary,
    )?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    if runtime
        .block_on(crate::reconcile::recover(&mut owner, preparation::now()?))?
        .iter()
        .any(|o| matches!(o, crate::reconcile::RecoveryOutcome::Conflict(_)))
    {
        return Err(preparation::Error::Context.into());
    }
    let registry = RegistrySnapshot::parse(
        workspace.config().id(),
        &read_authoring(&workspace.root().join(".transflow/catalog.toml"))?,
    )?;
    let reference = args.get_one::<String>("reference").ok_or(Error::Target)?;
    let (path, id, registered) = match registry.resolve_output(reference) {
        Ok(d) if d.kind() == DatasetKind::Imported => {
            (d.path().clone(), d.key().dataset_id(), false)
        }
        Err(e)
            if matches!(
                e.kind(),
                RegistryErrorKind::Missing | RegistryErrorKind::Namespace
            ) =>
        {
            let path = DatasetPath::parse_for_scope(reference, DatasetScope::Local)
                .map_err(|_| Error::Target)?;
            let id = discovery::random_id()?
                .parse::<DatasetId>()
                .map_err(|_| Error::Target)?;
            (path, id, true)
        }
        _ => return Err(Error::Target),
    };
    let branch: BranchName = args
        .get_one::<String>("branch")
        .map(String::as_str)
        .unwrap_or(workspace.config().default_branch())
        .parse()
        .map_err(|_| Error::Target)?;
    if branch.as_str().len() > 4096 {
        return Err(Error::Target);
    }
    let selection = FileSelection::freeze(Path::new(
        args.get_one::<String>("path").ok_or(Error::Target)?,
    ))?;
    let request = discovery::random_id()?.parse().map_err(|_| Error::Target)?;
    let files = PreparedFiles::copy(
        workspace.root(),
        selection.descriptor(),
        selection.files(),
        request,
        |_| Ok(()),
    )?;
    // The explicit import is a pending registration while discovery resolves Inputs.
    // No UUID is persisted unless the complete overlaid graph is structurally valid.
    let additions = if registered {
        vec![(path.clone(), id, DatasetKind::Imported)]
    } else {
        vec![]
    };
    let replacement = registry.render_additions(&additions)?;
    let overlay = RegistrySnapshot::parse(workspace.config().id(), &replacement)?;
    let mut inspected = preparation::inspect_overlay(
        &workspace,
        args.get_one::<String>("python"),
        true,
        Some((&registry, &overlay)),
    )?;
    let original_id = inspected.capture.id()?;
    let expected = if registered {
        replacement
    } else {
        String::from_utf8(
            inspected
                .capture
                .read(Path::new(".transflow/catalog.toml"), 16 * 1024 * 1024)?,
        )
        .map_err(|_| Error::Target)?
    };
    let proposal = RegistryProposal::imported(&registry, original_id, &path, id)?;
    selection.verify_root()?;
    files.verify_sources()?;
    files.verify_copies()?;
    if environment::inspect(
        workspace.root(),
        &inspected.python,
        &inspected.environment_request,
    )? != inspected.env
    {
        return Err(preparation::Error::Context.into());
    }
    let completed = runtime.block_on(crate::reconcile::reconcile(
        owner,
        crate::reconcile::ReconcileRequest {
            capture: inspected.capture,
            proposal,
            context: WriteContext::WorkingTree,
            id: request,
            at_us: preparation::now()?,
        },
    ))?;
    completed.outcome?;
    let original = SourceSnapshot::open(
        &workspace.root().join(".transflow/runtime/source-snapshots"),
        original_id,
    )?;
    original.verify_working_copy(&workspace, registered)?;
    let final_capture = SourceSnapshot::capture(&workspace, CaptureLimits::default())?;
    original.verify_working_copy(&workspace, registered)?;
    if !original.same_inputs_except_registry(&final_capture)
        || final_capture.read(Path::new(".transflow/catalog.toml"), 16 * 1024 * 1024)?
            != expected.as_bytes()
    {
        return Err(preparation::Error::Context.into());
    }
    inspected.result["source_snapshot_id"] = final_capture.id()?.to_string().into();
    let projection = overlay.sdk_projection(final_capture.id()?)?;
    preparation::rebind(
        &mut inspected.result,
        projection["catalog_fingerprint"]
            .as_str()
            .ok_or(Error::Target)?,
    );
    let graph = preparation::validate(&overlay, &final_capture, &inspected.result, &inspected.env)?;
    selection.verify_root()?;
    files.verify_sources()?;
    files.verify_copies()?;
    completed.owner.validate_paths()?;
    EditorCache::open(workspace.root())?.refresh(
        &Overlay::render(&projection)?,
        request,
        |_| {
            completed
                .owner
                .validate_paths()
                .map_err(|_| EditorError::Conflict)?;
            final_capture
                .verify_working_copy(&workspace, false)
                .map_err(|_| EditorError::Conflict)
        },
    )?;
    let value = json!({"kind":"import_preparation","status":"prepared","published":false,"import_id":request.to_string(),"workspace_id":workspace.config().id().to_string(),"dataset_id":id.to_string(),"path":path.as_str(),"branch":branch.as_str(),"source_snapshot_id":final_capture.id()?.to_string(),"schema_normalization":"complete","source_snapshot_limitation":true,"registered":registered,"file_count":selection.files().len().to_string(),"row_count":files.rows()?.to_string(),"byte_count":files.bytes()?.to_string(),"staging_path":format!(".transflow/runtime/import-staging/{request}")});
    let mut manifest = value.clone();
    for key in [
        "status",
        "published",
        "registered",
        "file_count",
        "row_count",
        "byte_count",
        "staging_path",
    ] {
        manifest.as_object_mut().ok_or(Error::Target)?.remove(key);
    }
    manifest["kind"] = "import_staging".into();
    manifest["format_version"] = 1.into();
    manifest["catalog_fingerprint"] = projection["catalog_fingerprint"].clone();
    manifest["environment_fingerprint"] = inspected.env.fingerprint.into();
    manifest["validation_certificate_fingerprint"] = graph.certificate().fingerprint().hex().into();
    manifest["source_root"] = selection.root().to_str().ok_or(Error::Target)?.into();
    manifest["source_files"] = json!(selection.files());
    manifest["files"] = files.files();
    manifest["logical_schema"] = files.schema()?.value().clone();
    manifest["schema_fingerprint"] = files.schema()?.fingerprint().into();
    selection.verify_root()?;
    files.retain(&manifest)?;
    Ok(value)
}
