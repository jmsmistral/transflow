//! Read-only Git provenance and explicit-ref source isolation in synthetic real repositories.
#![allow(clippy::unwrap_used, reason = "Synthetic Git fixture assertions")]
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_catalog::{
    capture::{CaptureLimits, SourceSnapshot},
    git::{self, GitError},
    workspace::Workspace,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree {
    root: PathBuf,
    workspace: PathBuf,
}
impl Tree {
    fn new(nested: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "transflow-git-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let workspace = if nested {
            root.join("analysis")
        } else {
            root.clone()
        };
        fs::create_dir_all(&workspace).unwrap();
        let t = Self { root, workspace };
        t.put("workspace.toml","format_version=1\nworkspace_id='1837c4ad-54ed-4ea9-8301-cde739093271'\nsource_exclude=['*.private']\n");
        t.put(".transflow/catalog.toml", "format_version=1\n");
        t.put("src/helper.py", "VALUE=1\n");
        t.put("requirements.in", "polars==1.44.2\n");
        t.put("requirements.lock", "locked\n");
        t.put(".gitignore", ".transflow/runtime/\ntransflow.local.toml\n");
        fs::create_dir(t.workspace.join(".transflow/runtime")).unwrap();
        t
    }
    fn put(&self, path: &str, text: &str) {
        let p = self.workspace.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    fn git(&self, args: &[&str]) -> String {
        let o = Command::new("git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(&self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap().trim().to_owned()
    }
    fn init(&self) {
        self.git(&["init", "--quiet", "--initial-branch=develop"]);
    }
    fn commit(&self) -> String {
        self.git(&["add", "."]);
        self.git(&["commit", "--quiet", "-m", "fixture"]);
        self.git(&["rev-parse", "HEAD"])
    }
    fn load(&self) -> Workspace {
        Workspace::load(&self.workspace, None).unwrap()
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn absent_unborn_attached_and_detached_branch_policy() {
    let t = Tree::new(false);
    let w = t.load();
    assert!(git::inspect(&w).unwrap().is_none());
    assert_eq!(
        git::output_branch(w.config(), None, None, false)
            .unwrap()
            .as_str(),
        "master"
    );
    t.init();
    let unborn = git::inspect(&w).unwrap().unwrap();
    assert_eq!(unborn.branch(), Some("develop"));
    assert_eq!(unborn.commit(), None);
    assert_eq!(
        git::output_branch(w.config(), Some(&unborn), None, false)
            .unwrap()
            .as_str(),
        "develop"
    );
    let commit = t.commit();
    let attached = git::inspect(&w).unwrap().unwrap();
    assert_eq!(attached.commit(), Some(commit.as_str()));
    assert!(attached.dirty_paths().is_empty());
    t.git(&["checkout", "--detach", "--quiet"]);
    let detached = git::inspect(&w).unwrap().unwrap();
    assert!(detached.branch().is_none());
    assert!(matches!(
        git::output_branch(w.config(), Some(&detached), None, false),
        Err(GitError::BranchRequired)
    ));
    let branch = "experiment".parse().unwrap();
    assert_eq!(
        git::output_branch(w.config(), Some(&detached), Some(&branch), false).unwrap(),
        branch
    );
}
#[test]
fn nested_repository_and_dirty_working_capture_retain_exact_source() {
    let t = Tree::new(true);
    t.init();
    t.commit();
    t.put("src/helper.py", "VALUE=2\n");
    t.put("src/new.py", "pass\n");
    let w = t.load();
    let p = git::inspect(&w).unwrap().unwrap();
    assert_eq!(p.workspace_relative(), Path::new("analysis"));
    assert_eq!(p.dirty_paths(), ["src/helper.py", "src/new.py"]);
    let capture = git::capture_working_tree(&w, CaptureLimits::default()).unwrap();
    assert_eq!(capture.git().unwrap().dirty_paths(), p.dirty_paths());
    assert_eq!(
        capture.read(Path::new("src/helper.py"), 100).unwrap(),
        b"VALUE=2\n"
    );
    let opened = SourceSnapshot::open(
        &t.workspace.join(".transflow/runtime/source-snapshots"),
        capture.id().unwrap(),
    )
    .unwrap();
    assert_eq!(opened.git(), capture.git());
}
#[test]
fn explicit_ref_uses_raw_committed_bytes_and_never_changes_checkout() {
    let t = Tree::new(true);
    t.init();
    t.put(
        ".gitattributes",
        "src/helper.py export-ignore export-subst\n",
    );
    t.put("src/secret.private", "must not capture");
    let first = t.commit();
    t.put("src/helper.py", "VALUE=2\n");
    t.commit();
    t.put("src/helper.py", "VALUE=3\n");
    let before_head = t.git(&["rev-parse", "HEAD"]);
    let before_status = t.git(&["status", "--porcelain"]);
    let w = t.load();
    assert!(matches!(
        git::capture_ref(&w, &first, None, CaptureLimits::default()),
        Err(GitError::BranchRequired)
    ));
    let capture = git::capture_ref(
        &w,
        &first,
        Some(&"results".parse().unwrap()),
        CaptureLimits::default(),
    )
    .unwrap();
    assert_eq!(
        capture.read(Path::new("src/helper.py"), 100).unwrap(),
        b"VALUE=1\n"
    );
    assert!(!capture.files_root().join("src/secret.private").exists());
    assert_eq!(capture.git().unwrap().commit(), Some(first.as_str()));
    assert_eq!(capture.git().unwrap().requested_ref(), Some(first.as_str()));
    assert!(capture.git().unwrap().dirty_paths().is_empty());
    assert_eq!(before_head, t.git(&["rev-parse", "HEAD"]));
    assert_eq!(before_status, t.git(&["status", "--porcelain"]));
    assert_eq!(
        fs::read_to_string(t.workspace.join("src/helper.py")).unwrap(),
        "VALUE=3\n"
    );
    assert!(
        !fs::read_dir(t.workspace.join(".transflow/runtime"))
            .unwrap()
            .any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("git-capture-"))
    );
}
#[test]
fn invalid_ref_broken_metadata_and_foreign_workspace_fail_closed() {
    let t = Tree::new(false);
    t.init();
    t.commit();
    let w = t.load();
    let branch = "results".parse().unwrap();
    assert!(git::capture_ref(&w, "missing-ref", Some(&branch), CaptureLimits::default()).is_err());
    t.put(
        "workspace.toml",
        "format_version=1\nworkspace_id='00000000-0000-4000-8000-000000000001'\n",
    );
    let foreign = t.commit();
    assert!(matches!(
        git::capture_ref(&w, &foreign, Some(&branch), CaptureLimits::default()),
        Err(GitError::Source)
    ));
    let t = Tree::new(false);
    fs::write(t.root.join(".git"), "invalid git pointer").unwrap();
    assert!(git::inspect(&t.load()).is_err());
}
#[test]
fn no_git_and_follow_disabled_do_not_require_an_output_override() {
    let t = Tree::new(false);
    let w = t.load();
    assert!(matches!(
        git::capture_ref(
            &w,
            "HEAD",
            Some(&"results".parse().unwrap()),
            CaptureLimits::default()
        ),
        Err(GitError::NoRepository)
    ));
    t.init();
    t.commit();
    t.git(&["checkout", "--quiet", "--detach"]);
    t.put("workspace.toml","format_version=1\nworkspace_id='1837c4ad-54ed-4ea9-8301-cde739093271'\n[branching]\nfollow_git_branch=false\n");
    let w = t.load();
    let p = git::inspect(&w).unwrap();
    assert_eq!(
        git::output_branch(w.config(), p.as_ref(), None, false)
            .unwrap()
            .as_str(),
        "master"
    );
}
