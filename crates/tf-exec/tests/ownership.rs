//! Real process/descriptor tests for runtime ownership and stale discovery.
#[allow(
    dead_code,
    reason = "Shared T007 harness supplies additional fixture types"
)]
#[path = "../../../tests/support/mod.rs"]
mod support;
use std::{
    error::Error,
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use support::filesystem::ScratchDirectory;
use tf_domain::WorkspaceId;
use tf_exec::ownership::*;
type Result = std::result::Result<(), Box<dyn Error>>;
fn workspace() -> WorkspaceId {
    WorkspaceId::from_bytes([11; 16])
}
fn fixture() -> std::io::Result<ScratchDirectory> {
    let root = ScratchDirectory::new()?;
    fs::create_dir_all(root.path().join(".transflow/runtime"))?;
    fs::set_permissions(
        root.path().join(".transflow/runtime"),
        fs::Permissions::from_mode(0o700),
    )?;
    Ok(root)
}
fn listener() -> std::io::Result<TcpListener> {
    TcpListener::bind("127.0.0.1:0")
}
struct OwnerChild {
    child: Child,
}
impl OwnerChild {
    fn start(root: &Path) -> std::result::Result<Self, Box<dyn Error>> {
        let child = Command::new(std::env::current_exe()?)
            .args(["--exact", "owner_child", "--nocapture"])
            .env("TF_OWNER_CHILD_ROOT", root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut this = Self { child };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.join("ready").exists() {
            if this.child.try_wait()?.is_some() || Instant::now() > deadline {
                return Err("Owner child failed to become ready".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(this)
    }
    fn stop(&mut self) -> std::io::Result<()> {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }
}
impl Drop for OwnerChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn owner_child() -> Result {
    let Some(root) = std::env::var_os("TF_OWNER_CHILD_ROOT") else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    let mut owner = RuntimeOwner::acquire(&root, workspace(), CoordinatorMode::Persistent)?;
    let listener = listener()?;
    owner.register_endpoint(&listener)?;
    fs::File::create(root.join("ready"))?.write_all(b"ready")?;
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte)?;
    Ok(())
}
#[test]
fn real_process_lock_blocks_second_owner_and_survives_stale_pid_metadata() -> Result {
    let root = fixture()?;
    let mut child = OwnerChild::start(root.path())?;
    assert!(matches!(
        RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Temporary),
        Err(OwnershipError::Owned)
    ));
    let registered = discover(root.path(), workspace())?.ok_or("Missing active owner")?;
    assert_eq!(registered.mode(), CoordinatorMode::Persistent);
    assert_eq!(registered.nonce().len(), 64);
    assert!(!format!("{registered:?}").contains(registered.nonce()));
    child.stop()?;
    assert!(discover(root.path(), workspace())?.is_none());
    let mut next = RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Temporary)?;
    let listener = listener()?;
    next.register_endpoint(&listener)?;
    let current = discover(root.path(), workspace())?.ok_or("Missing replacement")?;
    assert_ne!(current.session()?, registered.session()?);
    assert_ne!(current.nonce(), registered.nonce());
    Ok(())
}
#[test]
fn same_process_contenders_and_copied_clone_metadata_are_rejected() -> Result {
    let root = fixture()?;
    let clone = fixture()?;
    let mut owner = RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Persistent)?;
    let listener = listener()?;
    owner.register_endpoint(&listener)?;
    assert!(matches!(
        RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Temporary),
        Err(OwnershipError::Owned)
    ));
    fs::copy(
        root.path().join(".transflow/runtime/runtime.json"),
        clone.path().join(".transflow/runtime/runtime.json"),
    )?;
    assert!(discover(clone.path(), workspace())?.is_none());
    let _clone_owner =
        RuntimeOwner::acquire(clone.path(), workspace(), CoordinatorMode::Temporary)?;
    assert!(matches!(
        discover(clone.path(), workspace()),
        Err(OwnershipError::Invalid)
    ));
    assert!(matches!(
        discover(root.path(), WorkspaceId::from_bytes([22; 16])),
        Err(OwnershipError::Invalid)
    ));
    Ok(())
}
#[test]
fn discovery_rejects_stale_process_future_protocol_unknown_fields_and_remote_endpoint() -> Result {
    let root = fixture()?;
    let mut owner = RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Persistent)?;
    let listener = listener()?;
    owner.register_endpoint(&listener)?;
    let path = root.path().join(".transflow/runtime/runtime.json");
    let original = fs::read(&path)?;
    for (field, value) in [
        ("process_start", serde_json::json!("stale")),
        ("protocol_major", serde_json::json!(2)),
        ("format_version", serde_json::json!(2)),
        ("endpoint", serde_json::json!("192.0.2.1:1234")),
        ("nonce", serde_json::json!("short")),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut json: serde_json::Value = serde_json::from_slice(&original)?;
        json[field] = value;
        fs::write(&path, serde_json::to_vec(&json)?)?;
        assert!(discover(root.path(), workspace()).is_err());
    }
    fs::write(&path, vec![b' '; 17 * 1024])?;
    assert!(discover(root.path(), workspace()).is_err());
    fs::write(&path, &original)?;
    assert!(discover(root.path(), workspace())?.is_some());
    Ok(())
}
#[test]
fn temporary_and_metadata_modes_never_enable_schedules_or_no_wait() -> Result {
    for mode in [CoordinatorMode::Temporary, CoordinatorMode::MetadataOnly] {
        assert!(!mode.schedules_enabled());
        assert!(matches!(
            mode.require_persistent(),
            Err(OwnershipError::PersistentRequired)
        ));
        let root = fixture()?;
        let mut owner = RuntimeOwner::acquire(root.path(), workspace(), mode)?;
        let listener = listener()?;
        owner.register_endpoint(&listener)?;
        assert_eq!(
            discover(root.path(), workspace())?
                .ok_or("Missing registration")?
                .mode(),
            mode
        );
    }
    assert!(CoordinatorMode::Persistent.schedules_enabled());
    CoordinatorMode::Persistent.require_persistent()?;
    Ok(())
}
#[test]
fn unsafe_runtime_permissions_symlinks_and_replaced_lock_fail_closed() -> Result {
    let root = fixture()?;
    let runtime = root.path().join(".transflow/runtime");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755))?;
    assert!(RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Temporary).is_err());
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
    let target = root.path().join("untouched");
    fs::write(&target, b"keep")?;
    std::os::unix::fs::symlink(&target, runtime.join("lock"))?;
    assert!(RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Temporary).is_err());
    assert_eq!(fs::read(&target)?, b"keep");
    fs::remove_file(runtime.join("lock"))?;
    let mut owner = RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Temporary)?;
    fs::remove_file(runtime.join("lock"))?;
    fs::write(runtime.join("lock"), b"")?;
    fs::set_permissions(runtime.join("lock"), fs::Permissions::from_mode(0o600))?;
    assert!(owner.register_endpoint(&listener()?).is_err());
    Ok(())
}
#[test]
fn owned_store_closes_before_ownership_can_be_released() -> Result {
    let root = fixture()?;
    let mut owner = RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::MetadataOnly)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut store = owner.open_store().await?;
            store
                .repository()?
                .register_workspace(workspace(), "synthetic-root", 0)
                .await?;
            assert!(matches!(
                RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Temporary),
                Err(OwnershipError::Owned)
            ));
            store.close().await
        })?;
    drop(owner);
    let _next = RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Temporary)?;
    Ok(())
}

#[test]
fn endpoint_registration_requires_bound_loopback_and_keeps_metadata_private() -> Result {
    let root = fixture()?;
    let mut owner = RuntimeOwner::acquire(root.path(), workspace(), CoordinatorMode::Temporary)?;
    let wildcard = TcpListener::bind("0.0.0.0:0")?;
    assert!(owner.register_endpoint(&wildcard).is_err());
    assert!(!root.path().join(".transflow/runtime/runtime.json").exists());
    let local = listener()?;
    owner.register_endpoint(&local)?;
    assert_eq!(
        fs::metadata(root.path().join(".transflow/runtime/runtime.json"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        discover(root.path(), workspace())?
            .ok_or("Missing endpoint")?
            .endpoint(),
        local.local_addr()?
    );
    Ok(())
}
