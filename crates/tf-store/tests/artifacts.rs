//! Real bytes, immutable installation and strict corruption/containment checks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic fixture assertions"
)]
use serde_json::json;
use std::{
    fs::{self, File, Permissions},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_domain::RequestId;
use tf_store::artifacts::{ArtifactStore, Boundary};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "tf-artifacts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        let p = p.canonicalize().unwrap();
        fs::create_dir_all(p.join(".transflow/runtime")).unwrap();
        fs::create_dir(p.join("source")).unwrap();
        for (name, fixture) in [("one", "two-rows.parquet"), ("empty", "empty.parquet")] {
            fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/fixtures/import")
                    .join(fixture),
                p.join("source").join(name),
            )
            .unwrap();
        }
        Self(p)
    }
    fn source(&self) -> File {
        File::open(self.0.join("source")).unwrap()
    }
    fn object(&self, hex: &str) -> PathBuf {
        self.0
            .join(".transflow/runtime/objects")
            .join(&hex[..2])
            .join(hex)
    }
}
fn writable(path: &Path) {
    let m = fs::symlink_metadata(path).unwrap();
    if m.is_symlink() {
        return;
    }
    if m.is_dir() {
        fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
        for e in fs::read_dir(path).unwrap() {
            writable(&e.unwrap().path());
        }
    } else {
        fs::set_permissions(path, Permissions::from_mode(0o600)).unwrap();
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        writable(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn writer() -> serde_json::Value {
    json!({"engine":"import","version":"fixture","compression":"uncompressed","row_group_size":"8192"})
}
fn id(n: u8) -> RequestId {
    RequestId::from_bytes([n; 16])
}
#[test]
fn ordered_multifile_and_empty_schema_bearing_artifacts_are_durable() {
    let tree = Tree::new();
    let store = ArtifactStore::open(&tree.0).unwrap();
    let paths = vec!["one".into(), "empty".into()];
    let candidate = store
        .prepare(&tree.source(), &paths, writer(), id(1), |_| Ok(()))
        .unwrap();
    let manifest = candidate.manifest().clone();
    let digest = candidate.digest().unwrap();
    assert_eq!(manifest["files"][0]["row_count"], "2");
    assert_eq!(manifest["files"][1]["row_count"], "0");
    let object = candidate.install(|_| Ok(())).unwrap();
    assert_eq!(object.digest(), digest);
    assert_eq!(store.verify(digest).unwrap().manifest(), &manifest);
    assert_eq!(
        fs::metadata(tree.object(&digest.hex()).join("part-00000.parquet"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o400
    );
    fs::write(tree.0.join("source/one"), b"changed source").unwrap();
    assert!(store.verify(digest).is_ok());
    let empty = store
        .prepare(&tree.source(), &["empty".into()], writer(), id(2), |_| {
            Ok(())
        })
        .unwrap()
        .install(|_| Ok(()))
        .unwrap();
    assert_eq!(empty.manifest()["files"][0]["row_count"], "0");
    assert!(
        store
            .prepare(&tree.source(), &[], writer(), id(3), |_| Ok(()))
            .is_err()
    );
}
#[test]
fn order_affects_identity_and_duplicate_install_preserves_inode() {
    use std::os::unix::fs::MetadataExt;
    let tree = Tree::new();
    let store = ArtifactStore::open(&tree.0).unwrap();
    let paths = vec!["one".into(), "empty".into()];
    let first = store
        .prepare(&tree.source(), &paths, writer(), id(1), |_| Ok(()))
        .unwrap()
        .install(|_| Ok(()))
        .unwrap();
    let path = tree.object(&first.digest().hex());
    let inode = fs::metadata(&path).unwrap().ino();
    let second = store
        .prepare(&tree.source(), &paths, writer(), id(2), |_| Ok(()))
        .unwrap()
        .install(|_| Ok(()))
        .unwrap();
    assert_eq!(first.digest(), second.digest());
    assert_eq!(inode, fs::metadata(path).unwrap().ino());
    let reverse = store
        .prepare(
            &tree.source(),
            &["empty".into(), "one".into()],
            writer(),
            id(3),
            |_| Ok(()),
        )
        .unwrap();
    assert_ne!(reverse.digest().unwrap(), first.digest());
    assert!(
        store
            .prepare(
                &tree.source(),
                &["one".into(), "one".into()],
                writer(),
                id(4),
                |_| Ok(())
            )
            .is_err()
    );
}
#[test]
fn corrupt_missing_extra_linked_and_noncanonical_objects_never_verify_or_repair() {
    for damage in [
        "footer", "content", "missing", "extra", "symlink", "hardlink", "manifest",
    ] {
        let tree = Tree::new();
        let store = ArtifactStore::open(&tree.0).unwrap();
        let first = store
            .prepare(&tree.source(), &["one".into()], writer(), id(1), |_| Ok(()))
            .unwrap()
            .install(|_| Ok(()))
            .unwrap();
        let path = tree.object(&first.digest().hex());
        writable(&path);
        let part = path.join("part-00000.parquet");
        match damage {
            "footer" => {
                let mut b = fs::read(&part).unwrap();
                let n = b.len();
                b[n - 1] ^= 1;
                fs::write(&part, b).unwrap();
            }
            "content" => {
                let mut b = fs::read(&part).unwrap();
                b[10] ^= 1;
                fs::write(&part, b).unwrap();
            }
            "missing" => fs::remove_file(&part).unwrap(),
            "extra" => fs::write(path.join("extra"), b"extra").unwrap(),
            "symlink" => {
                fs::remove_file(&part).unwrap();
                symlink(tree.0.join("source/one"), &part).unwrap();
            }
            "hardlink" => {
                fs::remove_file(&part).unwrap();
                fs::hard_link(tree.0.join("source/one"), &part).unwrap();
            }
            _ => {
                let mut b = fs::read(path.join("manifest.json")).unwrap();
                b.push(b'\n');
                fs::write(path.join("manifest.json"), b).unwrap();
            }
        }
        assert!(store.verify(first.digest()).is_err(), "{damage}");
        if damage == "hardlink" {
            fs::rename(tree.0.join("source/one"), tree.0.join("linked-source")).unwrap();
            fs::copy(&part, tree.0.join("source/one")).unwrap();
        }
        assert!(
            store
                .prepare(&tree.source(), &["one".into()], writer(), id(2), |_| Ok(()))
                .unwrap()
                .install(|_| Ok(()))
                .is_err(),
            "Must not repair {damage}"
        );
    }
}
#[test]
fn copy_mutations_and_path_escape_are_refused() {
    let tree = Tree::new();
    let store = ArtifactStore::open(&tree.0).unwrap();
    symlink("one", tree.0.join("source/link")).unwrap();
    for name in ["../source/one", "/etc/passwd", "link"] {
        assert!(
            store
                .prepare(&tree.source(), &[name.into()], writer(), id(1), |_| Ok(()))
                .is_err()
        );
    }
    assert!(
        store
            .prepare(&tree.source(), &["one".into()], writer(), id(1), |_| {
                fs::write(tree.0.join("source/one"), b"changed")
            })
            .is_err()
    );
    assert_eq!(
        fs::read_dir(tree.0.join(".transflow/runtime/objects"))
            .unwrap()
            .count(),
        0
    );
}
#[test]
fn install_failures_leave_no_visible_version_and_never_overwrite_objects() {
    for point in [
        Boundary::CandidateSynced,
        Boundary::BeforeInstall,
        Boundary::Installed,
        Boundary::Durable,
    ] {
        let tree = Tree::new();
        let store = ArtifactStore::open(&tree.0).unwrap();
        let hook = |b| {
            if b == point {
                Err(std::io::Error::from_raw_os_error(28))
            } else {
                Ok(())
            }
        };
        let candidate = store.prepare(&tree.source(), &["one".into()], writer(), id(1), hook);
        if point == Boundary::CandidateSynced {
            assert!(candidate.is_err());
            continue;
        }
        let candidate = candidate.unwrap();
        let digest = candidate.digest().unwrap();
        assert!(candidate.install(hook).is_err());
        assert_eq!(
            tree.object(&digest.hex()).exists(),
            matches!(point, Boundary::Installed | Boundary::Durable)
        );
        if tree.object(&digest.hex()).exists() {
            assert!(store.verify(digest).is_ok());
        }
        assert!(!tree.0.join(".transflow/runtime/state.sqlite").exists());
    }
}
#[test]
fn changed_runtime_namespace_is_rejected_without_following_it() {
    let tree = Tree::new();
    let store = ArtifactStore::open(&tree.0).unwrap();
    let old = tree.0.join(".transflow/runtime/objects");
    fs::rename(&old, tree.0.join("saved")).unwrap();
    fs::create_dir(&old).unwrap();
    assert!(
        store
            .prepare(&tree.source(), &["one".into()], writer(), id(1), |_| Ok(()))
            .is_err()
    );
    assert_eq!(fs::read_dir(old).unwrap().count(), 0);
}
