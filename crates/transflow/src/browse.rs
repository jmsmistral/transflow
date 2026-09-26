//! Current-authoring display metadata, independent of the accepted execution capture.
use crate::preparation::{self, Error};
use serde_json::Value;
use tf_catalog::{
    RegistrySnapshot, capture::SourceSnapshot, editor::EditorError, workspace::Workspace,
};
use tf_domain::RequestId;
use tf_exec::{environment::ManagedEnvironment, ownership::RuntimeOwner};

/// Refresh only before accepting a current-working-tree build, under its owner and source guards.
/// Registration can change catalogue bytes, so display discovery receives its own verified capture.
/// Historical/replay callers must not call this; execution keeps its original source identity.
pub(crate) fn refresh(
    owner: &RuntimeOwner,
    workspace: &Workspace,
    previous: &SourceSnapshot,
    registry: &RegistrySnapshot,
    discovery: &Value,
    environment: &ManagedEnvironment,
    request: RequestId,
) -> Result<(), Error> {
    owner.validate_paths()?;
    previous.verify_working_copy(workspace, true)?;
    let capture = tf_catalog::git::capture_working_tree(workspace, preparation::limits())?;
    previous.verify_working_copy(workspace, true)?;
    if !previous.same_inputs_except_registry(&capture)
        || RegistrySnapshot::parse(
            workspace.config().id(),
            &preparation::text(&capture, ".transflow/catalog.toml")?,
        )?
        .raw_digest()
            != registry.raw_digest()
    {
        return Err(Error::Context);
    }
    let mut discovery = discovery.clone();
    let source = capture.id()?;
    discovery["source_snapshot_id"] = source.to_string().into();
    let projection = registry.sdk_projection(source)?;
    discovery["catalog_fingerprint"] = projection["catalog_fingerprint"].clone();
    preparation::rebind(
        &mut discovery,
        projection["catalog_fingerprint"]
            .as_str()
            .ok_or(Error::Context)?,
    );
    let graph = preparation::validate(registry, &capture, &discovery, environment)?;
    tf_catalog::graph_cache::retain(
        workspace.root(),
        source,
        &graph,
        &discovery,
        request,
        || {
            owner.validate_paths().map_err(|_| EditorError::Conflict)?;
            capture
                .verify_working_copy(workspace, false)
                .map_err(|_| EditorError::Conflict)
        },
    )?;
    Ok(())
}
