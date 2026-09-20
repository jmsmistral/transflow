//! The public composition boundary requires current complete validation before branch mutation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic validation fixtures"
)]
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_catalog::{
    RegistrySnapshot,
    git::{OutputBranchOrigin, OutputBranchSelection},
    validation::{self, CHECK_SEMANTICS, SDK_VERSION, ValidationRequest},
};
use tf_domain::*;
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
use tf_store::{
    Reader,
    branches::{BranchExistence, OutputBranchPreview},
};
use transflow::output_branch::{authorize_output_branch, ensure_output_branch};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn workspace() -> WorkspaceId {
    WorkspaceId::from_bytes([1; 16])
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
struct Tree(std::path::PathBuf);
impl Tree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tf-output-branch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join(".transflow/runtime")).unwrap();
        fs::set_permissions(
            path.join(".transflow/runtime"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn current_validation_authorizes_creation_but_context_changes_and_metadata_only_do_not() {
    runtime().block_on(async {
        let tree = Tree::new();
        let registry = RegistrySnapshot::parse(workspace(), "format_version=1\n").unwrap();
        let source = SourceSnapshotId::from_bytes([2; 16]);
        let config = format!("format_version=1\nworkspace_id='{}'\n", workspace());
        let modules = BTreeMap::new();
        let discovery = json!({
            "format_version": 1,
            "source_snapshot_id": source.to_string(),
            "catalog_fingerprint": registry.sdk_projection(source).unwrap()["catalog_fingerprint"],
            "environment_fingerprint": "a".repeat(64),
            "definitions": [],
            "imported_modules": []
        });
        let source_digest = "b".repeat(64);
        let environment = "a".repeat(64);
        let changed_environment = "c".repeat(64);
        let mut context = ValidationRequest {
            registry: &registry,
            source,
            source_digest: &source_digest,
            config_toml: &config,
            modules: &modules,
            discovery: &discovery,
            environment_fingerprint: &environment,
            sdk_version: SDK_VERSION,
            check_semantics: CHECK_SEMANTICS,
        };
        let graph = validation::validate(&context).unwrap();
        let selected = OutputBranchSelection {
            name: "feature/new".parse().unwrap(),
            origin: OutputBranchOrigin::Explicit,
        };
        let preview = OutputBranchPreview::without_runtime(workspace(), selected.name.clone());
        let authorized =
            authorize_output_branch(&selected, preview.clone(), &graph, &context).unwrap();
        context.environment_fingerprint = &changed_environment;
        assert!(authorize_output_branch(&selected, preview, &graph, &context).is_err());

        let mut owner =
            RuntimeOwner::acquire(&tree.0, workspace(), CoordinatorMode::MetadataOnly).unwrap();
        let mut store = owner.open_store().await.unwrap();
        store
            .repository()
            .unwrap()
            .register_workspace(workspace(), "fixture", 1)
            .await
            .unwrap();
        store.close().await.unwrap();
        assert!(
            ensure_output_branch(&mut owner, &authorized, BranchId::from_bytes([3; 16]), 10)
                .await
                .is_err()
        );
        drop(owner);
        let mut reader = Reader::open_existing(&tree.0.join(".transflow/runtime/catalog.sqlite"))
            .await
            .unwrap();
        assert_eq!(
            reader
                .preview_output_branch(workspace(), &selected.name)
                .await
                .unwrap()
                .state(),
            BranchExistence::Absent
        );
        let mut owner =
            RuntimeOwner::acquire(&tree.0, workspace(), CoordinatorMode::Temporary).unwrap();
        let id = BranchId::from_bytes([3; 16]);
        assert_eq!(
            ensure_output_branch(&mut owner, &authorized, id, 11)
                .await
                .unwrap(),
            id
        );
        assert_eq!(
            reader
                .preview_output_branch(workspace(), &selected.name)
                .await
                .unwrap()
                .state(),
            BranchExistence::Present { id, revision: 0 }
        );
        reader.close().await.unwrap();
    });
}
