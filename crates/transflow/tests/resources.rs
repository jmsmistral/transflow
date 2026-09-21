//! Configuration precedence is projected into the same frozen runtime limits used by planning.
#![allow(clippy::unwrap_used)]
use tf_catalog::workspace::{BuildOverrides, DefinitionOverrides, Origin, WorkspaceConfig};
use transflow::resources::Resolved;
#[test]
fn resolved_resources_preserve_defaults_origins_disabled_limits_and_optional_memory() {
    let base = "format_version=1\nworkspace_id='1837c4ad-54ed-4ea9-8301-cde739093271'\n";
    let config = WorkspaceConfig::parse(base).unwrap();
    let policy = config
        .execution_policy(
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
    let resolved = Resolved::new(&policy).unwrap();
    assert_eq!(resolved.capacity.jobs, 2);
    assert_eq!(resolved.capacity.memory_bytes, None);
    assert_eq!(resolved.timers.validation.value, 3600);
    assert_eq!(resolved.timers.interactive.value, 30);
    let config=WorkspaceConfig::parse(&format!("{base}[execution]\nmax_jobs=3\ncpu_tokens=12\nmemory_budget_mib=128\nwall_timeout_seconds=45\n")).unwrap();
    let policy = config
        .execution_policy(
            &DefinitionOverrides {
                transform_seconds: Some(90),
            },
            &BuildOverrides {
                validation_seconds: Some(60),
                ..Default::default()
            },
            &BuildOverrides {
                transform_seconds: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    let resolved = Resolved::new(&policy).unwrap();
    assert_eq!(resolved.capacity.memory_bytes, Some(128 * 1024 * 1024));
    assert_eq!(resolved.worker_threads, 4);
    assert_eq!(resolved.timers.transform.value, 0);
    assert_eq!(resolved.timers.transform.origin, Origin::Explicit);
    assert_eq!(resolved.timers.discovery.value, 45);
    assert_eq!(resolved.timers.discovery.origin, Origin::Workspace);
    let value = resolved.json(&policy);
    tf_protocol::validate_document("ResourcePolicyV1", &value).unwrap();
    assert_eq!(value["memory_enforcement"], "estimated_reservation");
    let config = WorkspaceConfig::parse(&format!(
        "{base}[execution]\nmemory_budget_mib=9223372036854775807\n"
    ))
    .unwrap();
    let policy = config
        .execution_policy(
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
    assert!(Resolved::new(&policy).is_err());
}
