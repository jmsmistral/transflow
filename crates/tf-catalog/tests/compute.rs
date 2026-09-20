//! Result-affecting invalidation versus explicit presentation/resource exclusions.
#![allow(
    clippy::unwrap_used,
    reason = "Synthetic captured computation fixtures"
)]
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_catalog::{
    RegistrySnapshot,
    capture::{CaptureLimits, SourceSnapshot},
    compute::{self, Execution},
    validation::{self, ValidationRequest},
    workspace::Workspace,
};
use tf_domain::*;
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    definition: Value,
    environment: String,
    parameters: Value,
    inputs: Vec<Value>,
    writer: Value,
    clock: Option<i64>,
}
fn workspace() -> WorkspaceId {
    WorkspaceId::from_bytes([1; 16])
}
fn key(n: u8) -> DatasetKey {
    DatasetKey::new(workspace(), DatasetId::from_bytes([n; 16]))
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tf-compute-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join(".transflow/runtime")).unwrap();
        fs::write(
            root.join("workspace.toml"),
            format!("format_version=1\nworkspace_id='{}'\n", workspace()),
        )
        .unwrap();
        fs::write(root.join(".transflow/catalog.toml"),format!("format_version=1\n[[datasets]]\nid='{}'\npath='raw/input'\nkind='imported'\n[[datasets]]\nid='{}'\npath='curated/output'\nkind='transform'\n",key(2).dataset_id(),key(3).dataset_id())).unwrap();
        fs::write(
            root.join("src/output.py"),
            "# synthetic captured producer\n",
        )
        .unwrap();
        fs::write(root.join("src/helpers.py"), "VALUE=1\n").unwrap();
        fs::write(root.join("src/query.sql"), "select 1\n").unwrap();
        fs::write(root.join("requirements.in"), "# input\n").unwrap();
        fs::write(root.join("requirements.lock"), "# lock\n").unwrap();
        let definition = json!({"module":"output","function":"produce","path":"src/output.py","line":1,"source":false,"engine":"polars","inputs":[{"alias":"rows","ref":{"form":"string","value":"raw/input"},"branch":{"kind":"omitted","name":null},"stop_branch_fallback":false,"role":"data","checks":[]}],"output":{"ref":{"form":"string","value":"curated/output"},"schema":null,"checks":[{"id":"key","name":"Key","expectation":{"kind":"non_null","columns":["id"]},"on_error":"FAIL","null_policy":null,"sample_rows":null,"description":null}]},"parameters":[{"name":"limit","logical_type":{"type":"i64"},"default":{"type":"i64","value":"1"}}],"wall_timeout_seconds":null,"cache":"deterministic","refresh":null,"secret_refs":[],"lineage_json":null});
        let inputs = vec![
            json!({"consumer_workspace":workspace().to_string(),"consumer_dataset":key(3).dataset_id().to_string(),"alias":"rows","origin_workspace":workspace().to_string(),"origin_dataset":key(2).dataset_id().to_string(),"role":"data","declared":{"kind":"omitted","name":null},"starting_branch":"master","resolved_branch":"master","version":VersionId::from_bytes([4;16]).to_string(),"artifact":"d".repeat(64)}),
        ];
        Self {
            root,
            definition,
            environment: "a".repeat(64),
            parameters: json!({"limit":{"type":"i64","value":"1"}}),
            inputs,
            writer: json!({"format":"parquet","normalization":"canonical-v1"}),
            clock: None,
        }
    }
    fn key(&self) -> Result<compute::ComputeKey, compute::Error> {
        let workspace = Workspace::load(&self.root, None).unwrap();
        let capture = SourceSnapshot::capture(&workspace, CaptureLimits::default()).unwrap();
        let registry = RegistrySnapshot::parse(
            workspace.config().id(),
            &fs::read_to_string(self.root.join(".transflow/catalog.toml")).unwrap(),
        )
        .unwrap();
        let modules = BTreeMap::from([
            ("output".into(), "src/output.py".into()),
            ("helpers".into(), "src/helpers.py".into()),
        ]);
        let discovery = json!({"format_version":1,"source_snapshot_id":capture.id().unwrap().to_string(),"catalog_fingerprint":registry.sdk_projection(capture.id().unwrap()).unwrap()["catalog_fingerprint"],"environment_fingerprint":self.environment,"definitions":[self.definition],"imported_modules":["helpers","output"]});
        let config = fs::read_to_string(self.root.join("workspace.toml")).unwrap();
        let context = ValidationRequest {
            registry: &registry,
            source: capture.id().unwrap(),
            source_digest: capture.digest(),
            config_toml: &config,
            modules: &modules,
            discovery: &discovery,
            environment_fingerprint: &self.environment,
            sdk_version: validation::SDK_VERSION,
            check_semantics: validation::CHECK_SEMANTICS,
        };
        let graph = validation::validate(&context).unwrap();
        compute::finalize(
            &graph,
            &context,
            &capture,
            Execution {
                output: key(3),
                parameters: &self.parameters,
                inputs: &self.inputs,
                writer: &self.writer,
                evaluation_us: self.clock,
                secret_versions: &BTreeMap::new(),
            },
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn helper_sql_lock_and_environment_bytes_invalidate() {
    let mut f = Fixture::new();
    let baseline = f.key().unwrap().digest();
    assert_eq!(baseline, f.key().unwrap().digest());
    for path in ["src/helpers.py", "src/query.sql", "requirements.lock"] {
        let before = fs::read(f.root.join(path)).unwrap();
        fs::write(f.root.join(path), "# changed bytes\n").unwrap();
        assert_ne!(baseline, f.key().unwrap().digest());
        fs::write(f.root.join(path), before).unwrap();
    }
    f.environment = "b".repeat(64);
    assert_ne!(baseline, f.key().unwrap().digest());
}
#[test]
fn metadata_and_resource_limits_are_excluded_without_stripping_user_parameter_keys() {
    let mut f = Fixture::new();
    let baseline = f.key().unwrap();
    f.definition["output"]["checks"][0]["name"] = json!("New display label");
    f.definition["output"]["checks"][0]["description"] = json!("Human explanation");
    f.definition["wall_timeout_seconds"] = json!("5");
    f.definition["lineage_json"] = json!("{\"x\":10}");
    let path = f.root.join(".transflow/catalog.toml");
    let before = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        format!("{before}\n# catalogue presentation comment\n"),
    )
    .unwrap();
    let config = f.root.join("workspace.toml");
    let before = fs::read_to_string(&config).unwrap();
    fs::write(config,format!("{before}\n[execution]\nmax_jobs=7\nwall_timeout_seconds=3\n[interactive]\npreview_rows=9\n")).unwrap();
    assert_eq!(baseline.digest(), f.key().unwrap().digest());
    assert_eq!(
        baseline.check_fingerprint(),
        f.key().unwrap().check_fingerprint()
    );
    f.parameters["limit"]["value"] = json!("2");
    assert_ne!(baseline.digest(), f.key().unwrap().digest());
}
#[test]
fn exact_versions_branches_checks_writer_and_clock_affect_identity() {
    let mut f = Fixture::new();
    let baseline = f.key().unwrap();
    let original = f.inputs[0].clone();
    for (key, value) in [
        ("version", json!(VersionId::from_bytes([5; 16]).to_string())),
        ("artifact", json!("e".repeat(64))),
        ("resolved_branch", json!("release")),
    ] {
        f.inputs[0][key] = value;
        assert_ne!(baseline.digest(), f.key().unwrap().digest());
        f.inputs[0] = original.clone();
    }
    f.definition["output"]["checks"][0]["on_error"] = json!("WARN");
    assert_ne!(baseline.digest(), f.key().unwrap().digest());
    assert_ne!(
        baseline.check_fingerprint(),
        f.key().unwrap().check_fingerprint()
    );
    f.definition["output"]["checks"][0]["on_error"] = json!("FAIL");
    f.clock = Some(1);
    assert_ne!(baseline.digest(), f.key().unwrap().digest());
    f.clock = None;
    f.writer["normalization"] = json!("canonical-v2");
    assert_ne!(baseline.digest(), f.key().unwrap().digest());
}
#[test]
fn missing_duplicate_or_wrong_owner_bindings_never_produce_a_key() {
    let mut f = Fixture::new();
    let input = f.inputs[0].clone();
    f.inputs.clear();
    assert!(matches!(f.key(), Err(compute::Error::Pending)));
    f.inputs = vec![input.clone(), input.clone()];
    assert!(matches!(f.key(), Err(compute::Error::Semantics)));
    f.inputs = vec![input];
    f.inputs[0]["origin_workspace"] = json!(WorkspaceId::from_bytes([9; 16]).to_string());
    assert!(matches!(f.key(), Err(compute::Error::Semantics)));
}

#[test]
fn typed_parameter_defaults_and_external_io_cache_opt_in_are_explicit() {
    let mut f = Fixture::new();
    assert_eq!(
        compute::parameters(&f.definition, &json!({})).unwrap(),
        f.parameters
    );
    assert!(
        compute::parameters(&f.definition, &json!({"typo":{"type":"i64","value":"2"}})).is_err()
    );
    assert!(
        compute::parameters(
            &f.definition,
            &json!({"limit":{"type":"string","value":"2"}})
        )
        .is_err()
    );
    assert!(f.key().unwrap().reusable());
    f.definition["cache"] = json!("never");
    assert!(!f.key().unwrap().reusable());
}

#[test]
fn comparison_evidence_uses_same_semantic_inputs_and_survives_roundtrip() {
    let mut f = Fixture::new();
    let first = f.key().unwrap();
    let baseline = first.evidence();
    assert!(baseline.valid());
    assert_eq!(baseline.compute, first.digest().hex());
    let encoded = serde_json::to_value(baseline).unwrap();
    let restored: compute::Evidence = serde_json::from_value(encoded).unwrap();
    assert_eq!(&restored, baseline);
    f.parameters = json!({"limit":{"type":"i64","value":"2"}});
    let changed = f.key().unwrap();
    assert_ne!(
        baseline.components["parameters"],
        changed.evidence().components["parameters"]
    );
    assert_eq!(
        baseline.components["code"],
        changed.evidence().components["code"]
    );
    f.inputs[0]["version"] = json!(VersionId::from_bytes([8; 16]).to_string());
    let changed = f.key().unwrap();
    assert_ne!(baseline.inputs["rows"], changed.evidence().inputs["rows"]);
    assert_eq!(baseline.files, changed.evidence().files);
    fs::write(f.root.join("src/helpers.py"), "VALUE=2\n").unwrap();
    let changed = f.key().unwrap();
    assert_ne!(
        baseline.files["src/helpers.py"],
        changed.evidence().files["src/helpers.py"]
    );
    assert_ne!(
        baseline.components["code"],
        changed.evidence().components["code"]
    );
}
