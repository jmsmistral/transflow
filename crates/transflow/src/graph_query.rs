//! Captured-source lineage inspection shared by future CLI/API adapters. No data reads or writes.
use crate::{
    build_plan::{Completion, Error, failure},
    preparation,
};
use std::path::Path;
use tf_catalog::{
    traversal::{Direction, Graph, Traversal},
    validation::{self, ValidationRequest},
    workspace::Workspace,
};
use tf_domain::BranchName;
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
/// Query context, independent of build modes, exclusions or visible canvas state.
#[derive(Clone, Debug)]
pub struct Request {
    /// Interpreter in the already prepared environment.
    pub python: Option<String>,
    /// Data branch label; source selection is captured separately.
    pub branch: BranchName,
    /// Exact registered path/alias, local dataset:UUID, or pending output path.
    pub start: String,
    /// Upstream or downstream declared lineage.
    pub direction: Direction,
    /// Maximum shortest hop count; omitted returns all reachable local/boundary nodes.
    pub depth: Option<u64>,
}
/// Capture and validate once, then return an immutable paginated traversal. Dropping the awaiting
/// future cannot release runtime ownership during source imports. Producers are never invoked.
pub async fn inspect(
    owner: RuntimeOwner,
    request: Request,
) -> Result<Completion<Traversal>, Error> {
    tokio::task::spawn_blocking(move || {
        let owner = owner;
        let result = (|| {
            owner.validate_paths().map_err(failure)?;
            if owner.registration().mode() == CoordinatorMode::MetadataOnly {
                return Err(failure("metadata-only owners cannot discover source"));
            }
            let workspace = Workspace::load(owner.workspace_root(), Some(owner.workspace_root()))
                .map_err(failure)?;
            if owner.workspace_id().map_err(failure)? != workspace.config().id() {
                return Err(failure(
                    "workspace identity changed during graph inspection",
                ));
            }
            let inspected =
                preparation::inspect(&workspace, request.python.as_ref(), true).map_err(failure)?;
            let config = String::from_utf8(
                inspected
                    .capture
                    .read(Path::new("workspace.toml"), 16 * 1024 * 1024)
                    .map_err(failure)?,
            )
            .map_err(failure)?;
            let modules = serde_json::from_value(inspected.result["module_index"].clone())
                .map_err(failure)?;
            let context = ValidationRequest {
                registry: &inspected.registry,
                source: inspected.capture.id().map_err(failure)?,
                source_digest: inspected.capture.digest(),
                config_toml: &config,
                modules: &modules,
                discovery: &inspected.result,
                environment_fingerprint: &inspected.env.fingerprint,
                sdk_version: &inspected.env.runtime_version,
                check_semantics: validation::CHECK_SEMANTICS,
            };
            let graph =
                Graph::validated(&inspected.graph, &context, request.branch).map_err(failure)?;
            graph
                .traverse(tf_catalog::traversal::Request {
                    start: graph.resolve(&request.start).map_err(failure)?,
                    direction: request.direction,
                    depth: request.depth,
                })
                .map_err(failure)
        })();
        Completion { owner, result }
    })
    .await
    .map_err(failure)
}
