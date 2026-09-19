//! Exact rendered bytes plus real durable refresh and failure boundaries.
#![allow(clippy::unwrap_used, reason = "Synthetic assertions")]
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_catalog::editor::{Boundary, EditorCache, EditorError, Overlay};
use tf_domain::RequestId;
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "transflow-editor-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!(
        "../../../schemas/fixtures/editor-overlays-v1.json"
    ))
    .unwrap()
}
fn overlay(index: usize) -> Overlay {
    Overlay::render(&cases()[index]["snapshot"]).unwrap()
}
fn request(n: u8) -> RequestId {
    RequestId::from_bytes([n; 16])
}
#[test]
fn renderer_matches_installed_checker_fixtures_and_ignores_capture_id() {
    for case in cases() {
        let o = Overlay::render(&case["snapshot"]).unwrap();
        assert_eq!(serde_json::to_value(o.files()).unwrap(), case["files"]);
        let mut snapshot = case["snapshot"].clone();
        snapshot["source_snapshot_id"] = "00000000-0000-0000-0000-000000000088".into();
        assert_eq!(Overlay::render(&snapshot).unwrap().files(), o.files());
        snapshot["aliases"][0]["path"] = "different".into();
        assert!(Overlay::render(&snapshot).is_err());
    }
}
#[test]
fn independent_workspaces_refresh_with_immutable_private_generations() {
    let a = Tree::new();
    let b = Tree::new();
    let first = overlay(0);
    let other = overlay(1);
    let second = overlay(2);
    let ac = EditorCache::open(&a.0).unwrap();
    let bc = EditorCache::open(&b.0).unwrap();
    let path = ac.refresh(&first, request(1), |_| Ok(())).unwrap();
    bc.refresh(&other, request(2), |_| Ok(())).unwrap();
    let original = fs::read(path.join("transflow/catalog.pyi")).unwrap();
    assert!(ac.check(&second).is_err());
    assert_eq!(ac.refresh(&second, request(3), |_| Ok(())).unwrap(), path);
    assert!(ac.check(&first).is_err());
    bc.check(&other).unwrap();
    assert_ne!(
        fs::read(path.join("transflow/catalog.pyi")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(
            a.0.join(".transflow/runtime/generated")
                .join(first.fingerprint())
                .join("type-stubs/transflow/catalog.pyi")
        )
        .unwrap(),
        original
    );
    assert_eq!(
        fs::metadata(path.join("manifest.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    ac.refresh(&second, request(4), |_| Ok(())).unwrap();
}
#[test]
fn interruption_at_each_boundary_preserves_active_generation() {
    for stop in [
        Boundary::Staged,
        Boundary::Installed,
        Boundary::BeforeActivate,
    ] {
        let t = Tree::new();
        let cache = EditorCache::open(&t.0).unwrap();
        let old = overlay(0);
        let new = overlay(2);
        cache.refresh(&old, request(1), |_| Ok(())).unwrap();
        assert!(
            cache
                .refresh(&new, request(2), |b| if b == stop {
                    Err(EditorError::Conflict)
                } else {
                    Ok(())
                })
                .is_err()
        );
        cache.check(&old).unwrap();
        assert!(cache.check(&new).is_err());
        cache.refresh(&new, request(3), |_| Ok(())).unwrap();
    }
}
#[test]
fn tampered_generations_and_unsafe_pointers_are_never_accepted() {
    for change in ["bytes", "extra", "symlink", "manifest", "pointer"] {
        let t = Tree::new();
        let cache = EditorCache::open(&t.0).unwrap();
        let o = overlay(0);
        let current = cache.refresh(&o, request(1), |_| Ok(())).unwrap();
        match change {
            "bytes" => fs::write(current.join("transflow/catalog.pyi"), "wrong").unwrap(),
            "extra" => {
                fs::write(current.join("transflow/__init__.py"), "raise RuntimeError").unwrap()
            }
            "symlink" => {
                let p = current.join("transflow/catalog.pyi");
                fs::remove_file(&p).unwrap();
                symlink("__init__.pyi", p).unwrap();
            }
            "manifest" => fs::write(current.join("manifest.json"), "{}").unwrap(),
            _ => {
                fs::remove_file(&current).unwrap();
                symlink("/tmp", &current).unwrap();
            }
        }
        assert!(cache.check(&o).is_err());
        assert!(cache.refresh(&o, request(2), |_| Ok(())).is_err());
    }
}
#[test]
fn directory_replacement_or_competing_refresh_cannot_activate() {
    for replace in [true, false] {
        let t = Tree::new();
        let cache = EditorCache::open(&t.0).unwrap();
        let first = overlay(0);
        let second = overlay(2);
        cache.refresh(&first, request(1), |_| Ok(())).unwrap();
        assert!(
            cache
                .refresh(&second, request(2), |b| {
                    if b == Boundary::BeforeActivate {
                        let generated = t.0.join(".transflow/runtime/generated");
                        if replace {
                            fs::rename(&generated, generated.with_file_name("moved")).unwrap();
                            fs::create_dir(&generated).unwrap();
                        } else {
                            fs::remove_file(generated.join("current")).unwrap();
                            symlink(
                                format!("{}/type-stubs", "f".repeat(64)),
                                generated.join("current"),
                            )
                            .unwrap();
                        }
                    }
                    Ok(())
                })
                .is_err()
        );
    }
    let t = Tree::new();
    symlink("/tmp", t.0.join(".transflow")).unwrap();
    assert!(EditorCache::open(&t.0).is_err());
}
