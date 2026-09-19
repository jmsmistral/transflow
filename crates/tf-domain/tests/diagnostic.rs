//! Safe construction boundaries shared by CLI and future services.
#![allow(clippy::unwrap_used, reason = "Test fixture assertions")]
use tf_domain::diagnostic::*;
fn diagnostic() -> Diagnostic {
    let r = Redactor::default();
    Diagnostic::new(
        DiagnosticCode::GraphCycle,
        r.text("Workspace validation failed").unwrap(),
        r.text("A dependency cycle was found in the transform graph.")
            .unwrap(),
        r.text("Remove one dependency before building. No transform functions were run.")
            .unwrap(),
    )
}
#[test]
fn secrets_controls_and_bidi_are_never_live_output() {
    let r = Redactor::new(vec![
        "fixture-private".into(),
        "fixture-private-long".into(),
    ])
    .unwrap();
    for text in [
        "fixture-private-long",
        "Authorization: Bearer synthetic",
        "password=synthetic",
        "Cookie: synthetic",
    ] {
        let s = r.text(text).unwrap();
        assert!(s.as_str().contains("REDACTED"));
        assert!(!s.as_str().contains("synthetic"));
        assert!(!s.as_str().contains("fixture-private"));
    }
    for scalar in (0..=0x9f).chain([
        0x61c, 0x200e, 0x200f, 0x2028, 0x202e, 0x2066, 0x2069, 0xfeff,
    ]) {
        let ch = char::from_u32(scalar).unwrap();
        if unsafe_character(ch) {
            let output = r.text(&format!("before{ch}after")).unwrap();
            assert!(!output.as_str().chars().any(unsafe_character));
            assert!(output.as_str().contains("\\u{"));
        }
    }
    assert!(r.text("").is_err());
    assert!(r.text(&"a".repeat(4097)).is_err());
    assert!(Redactor::new(vec![String::new()]).is_err());
}
#[test]
fn ranges_and_cause_trees_are_bounded_and_immutable() {
    let r = Redactor::default();
    let path = r.text("src/orders.py").unwrap();
    assert!(SourceRange::new(path.clone(), (0, 1), (1, 1)).is_err());
    assert!(SourceRange::new(path.clone(), (2, 9), (2, 8)).is_err());
    assert!(SourceRange::new(path, (2, 9), (3, 1)).is_ok());
    let original = diagnostic();
    let mut nested = original.clone();
    for _ in 0..3 {
        nested = original.clone().with_causes(vec![nested]).unwrap();
    }
    assert!(original.clone().with_causes(vec![nested]).is_err());
    assert!(
        original
            .clone()
            .with_causes(vec![original.clone(); 9])
            .is_err()
    );
    assert!(original.causes().is_empty());
    assert!(
        original
            .clone()
            .with_affected(vec![r.text("orders").unwrap(); 33])
            .is_err()
    );
    assert_eq!(
        [
            ExitStatus::Success,
            ExitStatus::Failure,
            ExitStatus::Usage,
            ExitStatus::Interrupted
        ]
        .map(ExitStatus::code),
        [0, 1, 2, 130]
    );
}
