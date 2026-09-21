//! Complete-graph validation and context-bound evidence, without filesystem writes.
#![allow(clippy::unwrap_used, reason = "Synthetic acceptance assertions")]
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tf_catalog::{
    RegistrySnapshot,
    validation::{
        self, CHECK_SEMANTICS, DeferredReason, SDK_VERSION, ValidationRequest, ValidationState,
    },
};
use tf_domain::{DatasetId, SourceSnapshotId, WorkspaceId, diagnostic::Redactor};
fn workspace() -> WorkspaceId {
    WorkspaceId::from_bytes([1; 16])
}
fn source() -> SourceSnapshotId {
    SourceSnapshotId::from_bytes([2; 16])
}
fn registry() -> RegistrySnapshot {
    RegistrySnapshot::parse(workspace(), "format_version=1\n").unwrap()
}
fn config() -> String {
    format!("format_version=1\nworkspace_id='{}'\n", workspace())
}
fn input(alias: &str, path: &str) -> Value {
    json!({"alias":alias,"ref":{"form":"string","value":path},"branch":{"kind":"omitted","name":null},"stop_branch_fallback":false,"role":"data","checks":[]})
}
fn definition(name: &str, path: &str, inputs: Vec<Value>) -> Value {
    json!({"module":name,"function":"produce","path":format!("src/{name}.py"),"line":3,"source":false,"engine":"polars","inputs":inputs,"output":{"ref":{"form":"string","value":path},"checks":[],"schema":null},"parameters":[],"wall_timeout_seconds":null,"cache":"deterministic","refresh":null,"secret_refs":[],"lineage_json":null})
}
fn check(kind: &str, columns: Vec<&str>) -> Value {
    json!({"id":"key","name":"Required key","expectation":{"ast_version":1,"kind":kind,"columns":columns},"on_error":"FAIL","null_policy":null,"sample_rows":null,"description":null})
}
fn schema() -> Value {
    json!({"format_version":1,"fields":[{"name":"id","logical_type":{"type":"i64"},"nullable":false}]})
}
struct Fixture {
    registry: RegistrySnapshot,
    config: String,
    modules: BTreeMap<String, String>,
    discovery: Value,
}
impl Fixture {
    fn new(defs: Vec<Value>) -> Self {
        Self::with_registry(registry(), defs)
    }
    fn with_registry(r: RegistrySnapshot, defs: Vec<Value>) -> Self {
        let modules = defs
            .iter()
            .map(|d| {
                (
                    d["module"].as_str().unwrap().to_owned(),
                    d["path"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let discovery = json!({"format_version":1,"source_snapshot_id":source().to_string(),"catalog_fingerprint":r.sdk_projection(source()).unwrap()["catalog_fingerprint"],"environment_fingerprint":"a".repeat(64),"definitions":defs,"imported_modules":modules.keys().collect::<Vec<_>>()});
        Self {
            registry: r,
            config: config(),
            modules,
            discovery,
        }
    }
    fn request(&self) -> ValidationRequest<'_> {
        ValidationRequest {
            registry: &self.registry,
            source: source(),
            source_digest: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            config_toml: &self.config,
            modules: &self.modules,
            discovery: &self.discovery,
            environment_fingerprint: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            sdk_version: SDK_VERSION,
            check_semantics: CHECK_SEMANTICS,
        }
    }
}
#[test]
fn fresh_complete_graph_is_nonpersistent_and_normalizes_inherited_checks() {
    let mut consumer = definition(
        "consumer",
        "curated/items",
        vec![input("items", "raw/items")],
    );
    consumer["inputs"][0]["checks"] = json!([check("non_null", vec!["id"])]);
    let mut f = Fixture::new(vec![consumer, definition("producer", "raw/items", vec![])]);
    f.config
        .push_str("[validation]\nnull_policy='ignore'\nsample_rows=7\n");
    let before = *f.registry.raw_digest();
    let original = f.discovery.clone();
    let graph = validation::validate(&f.request()).unwrap();
    assert_eq!(graph.candidate().pending().len(), 2);
    assert_eq!(*f.registry.raw_digest(), before);
    assert_eq!(f.registry.datasets().count(), 0);
    assert_eq!(f.discovery, original);
    let normalized = &graph.candidate().definitions()[0].inputs[0].declaration["checks"][0];
    assert_eq!(normalized["null_policy"], "ignore");
    assert_eq!(normalized["sample_rows"], "7");
    assert!(
        graph
            .deferred()
            .iter()
            .any(|d| d.check_id.as_deref() == Some("key")
                && d.reason == DeferredReason::SchemaUnavailable)
    );
    assert!(graph.certificate().matches(&f.request()));
    let mut evidence = graph.certificate().evidence();
    assert_eq!(evidence["source_snapshot_id"], source().to_string());
    assert_eq!(evidence["sdk"], SDK_VERSION);
    evidence["source_digest"] = "changed".into();
    assert_ne!(evidence, graph.certificate().evidence());
}
#[test]
fn invalid_unrelated_component_blocks_request_and_preserves_last_valid_display() {
    let good = Fixture::new(vec![definition("target", "target", vec![])]);
    let mut state = ValidationState::default();
    let fp = *state
        .submit(&good.request())
        .unwrap()
        .certificate()
        .fingerprint();
    let mut dependency = input("audit", "b");
    dependency["role"] = "validation".into();
    dependency["branch"] = json!({"kind":"named","name":"old"});
    let bad = Fixture::new(vec![
        definition("target", "target", vec![]),
        definition("one", "a", vec![dependency]),
        definition("two", "b", vec![input("value", "a")]),
    ]);
    let error = state.submit(&bad.request()).unwrap_err();
    assert_eq!(error.cycle, vec!["a", "b", "a"]);
    assert_eq!(error.locations.len(), 2);
    let diagnostic = error.diagnostic(&Redactor::default()).unwrap();
    assert_eq!(diagnostic.code().as_str(), "TF_GRAPH_CYCLE");
    assert_eq!(diagnostic.heading().as_str(), "Workspace validation failed");
    assert_eq!(diagnostic.sources()[0].start(), (3, 1));
    assert_eq!(*state.last_valid().unwrap().certificate().fingerprint(), fp);
    assert!(state.current_for(&good.request()).is_err());
    assert!(state.current_for(&bad.request()).is_err());
    assert_eq!(
        *state
            .submit(&good.request())
            .unwrap()
            .certificate()
            .fingerprint(),
        fp
    );
}
#[test]
fn all_context_inputs_invalidate_a_certificate() {
    let mut f = Fixture::new(vec![definition("one", "a", vec![])]);
    let certificate = validation::validate(&f.request())
        .unwrap()
        .certificate()
        .clone();
    for mutation in 0..7 {
        let mut request = f.request();
        let changed = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        match mutation {
            0 => request.source = SourceSnapshotId::from_bytes([3; 16]),
            1 => request.source_digest = changed,
            2 => request.environment_fingerprint = changed,
            3 => request.sdk_version = "future",
            4 => request.check_semantics = "future",
            5 => request.config_toml = "invalid TOML",
            _ => request.source_digest = "malformed",
        };
        assert!(!certificate.matches(&request));
    }
    f.config.push_str("# captured configuration edit\n");
    assert!(!certificate.matches(&f.request()));
    f.config = config();
    f.discovery["definitions"][0]["cache"] = "never".into();
    assert!(!certificate.matches(&f.request()));
    f.discovery["definitions"][0]["cache"] = "deterministic".into();
    f.modules.insert("helper".into(), "src/helper.py".into());
    assert!(!certificate.matches(&f.request()));
    f.modules.remove("helper");
    f.registry = RegistrySnapshot::parse(workspace(), "format_version=1\n# author edit\n").unwrap();
    assert!(!certificate.matches(&f.request()));
}
#[test]
fn declared_schema_errors_fail_but_real_data_checks_are_deferred() {
    for output in [true, false] {
        let mut producer = definition("producer", "a", vec![]);
        producer["output"]["schema"] = schema();
        let mut consumer = definition("consumer", "b", vec![input("value", "a")]);
        if output {
            producer["output"]["checks"] = json!([check("primary_key", vec!["absent"])]);
        } else {
            consumer["inputs"][0]["checks"] = json!([check("primary_key", vec!["absent"])]);
        }
        let mut f = Fixture::new(vec![producer, consumer]);
        assert!(
            validation::validate(&f.request())
                .unwrap_err()
                .message
                .contains("column")
        );
        let binding = if output {
            &mut f.discovery["definitions"][0]["output"]
        } else {
            &mut f.discovery["definitions"][1]["inputs"][0]
        };
        binding["checks"][0]["expectation"]["columns"] = json!(["id"]);
        let g = validation::validate(&f.request()).unwrap();
        assert!(
            g.deferred()
                .iter()
                .any(|d| d.check_id.as_deref() == Some("key")
                    && d.reason == DeferredReason::EvaluateData)
        );
    }
}
#[test]
fn foreign_and_named_branch_schemas_are_explicitly_unknown() {
    let r=RegistrySnapshot::parse(workspace(),&format!("format_version=1\n[[external_registrations]]\nid='{}'\nalias='external/provider/value'\nprovider_workspace_id='{}'\nprovider_dataset_id='{}'\nprovider_display_path='raw/value'\ndefault_branch='master'\n",tf_domain::ExternalRegistrationId::from_bytes([9;16]),WorkspaceId::from_bytes([4;16]),DatasetId::from_bytes([5;16]))).unwrap();
    let mut producer = definition("producer", "a", vec![]);
    producer["output"]["schema"] = schema();
    let mut historic = input("historic", "a");
    historic["branch"] = json!({"kind":"named","name":"old"});
    historic["checks"] = json!([check("non_null", vec!["unavailable_column"])]);
    let mut foreign = input("provider", "external/provider/value");
    foreign["checks"] = json!([check("non_null", vec!["foreign_column"])]);
    let f = Fixture::with_registry(
        r,
        vec![
            producer,
            definition("consumer", "b", vec![historic, foreign]),
        ],
    );
    let graph = validation::validate(&f.request()).unwrap();
    let pending: Vec<_> = graph
        .deferred()
        .iter()
        .filter(|d| d.check_id.is_some())
        .collect();
    assert_eq!(pending.len(), 2);
    assert!(
        pending
            .iter()
            .all(|d| d.reason == DeferredReason::SchemaUnavailable)
    );
}
#[test]
fn malformed_engine_alias_policy_check_and_parameter_metadata_fail() {
    let baseline = definition("consumer", "b", vec![input("value", "a")]);
    for change in 0..16 {
        let mut d = baseline.clone();
        match change {
            0 => d["engine"] = "duckdb".into(),
            1 => d["inputs"][0]["alias"] = "not an identifier".into(),
            2 => d["inputs"][0]["alias"] = "ctx".into(),
            3 => d["inputs"][0]["branch"]["name"] = "named_without_kind".into(),
            4 => d["output"]["checks"] = json!([check("non_null", vec![])]),
            5 => d["output"]["checks"] = json!([check("non_null", vec!["a", "b"])]),
            6 => d["output"]["checks"] = json!([check("primary_key", vec!["a", "a"])]),
            7 => {
                d["output"]["checks"] = json!([
                    check("primary_key", vec!["a"]),
                    check("primary_key", vec!["b"])
                ])
            }
            8 => {
                d["parameters"] = json!([{"name":"value","logical_type":{"type":"i64"},"default":{"type":"string","value":"wrong"}}])
            }
            9 => d["refresh"] = "always".into(),
            10 => d["lineage_json"] = "invalid JSON".into(),
            11 => d["source"] = true.into(),
            12 => d["secret_refs"] = json!(["secret"]),
            13 => d["cache"] = Value::Null,
            14 => d["output"]["checks"] = json!([check("sql", vec!["id"])]),
            _ => d["inputs"][0]["alias"] = "class".into(),
        }
        let f = Fixture::new(vec![definition("producer", "a", vec![]), d]);
        assert!(
            validation::validate(&f.request()).is_err(),
            "mutation {change}"
        );
        assert_eq!(f.registry.datasets().count(), 0);
    }
}
#[test]
fn imported_helpers_and_definition_locations_require_complete_matching_index() {
    for change in 0..5 {
        let mut f = Fixture::new(vec![definition("one", "a", vec![])]);
        match change {
            0 => {
                f.modules.insert("helper".into(), "src/helper.py".into());
            }
            1 => f.discovery["imported_modules"] = json!(["one", "one"]),
            2 => f.discovery["definitions"][0]["path"] = "src/other.py".into(),
            3 => {
                f.modules.insert("outside".into(), "../outside.py".into());
            }
            _ => f.discovery["environment_fingerprint"] = "f".repeat(64).into(),
        }
        assert!(validation::validate(&f.request()).is_err());
    }
    let mut f = Fixture::new(vec![]);
    f.modules.insert("helper".into(), "src/helper.py".into());
    f.discovery["imported_modules"] = json!(["helper"]);
    assert!(
        validation::validate(&f.request())
            .unwrap()
            .candidate()
            .definitions()
            .is_empty()
    );
}
#[test]
fn source_policy_typed_nested_defaults_and_unicode_aliases_are_accepted() {
    let mut d = definition("one", "a", vec![]);
    d["source"] = true.into();
    d["cache"] = Value::Null;
    d["refresh"] = 10.into();
    d["secret_refs"] = json!(["api_reference"]);
    d["parameters"] = json!([{"name":"options","logical_type":{"type":"list","element":{"name":"item","logical_type":{"type":"i64"},"nullable":true}},"default":{"type":"list","values":[{"type":"i64","value":"9"},{"type":"null"}]}}]);
    let f = Fixture::new(vec![d, definition("two", "b", vec![input("données", "a")])]);
    validation::validate(&f.request()).unwrap();
}

#[test]
fn installed_worker_fixture_validates_without_producer_execution_or_allocations() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../schemas/fixtures/structural-validation-v1.json"
    ))
    .unwrap();
    let f = Fixture {
        registry: registry(),
        config: config(),
        modules: serde_json::from_value(fixture["modules"].clone()).unwrap(),
        discovery: fixture["discovery"].clone(),
    };
    let graph = validation::validate(&f.request()).unwrap();
    assert_eq!(graph.candidate().definitions().len(), 2);
    assert_eq!(graph.candidate().pending().len(), 2);
    assert_eq!(
        graph
            .deferred()
            .iter()
            .filter(|d| d.check_id.is_some())
            .count(),
        2
    );
    assert_eq!(graph.deferred().len(), 5);
}

#[test]
fn input_preparation_preserves_aliases_and_rejects_a_changed_validation_context() {
    use tf_catalog::{input_bindings::local_bindings, workspace::WorkspaceConfig};
    use tf_domain::input::{BindingDisposition, InputRole};
    let raw = DatasetId::from_bytes([10; 16]);
    let output = DatasetId::from_bytes([11; 16]);
    let registry=RegistrySnapshot::parse(workspace(),&format!("format_version=1\n[[datasets]]\nid='{raw}'\npath='raw/items'\nkind='transform'\n[[datasets]]\nid='{output}'\npath='curated/items'\nkind='transform'\n")).unwrap();
    let mut right = input("baseline", "raw/items");
    right["branch"] = json!({"kind":"named","name":"master"});
    right["role"] = "validation".into();
    let mut f = Fixture::with_registry(
        registry,
        vec![
            definition("raw", "raw/items", vec![]),
            definition(
                "curated",
                "curated/items",
                vec![input("fresh", "raw/items"), right],
            ),
        ],
    );
    f.config
        .push_str("[branching]\ndefault_fallbacks=['master']\n[branching.fallbacks]\nmaster=[]\n");
    let graph = validation::validate(&f.request()).unwrap();
    let output_branch = "feature".parse().unwrap();
    let bindings = local_bindings(
        &graph,
        &f.request(),
        &output_branch,
        Some(&["develop".parse().unwrap()]),
    )
    .unwrap();
    assert_eq!(bindings.len(), 2);
    let writes = std::collections::BTreeSet::from([tf_domain::DatasetKey::new(workspace(), raw)]);
    for b in bindings.values() {
        if b.key.alias() == "fresh" {
            assert_eq!(b.disposition(&writes), BindingDisposition::PlannedProducer);
            assert_eq!(b.policy.candidates().len(), 2);
        } else {
            assert_eq!(b.role, InputRole::Validation);
            assert_eq!(b.disposition(&writes), BindingDisposition::OffBranch);
            assert_eq!(b.policy.candidates().len(), 1);
        }
    }
    let snapshot = WorkspaceConfig::parse(&f.config)
        .unwrap()
        .input_policies()
        .unwrap();
    f.config = f.config.replace("['master']", "['other']");
    assert!(local_bindings(&graph, &f.request(), &output_branch, None).is_err());
    assert_eq!(
        snapshot
            .local(
                &output_branch,
                tf_domain::BranchSelector::Omitted,
                tf_domain::FallbackPermission::Allowed,
                None
            )
            .unwrap()
            .candidates()[1]
            .as_str(),
        "master"
    );
}

#[test]
fn nested_ast_validates_aliases_types_and_preserves_deferred_obligations() {
    let mut d = definition("producer", "a", vec![input("reference", "raw/reference")]);
    // This fixture needs no registry data reads: the local producer declares the input.
    let source = definition("source", "raw/reference", vec![]);
    let mut c = check("non_null", vec!["id"]);
    c["expectation"] = json!({"ast_version":1,"kind":"dataset_all","children":[
        {"kind":"every","child":{"kind":"row_compare","op":"gte","left":{"kind":"column","name":"id"},"right":{"kind":"literal","value":{"type":"i64","value":"0"}}}},
        {"kind":"metric_compare","op":"equals","left":{"kind":"row_count","input":null},"right":{"kind":"row_count","input":{"kind":"input","alias":"reference"}}}
    ]});
    d["output"]["checks"] = json!([c]);
    let mut f = Fixture::new(vec![d, source]);
    let graph = validation::validate(&f.request()).unwrap();
    assert!(
        graph
            .deferred()
            .iter()
            .any(|d| d.check_id.as_deref() == Some("key")
                && d.reason == DeferredReason::SchemaUnavailable)
    );
    let original = f.discovery.clone();
    f.discovery["definitions"][0]["output"]["schema"] = schema();
    let graph = validation::validate(&f.request()).unwrap();
    assert!(
        graph
            .deferred()
            .iter()
            .any(|d| d.check_id.as_deref() == Some("key")
                && d.reason == DeferredReason::EvaluateData)
    );
    f.discovery["definitions"][0]["output"]["schema"]["fields"][0]["logical_type"]["type"] =
        "string".into();
    assert!(
        validation::validate(&f.request())
            .unwrap_err()
            .message
            .contains("incompatible")
    );
    f.discovery = original;
    f.discovery["definitions"][0]["output"]["checks"][0]["expectation"]["children"][1]["right"]["input"]
        ["alias"] = "hidden".into();
    assert!(
        validation::validate(&f.request())
            .unwrap_err()
            .message
            .contains("undeclared")
    );
}

#[test]
fn legacy_seed_normalizes_to_the_same_effective_ast_before_hashing() {
    let mut d = definition("producer", "a", vec![]);
    d["output"]["checks"] = json!([check("non_null", vec!["id"])]);
    let mut f = Fixture::new(vec![d]);
    let current = validation::validate(&f.request()).unwrap();
    f.discovery["definitions"][0]["output"]["checks"][0]["expectation"]
        .as_object_mut()
        .unwrap()
        .remove("ast_version");
    let legacy = validation::validate(&f.request()).unwrap();
    assert_eq!(
        current.candidate().definitions()[0].declaration["output"]["checks"],
        legacy.candidate().definitions()[0].declaration["output"]["checks"]
    );
}
