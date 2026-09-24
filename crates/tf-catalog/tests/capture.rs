//! Real immutable copies, mutation guards, retained reopen and failure cleanup.
#![allow(clippy::unwrap_used, reason = "Synthetic filesystem assertions")]
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_catalog::{
    capture::{CaptureError, CaptureLimits, SourceSnapshot},
    workspace::Workspace,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "transflow-capture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        let t = Self(p);
        t.put(
            "workspace.toml",
            "format_version=1\nworkspace_id='1837c4ad-54ed-4ea9-8301-cde739093271'\n",
        );
        t.put(".transflow/catalog.toml", "format_version=1\n");
        t.put("src/helper.py", "VALUE = 1\n");
        t.put("requirements.in", "polars==1.44.2\n");
        t.put("requirements.lock", "# synthetic dependency lock\n");
        fs::create_dir(t.0.join(".transflow/runtime")).unwrap();
        t
    }
    fn put(&self, path: &str, text: &str) {
        let p = self.0.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    fn workspace(&self) -> Workspace {
        Workspace::load(&self.0, None).unwrap()
    }
    fn parent(&self) -> PathBuf {
        self.0.join(".transflow/runtime/source-snapshots")
    }
    fn capture(&self) -> SourceSnapshot {
        SourceSnapshot::capture(&self.workspace(), CaptureLimits::default()).unwrap()
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn exact_copied_bytes_survive_live_edits_and_reopen() {
    let t = Tree::new();
    t.put("src/query.sql", "select 1");
    t.put("src/.env", "synthetic private value");
    let first = t.capture();
    let again = t.capture();
    assert_ne!(first.id().unwrap(), again.id().unwrap());
    assert_eq!(first.digest(), again.digest());
    assert!(!first.files_root().join("src/.env").exists());
    t.put("src/helper.py", "VALUE = 2\n");
    let reopened = SourceSnapshot::open(&t.parent(), first.id().unwrap()).unwrap();
    assert_eq!(
        reopened.read(Path::new("src/helper.py"), 1024).unwrap(),
        b"VALUE = 1\n"
    );
    assert_ne!(reopened.digest(), t.capture().digest());
    assert!(reopened.read(Path::new("src/helper.py"), 1).is_err());
    assert!(reopened.read(Path::new("../workspace.toml"), 1024).is_err());
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            t.parent()
                .join(first.id().unwrap().to_string())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(manifest["git"].is_null());
}
#[test]
fn observable_file_mutation_or_new_file_aborts_and_cleans_staging() {
    for add in [false, true] {
        let t = Tree::new();
        let result = SourceSnapshot::capture_with_observer(
            &t.workspace(),
            CaptureLimits::default(),
            |path| {
                if path == Path::new("src/helper.py") {
                    if add {
                        t.put("src/new.py", "pass");
                    } else {
                        t.put("src/helper.py", "VALUE = 2\n");
                    }
                }
                Ok(())
            },
        );
        assert!(matches!(result, Err(CaptureError::Changed)));
        assert_eq!(fs::read_dir(t.parent()).unwrap().count(), 0);
        assert_eq!(
            fs::read_to_string(t.0.join(".transflow/catalog.toml")).unwrap(),
            "format_version=1\n"
        );
    }
}
#[test]
fn missing_lock_limits_and_cancellation_do_not_publish_partial_capture() {
    let t = Tree::new();
    fs::remove_file(t.0.join("requirements.lock")).unwrap();
    assert!(SourceSnapshot::capture(&t.workspace(), CaptureLimits::default()).is_err());
    assert_eq!(fs::read_dir(t.parent()).unwrap().count(), 0);
    t.put("requirements.lock", "locked");
    assert!(matches!(
        SourceSnapshot::capture(
            &t.workspace(),
            CaptureLimits {
                file_bytes: 1,
                total_bytes: 1
            }
        ),
        Err(CaptureError::Limit)
    ));
    assert_eq!(fs::read_dir(t.parent()).unwrap().count(), 0);
    let result =
        SourceSnapshot::capture_with_observer(&t.workspace(), CaptureLimits::default(), |_| {
            Err(std::io::Error::other("synthetic cancellation"))
        });
    assert!(matches!(result, Err(CaptureError::Io(_))));
    assert_eq!(fs::read_dir(t.parent()).unwrap().count(), 0);
}
#[test]
fn corruption_unlisted_files_and_retained_symlinks_are_rejected() {
    let t = Tree::new();
    let first = t.capture();
    let file = first.files_root().join("src/helper.py");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&file, "changed").unwrap();
    assert!(first.read(Path::new("src/helper.py"), 1024).is_err());
    assert!(SourceSnapshot::open(&t.parent(), first.id().unwrap()).is_err());
    let second = t.capture();
    fs::write(second.files_root().join("src/unlisted.py"), "pass").unwrap();
    assert!(SourceSnapshot::open(&t.parent(), second.id().unwrap()).is_err());
    let third = t.capture();
    fs::remove_file(third.files_root().join("src/helper.py")).unwrap();
    std::os::unix::fs::symlink(
        t.0.join("src/helper.py"),
        third.files_root().join("src/helper.py"),
    )
    .unwrap();
    assert!(SourceSnapshot::open(&t.parent(), third.id().unwrap()).is_err());
}
#[test]
fn internal_source_symlinks_are_copied_by_value() {
    let t = Tree::new();
    std::os::unix::fs::symlink("helper.py", t.0.join("src/alias.py")).unwrap();
    let capture = t.capture();
    assert!(
        !fs::symlink_metadata(capture.files_root().join("src/alias.py"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        capture.read(Path::new("src/alias.py"), 1024).unwrap(),
        b"VALUE = 1\n"
    );
}
#[test]
fn no_git_executable_or_repository_is_required() {
    const FLAG: &str = "TRANSFLOW_CAPTURE_TEST_CHILD";
    if std::env::var_os(FLAG).is_some() {
        let t = Tree::new();
        let capture = t.capture();
        assert!(!t.0.join(".git").exists());
        assert_eq!(
            capture.read(Path::new("src/helper.py"), 1024).unwrap(),
            b"VALUE = 1\n"
        );
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "no_git_executable_or_repository_is_required"])
        .env(FLAG, "1")
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn edits_before_a_later_file_is_copied_are_detected() {
    let t = Tree::new();
    let result =
        SourceSnapshot::capture_with_observer(&t.workspace(), CaptureLimits::default(), |path| {
            if path == Path::new(".transflow/catalog.toml") {
                t.put("src/helper.py", "VALUE = 2\n");
            }
            Ok(())
        });
    assert!(matches!(result, Err(CaptureError::Changed)));
    assert_eq!(fs::read_dir(t.parent()).unwrap().count(), 0);
}

#[test]
fn registration_capture_guards_missing_lock_without_weakening_execution_capture() {
    let t = Tree::new();
    fs::remove_file(t.0.join("requirements.lock")).unwrap();
    let w = Workspace::load(&t.0, Some(&t.0)).unwrap();
    assert!(SourceSnapshot::capture(&w, CaptureLimits::default()).is_err());
    let registration = SourceSnapshot::registration(&w, None).unwrap();
    registration.verify_working_copy(&w, false).unwrap();
    fs::write(t.0.join("requirements.lock"), "new lock").unwrap();
    assert!(registration.verify_working_copy(&w, false).is_err());
}
