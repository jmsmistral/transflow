//! Real filesystem locator guards and safe partial-completion behavior.
#![allow(clippy::unwrap_used, reason = "Synthetic filesystem assertions")]
use std::{cell::Cell, fs, path::PathBuf};
use tf_catalog::{
    local_write::LocatorWrite, registry_write::RegistryWriteError, workspace::LocalConfig,
};
use tf_domain::{RequestId, WorkspaceId};
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tf-locator-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn local_edits_and_second_guard_failure_preserve_bytes_and_cleanup_temporary() {
    let t = Tree::new();
    let path = t.0.join("transflow.local.toml");
    fs::write(&path, "# original\n").unwrap();
    let prepare = || {
        LocatorWrite::prepare(
            &t.0,
            WorkspaceId::from_bytes([1; 16]),
            &PathBuf::from("/synthetic/provider"),
            RequestId::from_bytes([2; 16]),
        )
        .unwrap()
    };
    let write = prepare();
    fs::write(&path, "# author edit\n").unwrap();
    assert!(write.apply(|| Ok(())).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "# author edit\n");
    let write = prepare();
    let calls = Cell::new(0);
    assert!(
        write
            .apply(|| {
                calls.set(calls.get() + 1);
                if calls.get() == 2 {
                    Err(RegistryWriteError::Conflict)
                } else {
                    Ok(())
                }
            })
            .is_err()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "# author edit\n");
    assert_eq!(fs::read_dir(&t.0).unwrap().count(), 1);
    prepare().apply(|| Ok(())).unwrap();
    assert_eq!(
        LocalConfig::parse(&fs::read_to_string(&path).unwrap())
            .unwrap()
            .provider(WorkspaceId::from_bytes([1; 16])),
        Some(PathBuf::from("/synthetic/provider").as_path())
    );
}
#[test]
fn symlink_and_replaced_directory_cannot_redirect_local_writes() {
    let t = Tree::new();
    let other = Tree::new();
    let original = other.0.join("file");
    fs::write(&original, "# untouched\n").unwrap();
    std::os::unix::fs::symlink(&original, t.0.join("transflow.local.toml")).unwrap();
    assert!(
        LocatorWrite::prepare(
            &t.0,
            WorkspaceId::from_bytes([1; 16]),
            &other.0,
            RequestId::from_bytes([2; 16])
        )
        .is_err()
    );
    fs::remove_file(t.0.join("transflow.local.toml")).unwrap();
    let write = LocatorWrite::prepare(
        &t.0,
        WorkspaceId::from_bytes([1; 16]),
        &other.0,
        RequestId::from_bytes([2; 16]),
    )
    .unwrap();
    let moved = other.0.join("moved");
    fs::rename(&t.0, &moved).unwrap();
    fs::create_dir(&t.0).unwrap();
    assert!(write.apply(|| Ok(())).is_err());
    assert!(!t.0.join("transflow.local.toml").exists());
    assert_eq!(fs::read_to_string(original).unwrap(), "# untouched\n");
}
