//! Real source-tree allowlisting and safety exclusions; no user imports.
#![allow(clippy::unwrap_used, reason = "Synthetic filesystem assertions")]
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_catalog::{source::SourceIndex, workspace::Workspace};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new(extra: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "transflow-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let t = Self(root);
        t.put("workspace.toml",&format!("format_version=1\nworkspace_id='1837c4ad-54ed-4ea9-8301-cde739093271'\nsource_roots=['src','sql']\n{extra}"));
        t.put(".transflow/catalog.toml", "format_version=1\n");
        fs::create_dir(t.0.join("src")).unwrap();
        fs::create_dir(t.0.join("sql")).unwrap();
        t
    }
    fn put(&self, path: &str, text: &str) {
        let p = self.0.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    fn scan(&self) -> Result<SourceIndex, tf_catalog::source::SourceError> {
        SourceIndex::enumerate(&Workspace::load(&self.0, None).unwrap())
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn helpers_and_sql_captured_but_credentials_runtime_and_environments_are_not() {
    let t = Tree::new("source_exclude=['**/scratch/**','*.tmp']\n");
    for path in [
        "src/common/__init__.py",
        "src/common/helper.py",
        "src/transforms/orders.py",
        "sql/query.sql",
        "src/resource.json",
    ] {
        t.put(path, "raise RuntimeError('must never execute')");
    }
    for path in [
        "src/.env",
        "src/.env.local",
        "src/key.pem",
        "src/.transflow/runtime/file",
        "src/.git/config",
        "src/.venv/lib/mod.py",
        "src/privateenv/pyvenv.cfg",
        "src/privateenv/mod.py",
        "src/scratch/deep/file.py",
        "sql/discard.tmp",
        "src/nested/workspace.toml",
        "src/nested/mod.py",
    ] {
        t.put(path, "do not capture");
    }
    let index = t.scan().unwrap();
    let paths: Vec<_> = index
        .files()
        .iter()
        .map(|f| f.path().to_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        [
            "sql/query.sql",
            "src/common/__init__.py",
            "src/common/helper.py",
            "src/resource.json",
            "src/transforms/orders.py"
        ]
    );
    assert!(
        index
            .require_file(std::path::Path::new("sql/query.sql"))
            .is_ok()
    );
    assert!(
        index
            .require_file(std::path::Path::new("src/.env"))
            .is_err()
    );
    assert!(!index.excluded().is_empty());
    assert_eq!(index, t.scan().unwrap());
}
#[test]
fn escaped_symlinks_and_protected_targets_fail() {
    let t = Tree::new("");
    let outside = Tree::new("");
    outside.put("src/secret.py", "private");
    std::os::unix::fs::symlink(outside.0.join("src/secret.py"), t.0.join("src/link.py")).unwrap();
    assert!(t.scan().unwrap_err().reason.contains("escapes"));
    fs::remove_file(t.0.join("src/link.py")).unwrap();
    t.put("src/.env", "secret");
    std::os::unix::fs::symlink(t.0.join("src/.env"), t.0.join("src/link.py")).unwrap();
    assert!(t.scan().unwrap_err().reason.contains("protected"));
}
#[test]
fn internal_file_links_are_explicit_candidates_but_directory_cycles_fail() {
    let t = Tree::new("");
    t.put("src/original.py", "pass");
    std::os::unix::fs::symlink("original.py", t.0.join("src/alias.py")).unwrap();
    assert_eq!(t.scan().unwrap().files().len(), 2);
    std::os::unix::fs::symlink(".", t.0.join("src/cycle")).unwrap();
    assert!(t.scan().unwrap_err().reason.contains("twice"));
}
#[test]
fn globs_match_basename_or_workspace_paths_and_invalid_patterns_fail() {
    let t = Tree::new("source_exclude=['src/temp[0-9].py','**/scratch','*.tmp']\n");
    for p in [
        "src/temp1.py",
        "src/keep.py",
        "src/nested/scratch/data.py",
        "sql/foo.tmp",
    ] {
        t.put(p, "pass");
    }
    assert_eq!(t.scan().unwrap().files().len(), 1);
    let t = Tree::new("source_exclude=['[abc']\n");
    assert!(t.scan().is_err());
}

#[test]
fn symlinks_cannot_bypass_directory_exclusions_or_nested_workspace_boundaries() {
    let t = Tree::new("source_exclude=['src/private']\n");
    t.put("src/private/helper.py", "private");
    std::os::unix::fs::symlink("private/helper.py", t.0.join("src/link.py")).unwrap();
    assert!(t.scan().unwrap().files().is_empty());
    fs::remove_file(t.0.join("src/link.py")).unwrap();
    t.put("src/nested/workspace.toml", "foreign");
    t.put("src/nested/helper.py", "foreign");
    std::os::unix::fs::symlink("nested/helper.py", t.0.join("src/link.py")).unwrap();
    assert!(t.scan().unwrap_err().reason.contains("nested workspace"));
}
