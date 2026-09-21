//! Workspace authoring policy has no interpreter, Git or runtime-store dependency.
#![allow(clippy::unwrap_used, reason = "Synthetic test assertions")]
use tf_catalog::workspace::*;
const BASE: &str = "format_version=1\nworkspace_id='1837c4ad-54ed-4ea9-8301-cde739093271'\n";
#[test]
fn defaults_and_field_specific_precedence_preserve_disabled_timer() {
    let c = WorkspaceConfig::parse(BASE).unwrap();
    assert_eq!(c.source_roots(), [std::path::PathBuf::from("src")]);
    assert_eq!(c.default_branch(), "master");
    assert!(c.follow_git_branch());
    assert_eq!(c.fallbacks("develop"), ["master"]);
    let p = c
        .execution_policy(
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
    assert_eq!(p.transform_seconds.value, 3600);
    assert_eq!(p.validation_seconds.value, 3600);
    assert_eq!(p.interactive_seconds.value, 30);
    assert_eq!(p.max_jobs.value, 2);
    assert!(p.cpu_tokens > 0);
    assert!(p.memory_budget_mib.is_none());
    assert_eq!(p.transform_seconds.origin, Origin::Default);
    let c = WorkspaceConfig::parse(&format!(
        "{BASE}[execution]\nwall_timeout_seconds=15\nmemory_budget_mib=128\n"
    ))
    .unwrap();
    let p = c
        .execution_policy(
            &DefinitionOverrides {
                transform_seconds: Some(20),
            },
            &BuildOverrides {
                transform_seconds: Some(25),
                validation_seconds: Some(50),
                max_jobs: Some(3),
            },
            &BuildOverrides {
                transform_seconds: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        p.transform_seconds,
        Setting {
            value: 0,
            origin: Origin::Explicit
        }
    );
    assert_eq!(
        p.validation_seconds,
        Setting {
            value: 50,
            origin: Origin::Build
        }
    );
    assert_eq!(p.interactive_seconds.value, 30);
    assert_eq!(p.memory_budget_mib.unwrap().value, 128);
}
#[test]
fn strict_keys_paths_and_policies_fail_without_echoing_values() {
    for suffix in [
        "typo='sensitive-fixture'",
        "source_roots=['src','src/nested']",
        "source_roots=['../other']",
        "source_roots=['.transflow/runtime']",
        "source_roots=[]",
        "[python]\nversion='3.12'",
        "[execution]\nmax_jobs=0",
        "[execution]\ncpu_tokens='all'",
        "[execution]\nwall_timeout_seconds=-1",
        "[execution]\nmemory_budget_mib=0",
        "[validation]\nsample_rows=-1",
        "[security]\nlisten='0.0.0.0'",
        "[security]\ntelemetry=true",
        "[interactive]\npreview_rows=2000",
    ] {
        let e = WorkspaceConfig::parse(&format!("{BASE}{suffix}")).unwrap_err();
        assert!(!e.to_string().contains("sensitive-fixture"));
    }
    assert!(WorkspaceConfig::parse(&BASE.replace("format_version=1", "format_version=2")).is_err());
    assert!(
        WorkspaceConfig::parse(&format!("{BASE}source_roots=3"))
            .unwrap_err()
            .span
            .is_some()
    );
}
#[test]
fn locators_cannot_override_computation_policy() {
    let l=LocalConfig::parse("[python]\nexecutable='/synthetic/python'\n[secret_references]\nmarket='env:MARKET_TOKEN'\n[external_workspaces]\n'1837c4ad-54ed-4ea9-8301-cde739093271'='/synthetic/provider'\n").unwrap();
    assert_eq!(
        l.interpreter(),
        Some(std::path::Path::new("/synthetic/python"))
    );
    for text in [
        "[execution]\nwall_timeout_seconds=0",
        "[branching]\ndefault_fallbacks=[]",
        "[python]\nexecutable='../python'",
        "[secret_references]\nmarket='private-value'",
    ] {
        assert!(LocalConfig::parse(text).is_err());
    }
}

#[test]
fn only_the_supported_python_minor_is_accepted() {
    for minor in 0..=20 {
        let config = format!("{BASE}[python]\nversion='3.{minor}'\n");
        assert_eq!(WorkspaceConfig::parse(&config).is_ok(), minor == 14);
    }
}
