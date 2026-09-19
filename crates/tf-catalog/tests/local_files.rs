//! Explicit selection containment, deterministic glob membership and no later expansion.
#![allow(clippy::unwrap_used, reason = "Synthetic filesystem assertions")]
use std::{
    fs,
    os::unix::fs::symlink,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_catalog::local_files::FileSelection;
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let p = PathBuf::from(format!(
            "/tmp/tf-selection-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn glob_is_sorted_frozen_and_explicitly_contained() {
    let t = Tree::new();
    fs::write(t.0.join("z.parquet"), b"z").unwrap();
    fs::write(t.0.join("a.parquet"), b"a").unwrap();
    fs::create_dir(t.0.join("nested")).unwrap();
    fs::write(t.0.join("nested/b.parquet"), b"b").unwrap();
    let selection = FileSelection::freeze(&t.0.join("*.parquet")).unwrap();
    assert_eq!(
        selection.files(),
        [PathBuf::from("a.parquet"), PathBuf::from("z.parquet")]
    );
    fs::write(t.0.join("new.parquet"), b"new").unwrap();
    assert_eq!(selection.files().len(), 2);
    assert_eq!(
        FileSelection::freeze(&t.0.join("**/*.parquet"))
            .unwrap()
            .files()
            .len(),
        4
    );
    assert!(FileSelection::freeze(&t.0.join("*.missing")).is_err());
    assert!(FileSelection::freeze(&t.0.join("[broken")).is_err());
}
#[test]
fn symlink_escape_and_root_replacement_are_refused() {
    let t = Tree::new();
    let outside = Tree::new();
    fs::write(outside.0.join("outside.parquet"), b"outside").unwrap();
    symlink(
        outside.0.join("outside.parquet"),
        t.0.join("escape.parquet"),
    )
    .unwrap();
    assert!(FileSelection::freeze(&t.0.join("*.parquet")).is_err());
    fs::remove_file(t.0.join("escape.parquet")).unwrap();
    fs::write(t.0.join("ok.parquet"), b"ok").unwrap();
    let selection = FileSelection::freeze(&t.0.join("*.parquet")).unwrap();
    let old = t.0.with_extension("old");
    fs::rename(&t.0, &old).unwrap();
    fs::create_dir(&t.0).unwrap();
    assert!(selection.verify_root().is_err());
    fs::remove_dir_all(old).unwrap();
}
