//! Real owner locks, SQLite, filesystem replacement and SIGKILL recovery.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic integration assertions"
)]
use serde_json::json;
use std::{
    fs,
    io::{self, BufRead, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
use tf_catalog::{
    RegistrySnapshot,
    candidate::CandidateCatalog,
    capture::{CaptureLimits, SourceSnapshot},
    registry_write::{Boundary, WriteContext},
    workspace::Workspace,
};
use tf_domain::{DatasetId, RequestId, WorkspaceId};
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
use tf_store::catalog_mutations::MutationState;
use transflow::reconcile::{
    ReconcileOutcome, ReconcileRequest, RecoveryOutcome, reconcile, reconcile_with_observer,
    recover,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn workspace_id() -> WorkspaceId {
    WorkspaceId::from_bytes([1; 16])
}
fn dataset_id() -> DatasetId {
    DatasetId::from_bytes([4; 16])
}
fn mutation_id() -> RequestId {
    RequestId::from_bytes([5; 16])
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let root = PathBuf::from("/tmp").join(format!(
            "tf-reconcile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let t = Self(root.canonicalize().unwrap());
        t.put(
            "workspace.toml",
            &format!("format_version=1\nworkspace_id='{}'\n", workspace_id()),
        );
        t.put(".transflow/catalog.toml", "format_version=1\n");
        t.put("requirements.in", "# synthetic dependencies\n");
        t.put("requirements.lock", "# synthetic lock\n");
        t.put("src/new.py", "# synthetic producer source\n");
        fs::create_dir(t.0.join(".transflow/runtime")).unwrap();
        fs::set_permissions(
            t.0.join(".transflow/runtime"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        t
    }
    fn put(&self, path: &str, text: &str) {
        let p = self.0.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    fn owner(&self) -> RuntimeOwner {
        RuntimeOwner::acquire(&self.0, workspace_id(), CoordinatorMode::Temporary).unwrap()
    }
    fn request(&self) -> ReconcileRequest {
        request(&self.0)
    }
    fn registry(&self) -> PathBuf {
        self.0.join(".transflow/catalog.toml")
    }
    fn bytes(&self) -> Vec<u8> {
        fs::read(self.registry()).unwrap()
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn request(root: &Path) -> ReconcileRequest {
    let w = Workspace::load(root, Some(root)).unwrap();
    let capture = tf_catalog::git::capture_working_tree(&w, CaptureLimits::default()).unwrap();
    request_for_capture(capture)
}
fn request_for_capture(capture: SourceSnapshot) -> ReconcileRequest {
    let registry = RegistrySnapshot::parse(
        workspace_id(),
        std::str::from_utf8(
            &capture
                .read(Path::new(".transflow/catalog.toml"), 1024 * 1024)
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let source = capture.id().unwrap();
    let result = json!({"format_version":1,"source_snapshot_id":source.to_string(),"catalog_fingerprint":registry.sdk_projection(source).unwrap()["catalog_fingerprint"],"environment_fingerprint":"a".repeat(64),"imported_modules":[],"definitions":[{"module":"new","function":"produce","path":"src/new.py","line":1,"source":false,"engine":"polars","inputs":[],"output":{"ref":{"form":"string","value":"raw/new"},"checks":[],"schema":null},"parameters":[],"wall_timeout_seconds":null,"cache":"deterministic","refresh":null,"secret_refs":[],"lineage_json":null}]});
    let proposal = CandidateCatalog::prepare(&registry, source, &result)
        .unwrap()
        .propose(|| Ok(dataset_id()))
        .unwrap();
    ReconcileRequest {
        capture,
        proposal,
        context: WriteContext::WorkingTree,
        id: mutation_id(),
        at_us: 123,
    }
}
async fn indexed(owner: &mut RuntimeOwner) -> bool {
    let mut store = owner.open_store().await.unwrap();
    let mut reader = store.repository().unwrap().reader().await.unwrap();
    let result = reader
        .contains_dataset(workspace_id(), dataset_id())
        .await
        .unwrap();
    reader.close().await.unwrap();
    store.close().await.unwrap();
    result
}
async fn state(owner: &mut RuntimeOwner) -> Option<MutationState> {
    let mut store = owner.open_store().await.unwrap();
    let result = store
        .repository()
        .unwrap()
        .catalog_mutation_state(mutation_id())
        .await
        .unwrap();
    store.close().await.unwrap();
    result
}
#[test]
fn commit_is_additive_preserves_permissions_and_noop_preserves_bytes() {
    let t = Tree::new();
    let old_id = DatasetId::from_bytes([9; 16]);
    t.put(".transflow/catalog.toml",&format!("format_version=1\n[[tombstones]]\nid='{old_id}'\npath='old/removed'\nkind='transform'\n"));
    fs::set_permissions(t.registry(), fs::Permissions::from_mode(0o640)).unwrap();
    runtime().block_on(async {
        let request = t.request();
        let expected = request.proposal.replacement().as_bytes().to_vec();
        let completion = reconcile(t.owner(), request).await.unwrap();
        assert_eq!(
            completion.outcome.unwrap(),
            ReconcileOutcome::Indexed(mutation_id())
        );
        let mut owner = completion.owner;
        assert_eq!(t.bytes(), expected);
        assert!(indexed(&mut owner).await);
        assert_eq!(state(&mut owner).await, Some(MutationState::Indexed));
        let mut store = owner.open_store().await.unwrap();
        let mut reader = store.repository().unwrap().reader().await.unwrap();
        assert!(
            reader
                .contains_dataset(workspace_id(), old_id)
                .await
                .unwrap()
        );
        assert_eq!(
            reader
                .dataset_version_count(workspace_id(), dataset_id())
                .await
                .unwrap(),
            0
        );
        reader.close().await.unwrap();
        store.close().await.unwrap();
        assert_eq!(
            fs::metadata(t.registry()).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let before = fs::metadata(t.registry()).unwrap();
        let completion = reconcile(owner, t.request()).await.unwrap();
        assert_eq!(completion.outcome.unwrap(), ReconcileOutcome::Unchanged);
        let mut owner = completion.owner;
        assert_eq!(t.bytes(), expected);
        assert_eq!(fs::metadata(t.registry()).unwrap().ino(), before.ino());
        assert!(recover(&mut owner, 456).await.unwrap().is_empty());
    });
}
#[test]
fn stale_registry_source_config_lock_and_membership_never_overwrite_edits() {
    for edit in [
        ".transflow/catalog.toml",
        "src/new.py",
        "workspace.toml",
        "requirements.lock",
        "src/added.py",
    ] {
        let t = Tree::new();
        let req = t.request();
        let old = t.bytes();
        let content = if edit == "workspace.toml" {
            format!("{}\n# edit\n", fs::read_to_string(t.0.join(edit)).unwrap())
        } else if edit == ".transflow/catalog.toml" {
            "format_version=1\n# user edit\n".to_owned()
        } else {
            "# changed\n".to_owned()
        };
        t.put(edit, &content);
        let edited = t.bytes();
        runtime().block_on(async {
            let c = reconcile(t.owner(), req).await.unwrap();
            assert!(c.outcome.is_err(), "{edit}");
            let mut owner = c.owner;
            assert_eq!(state(&mut owner).await, None);
            assert!(!indexed(&mut owner).await);
        });
        assert_eq!(t.bytes(), edited);
        if edit != ".transflow/catalog.toml" {
            assert_eq!(t.bytes(), old);
        }
    }
}
#[test]
fn edit_at_final_guard_conflicts_and_recovery_preserves_user_bytes() {
    for boundary in [Boundary::BeforeRename, Boundary::BeforeIndex] {
        let t = Tree::new();
        let req = t.request();
        let path = t.registry();
        runtime().block_on(async {
            let c = reconcile_with_observer(t.owner(), req, move |b| {
                if b == boundary {
                    fs::write(&path, "format_version=1\n# concurrent author edit\n")?;
                }
                Ok(())
            })
            .await
            .unwrap();
            assert!(c.outcome.is_err());
            let mut owner = c.owner;
            assert_eq!(
                recover(&mut owner, 456).await.unwrap(),
                vec![RecoveryOutcome::Conflict(mutation_id())]
            );
            assert!(!indexed(&mut owner).await);
            assert_eq!(state(&mut owner).await, Some(MutationState::Conflict));
        });
        assert_eq!(t.bytes(), b"format_version=1\n# concurrent author edit\n");
    }
}
#[test]
fn io_failure_before_rename_or_index_remains_recoverable_with_exact_ids() {
    for boundary in [
        Boundary::JournalPrepared,
        Boundary::TemporarySynced,
        Boundary::Renamed,
        Boundary::BeforeIndex,
    ] {
        let t = Tree::new();
        let req = t.request();
        let old = t.bytes();
        let new = req.proposal.replacement().as_bytes().to_vec();
        runtime().block_on(async {
            let c = reconcile_with_observer(t.owner(), req, move |b| {
                if b == boundary {
                    Err(io::Error::other("synthetic disk/IO failure"))
                } else {
                    Ok(())
                }
            })
            .await
            .unwrap();
            assert!(c.outcome.is_err());
            let mut owner = c.owner;
            let renamed = matches!(boundary, Boundary::Renamed | Boundary::BeforeIndex);
            assert_eq!(t.bytes(), if renamed { new } else { old });
            assert_eq!(
                recover(&mut owner, 456).await.unwrap(),
                vec![if renamed {
                    RecoveryOutcome::Indexed(mutation_id())
                } else {
                    RecoveryOutcome::NotApplied(mutation_id())
                }]
            );
            assert_eq!(indexed(&mut owner).await, renamed);
            assert!(recover(&mut owner, 789).await.unwrap().is_empty());
        });
        assert!(
            !fs::read_dir(t.0.join(".transflow")).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
    }
}
#[test]
fn fixed_source_metadata_owner_and_symlink_registry_cannot_write() {
    for mode in [0, 1, 2] {
        let t = Tree::new();
        let mut req = t.request();
        let original = t.bytes();
        if mode == 0 {
            req.context = WriteContext::FixedSource;
        }
        if mode == 2 {
            fs::rename(t.registry(), t.0.join("outside.toml")).unwrap();
            symlink(t.0.join("outside.toml"), t.registry()).unwrap();
        }
        let owner = if mode == 1 {
            RuntimeOwner::acquire(&t.0, workspace_id(), CoordinatorMode::MetadataOnly).unwrap()
        } else {
            t.owner()
        };
        runtime().block_on(async {
            assert!(reconcile(owner, req).await.unwrap().outcome.is_err());
        });
        assert_eq!(t.bytes(), original);
    }
}
#[test]
fn changed_source_after_rename_aborts_acceptance_but_keeps_valid_identity() {
    let t = Tree::new();
    let req = t.request();
    let new = req.proposal.replacement().as_bytes().to_vec();
    let source = t.0.join("src/new.py");
    runtime().block_on(async {
        let c = reconcile_with_observer(t.owner(), req, move |b| {
            if b == Boundary::BeforeIndex {
                fs::write(&source, "# later source edit\n")?;
            }
            Ok(())
        })
        .await
        .unwrap();
        assert!(c.outcome.is_err());
        let mut owner = c.owner;
        assert_eq!(t.bytes(), new);
        assert_eq!(
            recover(&mut owner, 456).await.unwrap(),
            vec![RecoveryOutcome::Indexed(mutation_id())]
        );
        assert!(indexed(&mut owner).await);
        let mut store = owner.open_store().await.unwrap();
        let mut reader = store.repository().unwrap().reader().await.unwrap();
        assert_eq!(
            reader
                .dataset_version_count(workspace_id(), dataset_id())
                .await
                .unwrap(),
            0
        );
        reader.close().await.unwrap();
        store.close().await.unwrap();
    });
}
#[test]
fn cancelling_the_waiter_keeps_ownership_until_the_durability_section_finishes() {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let t = Tree::new();
    let req = t.request();
    let owner = t.owner();
    let rt = runtime();
    let entered = rt.enter();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let mut future = Box::pin(reconcile_with_observer(owner, req, move |b| {
        if b == Boundary::BeforeRename {
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }
        if b == Boundary::Indexed {
            done_tx.send(()).unwrap();
        }
        Ok(())
    }));
    assert!(matches!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    drop(future);
    assert!(RuntimeOwner::acquire(&t.0, workspace_id(), CoordinatorMode::Temporary).is_err());
    release_tx.send(()).unwrap();
    done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    drop(entered);
    drop(rt);
    runtime().block_on(async {
        let mut owner = t.owner();
        assert!(indexed(&mut owner).await);
        assert!(recover(&mut owner, 456).await.unwrap().is_empty());
    });
}
#[test]
fn registry_child() {
    let Ok(root) = std::env::var("TRANSFLOW_REGISTRY_TEST_ROOT") else {
        return;
    };
    let boundary = std::env::var("TRANSFLOW_REGISTRY_TEST_BOUNDARY").unwrap();
    let root = PathBuf::from(root);
    let owner = RuntimeOwner::acquire(&root, workspace_id(), CoordinatorMode::Temporary).unwrap();
    let req = request(&root);
    runtime().block_on(async {
        let c = reconcile_with_observer(owner, req, move |b| {
            if format!("{b:?}") == boundary {
                println!("REGISTRY_BOUNDARY_READY");
                io::stdout().flush()?;
                io::stdin().read_exact(&mut [0u8; 1])?;
            }
            Ok(())
        })
        .await
        .unwrap();
        c.outcome.unwrap();
    });
}
#[test]
fn sigkill_at_every_durability_boundary_recovers_whole_registry_and_same_ids() {
    for boundary in [
        Boundary::JournalPrepared,
        Boundary::TemporarySynced,
        Boundary::BeforeRename,
        Boundary::Renamed,
        Boundary::DirectorySynced,
        Boundary::BeforeIndex,
        Boundary::Indexed,
    ] {
        let t = Tree::new();
        let req = t.request();
        let old = t.bytes();
        let new = req.proposal.replacement().as_bytes().to_vec();
        drop(req);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "registry_child", "--nocapture"])
            .env("TRANSFLOW_REGISTRY_TEST_ROOT", &t.0)
            .env("TRANSFLOW_REGISTRY_TEST_BOUNDARY", format!("{boundary:?}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in io::BufReader::new(stdout).lines() {
                if line.unwrap().contains("REGISTRY_BOUNDARY_READY") {
                    tx.send(()).unwrap();
                    return;
                }
            }
        });
        let ready = rx.recv_timeout(Duration::from_secs(15));
        let killed = child.kill();
        let status = child.wait().unwrap();
        reader.join().unwrap();
        ready.unwrap();
        killed.unwrap();
        assert!(!status.success());
        let renamed = matches!(
            boundary,
            Boundary::Renamed
                | Boundary::DirectorySynced
                | Boundary::BeforeIndex
                | Boundary::Indexed
        );
        assert_eq!(t.bytes(), if renamed { new } else { old });
        runtime().block_on(async {
            let mut owner = t.owner();
            let expected = if boundary == Boundary::Indexed {
                vec![]
            } else if renamed {
                vec![RecoveryOutcome::Indexed(mutation_id())]
            } else {
                vec![RecoveryOutcome::NotApplied(mutation_id())]
            };
            assert_eq!(recover(&mut owner, 456).await.unwrap(), expected);
            assert_eq!(indexed(&mut owner).await, renamed);
            assert!(recover(&mut owner, 789).await.unwrap().is_empty());
        });
    }
}

#[test]
fn staged_file_corruption_and_directory_substitution_fail_closed() {
    for replace_directory in [false, true] {
        let t = Tree::new();
        let req = t.request();
        let original = t.bytes();
        let root = t.0.clone();
        runtime().block_on(async {
            let c = reconcile_with_observer(t.owner(), req, move |b| {
                if b == Boundary::TemporarySynced {
                    if replace_directory {
                        fs::rename(root.join(".transflow"), root.join("original-transflow"))?;
                        fs::create_dir(root.join(".transflow"))?;
                        fs::write(
                            root.join(".transflow/catalog.toml"),
                            "format_version=1\n# substituted\n",
                        )?;
                    } else {
                        fs::write(
                            root.join(format!(".transflow/.catalog-{}.tmp", mutation_id())),
                            "corrupt temporary bytes",
                        )?;
                    }
                }
                Ok(())
            })
            .await
            .unwrap();
            assert!(c.outcome.is_err());
            drop(c.owner);
        });
        if replace_directory {
            assert_eq!(
                fs::read(t.0.join("original-transflow/catalog.toml")).unwrap(),
                original
            );
            assert_eq!(t.bytes(), b"format_version=1\n# substituted\n");
        } else {
            assert_eq!(t.bytes(), original);
        }
    }
}

#[test]
fn identical_tree_git_switch_and_explicit_ref_cannot_modify_checkout() {
    for fixed in [false, true] {
        let t = Tree::new();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&t.0)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&[
            "add",
            "workspace.toml",
            ".transflow/catalog.toml",
            "requirements.in",
            "requirements.lock",
            "src",
        ]);
        git(&[
            "-c",
            "user.name=Synthetic Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "initial",
        ]);
        let req = if fixed {
            let w = Workspace::load(&t.0, Some(&t.0)).unwrap();
            request_for_capture(
                tf_catalog::git::capture_ref(
                    &w,
                    "HEAD",
                    Some(&"main".parse().unwrap()),
                    CaptureLimits::default(),
                )
                .unwrap(),
            )
        } else {
            let req = t.request();
            git(&["checkout", "-b", "same-tree"]);
            req
        };
        let old = t.bytes();
        runtime().block_on(async {
            let c = reconcile(t.owner(), req).await.unwrap();
            assert!(c.outcome.is_err());
            let mut owner = c.owner;
            assert_eq!(state(&mut owner).await, None);
        });
        assert_eq!(t.bytes(), old);
    }
}

#[test]
fn replaced_runtime_lock_cannot_authorize_a_later_registry_rename() {
    let t = Tree::new();
    let req = t.request();
    let original = t.bytes();
    let root = t.0.clone();
    runtime().block_on(async {
        let c = reconcile_with_observer(t.owner(), req, move |b| {
            if b == Boundary::BeforeRename {
                fs::rename(
                    root.join(".transflow/runtime/lock"),
                    root.join(".transflow/runtime/old-lock"),
                )?;
                fs::write(root.join(".transflow/runtime/lock"), b"replacement lock")?;
            }
            Ok(())
        })
        .await
        .unwrap();
        assert!(c.outcome.is_err());
        drop(c.owner);
        assert_eq!(t.bytes(), original);
    });
}

#[test]
fn lifecycle_rename_and_tombstone_recover_exact_ids_without_allocations() {
    for remove in [false, true] {
        let tree = Tree::new();
        runtime().block_on(async {
            let completed = reconcile(tree.owner(), tree.request()).await.unwrap();
            completed.outcome.unwrap();
            let workspace = Workspace::load(&tree.0, Some(&tree.0)).unwrap();
            let capture = SourceSnapshot::capture(&workspace, CaptureLimits::default()).unwrap();
            let registry = RegistrySnapshot::parse(
                workspace_id(),
                std::str::from_utf8(&tree.bytes()).unwrap(),
            )
            .unwrap();
            let replacement = if remove {
                registry.render_remove(dataset_id()).unwrap()
            } else {
                registry
                    .render_rename(dataset_id(), &"curated/renamed".parse().unwrap(), true)
                    .unwrap()
            };
            let proposal = tf_catalog::candidate::RegistryProposal::lifecycle(
                &registry,
                capture.id().unwrap(),
                replacement.clone(),
            )
            .unwrap();
            assert!(proposal.assignments().is_empty());
            let id = RequestId::from_bytes([8; 16]);
            let stopped = reconcile_with_observer(
                completed.owner,
                ReconcileRequest {
                    capture,
                    proposal,
                    context: WriteContext::WorkingTree,
                    id,
                    at_us: 456,
                },
                |boundary| {
                    if boundary == Boundary::Renamed {
                        Err(io::Error::other(
                            "synthetic interruption after lifecycle rename",
                        ))
                    } else {
                        Ok(())
                    }
                },
            )
            .await
            .unwrap();
            assert!(stopped.outcome.is_err());
            let mut owner = stopped.owner;
            assert_eq!(
                recover(&mut owner, 789).await.unwrap(),
                vec![RecoveryOutcome::Indexed(id)]
            );
            assert_eq!(tree.bytes(), replacement.as_bytes());
            assert!(indexed(&mut owner).await);
            let current = RegistrySnapshot::parse(workspace_id(), &replacement).unwrap();
            assert_eq!(current.datasets().count(), 1);
            assert_eq!(
                current.datasets().next().unwrap().key().dataset_id(),
                dataset_id()
            );
            assert_eq!(current.datasets().next().unwrap().is_tombstone(), remove);
        });
    }
}

#[test]
fn external_add_and_remove_recover_exact_registration_after_interrupted_rename() {
    let tree = Tree::new();
    runtime().block_on(async {
        let provider = RegistrySnapshot::parse(
            WorkspaceId::from_bytes([7; 16]),
            &format!(
                "format_version=1\n[[datasets]]\nid='{}'\npath='raw/orders'\nkind='transform'\n",
                dataset_id()
            ),
        )
        .unwrap();
        let registration = tf_domain::ExternalRegistrationId::from_bytes([9; 16]);
        let mut owner = tree.owner();
        for remove in [false, true] {
            let workspace = Workspace::load(&tree.0, Some(&tree.0)).unwrap();
            let capture = SourceSnapshot::registration(&workspace, None).unwrap();
            let registry = RegistrySnapshot::parse(
                workspace_id(),
                std::str::from_utf8(&tree.bytes()).unwrap(),
            )
            .unwrap();
            let replacement = if remove {
                registry.render_external_remove(registration).unwrap()
            } else {
                registry
                    .render_external_add(
                        registration,
                        &"external/market/orders".parse().unwrap(),
                        provider.resolve_output("raw/orders").unwrap(),
                        &"master".parse().unwrap(),
                        None,
                    )
                    .unwrap()
            };
            let proposal = tf_catalog::candidate::RegistryProposal::lifecycle(
                &registry,
                capture.id().unwrap(),
                replacement.clone(),
            )
            .unwrap();
            let id = RequestId::from_bytes([if remove { 11 } else { 10 }; 16]);
            let stopped = reconcile_with_observer(
                owner,
                ReconcileRequest {
                    capture,
                    proposal,
                    context: WriteContext::WorkingTree,
                    id,
                    at_us: 123,
                },
                |boundary| {
                    if boundary == Boundary::Renamed {
                        Err(io::Error::other("synthetic interruption"))
                    } else {
                        Ok(())
                    }
                },
            )
            .await
            .unwrap();
            assert!(stopped.outcome.is_err());
            owner = stopped.owner;
            assert_eq!(
                recover(&mut owner, 456).await.unwrap(),
                vec![RecoveryOutcome::Indexed(id)]
            );
            assert_eq!(tree.bytes(), replacement.as_bytes());
            let current = RegistrySnapshot::parse(workspace_id(), &replacement).unwrap();
            let e = current.external_history().next().unwrap();
            assert_eq!(e.id(), registration);
            assert_eq!(e.is_tombstone(), remove);
            assert_eq!(e.key(), provider.resolve("raw/orders").unwrap().key());
        }
    });
}
