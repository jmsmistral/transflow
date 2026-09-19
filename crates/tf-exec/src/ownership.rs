//! OS-held runtime ownership and bounded, authenticated-session discovery metadata.
use rustix::fs::{Mode, OFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use tf_domain::{CoordinatorSessionId, WorkspaceId};

/// Ownership/discovery failures never echo raw metadata or the secret nonce.
#[derive(Debug, thiserror::Error)]
pub enum OwnershipError {
    /// Another live process owns the runtime lock.
    #[error("This workspace runtime already has a coordinator")]
    Owned,
    /// Filesystem or OS process query failed.
    #[error("Coordinator ownership could not be established")]
    Io(#[from] std::io::Error),
    /// Files, permissions, roots or process identity are inconsistent.
    #[error("Runtime discovery metadata or filesystem identity is invalid")]
    Invalid,
    /// Future metadata/protocol cannot be attached by this binary.
    #[error("The active coordinator uses an incompatible runtime protocol")]
    Incompatible,
    /// Only explicit persistent serve can support unattended work.
    #[error("This operation requires a persistent coordinator")]
    PersistentRequired,
    /// Store creation failed after ownership acquisition.
    #[error("The owned runtime database could not be opened")]
    Store(#[from] tf_store::StoreError),
}
/// Ownership result.
pub type Result<T> = std::result::Result<T, OwnershipError>;
/// Scheduling is enabled only by explicit persistent serving.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorMode {
    /// Foreground persistent serve may activate schedules.
    Persistent,
    /// A single mutating command, without unattended schedules.
    Temporary,
    /// Provider read/export leases only; no imports, producers or schedules.
    MetadataOnly,
}
impl CoordinatorMode {
    /// Whether the coordinator may activate persisted schedules.
    pub fn schedules_enabled(self) -> bool {
        self == Self::Persistent
    }
    /// Fail before mutation for a no-wait request without persistent service.
    pub fn require_persistent(self) -> Result<()> {
        if self == Self::Persistent {
            Ok(())
        } else {
            Err(OwnershipError::PersistentRequired)
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Location {
    path: PathBuf,
    device: u64,
    inode: u64,
}
impl Location {
    fn inspect(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let m = fs::metadata(&path)?;
        if !m.is_dir() {
            return Err(OwnershipError::Invalid);
        }
        Ok(Self {
            path,
            device: m.dev(),
            inode: m.ino(),
        })
    }
}
/// A discovery candidate, not a successful authenticated transport attachment.
/// The eventual API handshake must prove this exact session nonce before mutation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    format_version: u32,
    protocol_major: u32,
    protocol_minor: u32,
    workspace_id: String,
    root: Location,
    runtime: Location,
    pid: u32,
    process_start: String,
    session: String,
    nonce: String,
    endpoint: SocketAddr,
    mode: CoordinatorMode,
}
impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("pid", &self.pid)
            .field("endpoint", &self.endpoint)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}
impl Registration {
    /// Loopback address registered from the owner's bound listener.
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
    /// Scheduling/provider mode of the live candidate.
    pub fn mode(&self) -> CoordinatorMode {
        self.mode
    }
    /// Random session identity, independent of PID reuse.
    pub fn session(&self) -> Result<CoordinatorSessionId> {
        self.session.parse().map_err(|_| OwnershipError::Invalid)
    }
    /// Secret proof for the future authenticated local handshake; never log this value.
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
}
/// Non-cloneable lock guard. The OS releases ownership even when the process is killed.
/// Runtime/lock files remain on disk; stale metadata is not evidence of a live owner.
pub struct RuntimeOwner {
    lock: File,
    directory: File,
    registration: Registration,
}
impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        // Explicit unlock releases the open-file-description lock even if a concurrent
        // fork inherited a descriptor before CLOEXEC closes it in the child.
        // Drop cannot report errors; closing the descriptor remains the fallback.
        let _ = self.lock.unlock();
    }
}
fn held_else_release(file: &File) -> Result<bool> {
    if lock(file)? {
        file.unlock()?;
        Ok(false)
    } else {
        Ok(true)
    }
}
fn io(error: rustix::io::Errno) -> OwnershipError {
    std::io::Error::from(error).into()
}
fn open_at(directory: &File, name: &str, flags: OFlags) -> Result<File> {
    Ok(File::from(
        rustix::fs::openat(
            directory,
            name,
            flags | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io)?,
    ))
}
fn regular(file: &File) -> Result<()> {
    let m = file.metadata()?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != rustix::process::geteuid().as_raw()
        || m.mode() & 0o077 != 0
    {
        return Err(OwnershipError::Invalid);
    }
    Ok(())
}
fn directory(root: &Path) -> Result<(Location, Location, File)> {
    let root = Location::inspect(root)?;
    let authoring = root.path.join(".transflow");
    let runtime = authoring.join("runtime");
    for p in [&authoring, &runtime] {
        if fs::symlink_metadata(p)?.file_type().is_symlink() {
            return Err(OwnershipError::Invalid);
        }
    }
    let location = Location::inspect(&runtime)?;
    let fd = File::from(
        rustix::fs::open(
            &runtime,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?,
    );
    let m = fd.metadata()?;
    if m.uid() != rustix::process::geteuid().as_raw()
        || m.mode() & 0o077 != 0
        || m.dev() != location.device
        || m.ino() != location.inode
    {
        return Err(OwnershipError::Invalid);
    }
    Ok((root, location, fd))
}
fn lock(file: &File) -> Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}
fn process_start(pid: u32) -> Result<String> {
    if pid == 0 {
        return Err(OwnershipError::Invalid);
    }
    #[cfg(target_os = "linux")]
    {
        let mut stat = String::new();
        File::open(format!("/proc/{pid}/stat"))?
            .take(8192)
            .read_to_string(&mut stat)?;
        let (_, fields) = stat.rsplit_once(") ").ok_or(OwnershipError::Invalid)?;
        let ticks = fields
            .split_whitespace()
            .nth(19)
            .ok_or(OwnershipError::Invalid)?;
        let _: u64 = ticks.parse().map_err(|_| OwnershipError::Invalid)?;
        let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        Ok(format!("{}:{ticks}", boot.trim()))
    }
    #[cfg(target_os = "macos")]
    {
        // ps is the OS interface here; arguments contain only a validated numeric PID.
        // Its one-second resolution is supplemented by the OS lock and random session proof.
        let output = std::process::Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .env("LC_ALL", "C")
            .output()?;
        if !output.status.success() || output.stdout.len() > 256 {
            return Err(OwnershipError::Invalid);
        }
        let start = std::str::from_utf8(&output.stdout)
            .map_err(|_| OwnershipError::Invalid)?
            .trim();
        if start.is_empty() {
            return Err(OwnershipError::Invalid);
        }
        Ok(start.to_owned())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(OwnershipError::Invalid)
    }
}
impl RuntimeOwner {
    /// Explicit canonical workspace associated with this live ownership guard.
    pub fn workspace_root(&self) -> &Path {
        &self.registration.root.path
    }
    /// Validated workspace identity fenced by this owner.
    pub fn workspace_id(&self) -> Result<WorkspaceId> {
        self.registration
            .workspace_id
            .parse()
            .map_err(|_| OwnershipError::Invalid)
    }
    /// Acquire ownership of an existing private `.transflow/runtime` directory.
    /// No DB, discovery import, process signal or metadata replacement happens on contention.
    pub fn acquire(root: &Path, workspace: WorkspaceId, mode: CoordinatorMode) -> Result<Self> {
        let (root, runtime, directory) = directory(root)?;
        let lock_file = open_at(&directory, "lock", OFlags::RDWR | OFlags::CREATE)?;
        regular(&lock_file)?;
        if !lock(&lock_file)? {
            return Err(OwnershipError::Owned);
        }
        let mut random = [0u8; 48];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let mut id = [0; 16];
        id.copy_from_slice(&random[..16]);
        id[6] = (id[6] & 0x0f) | 0x40;
        id[8] = (id[8] & 0x3f) | 0x80;
        let nonce = random[16..].iter().map(|b| format!("{b:02x}")).collect();
        let registration = Registration {
            format_version: 1,
            protocol_major: 1,
            protocol_minor: 0,
            workspace_id: workspace.to_string(),
            root,
            runtime,
            pid: std::process::id(),
            process_start: process_start(std::process::id())?,
            session: CoordinatorSessionId::from_bytes(id).to_string(),
            nonce,
            endpoint: SocketAddr::from(([127, 0, 0, 1], 0)),
            mode,
        };
        Ok(Self {
            lock: lock_file,
            directory,
            registration,
        })
    }
    fn validate_paths(&self) -> Result<()> {
        let root = Location::inspect(&self.registration.root.path)?;
        let runtime = Location::inspect(&self.registration.root.path.join(".transflow/runtime"))?;
        let file = open_at(&self.directory, "lock", OFlags::RDONLY)?;
        regular(&file)?;
        let a = file.metadata()?;
        let b = self.lock.metadata()?;
        if root != self.registration.root
            || runtime != self.registration.runtime
            || a.dev() != b.dev()
            || a.ino() != b.ino()
        {
            return Err(OwnershipError::Invalid);
        }
        Ok(())
    }
    /// Register a real already-bound loopback listener by durable atomic metadata replacement.
    pub fn register_endpoint(&mut self, listener: &TcpListener) -> Result<()> {
        self.validate_paths()?;
        let endpoint = listener.local_addr()?;
        if !endpoint.ip().is_loopback() || endpoint.port() == 0 {
            return Err(OwnershipError::Invalid);
        }
        let mut registration = self.registration.clone();
        registration.endpoint = endpoint;
        let bytes = serde_json::to_vec(&registration).map_err(|_| OwnershipError::Invalid)?;
        let name = format!("runtime-{}.tmp", registration.session);
        let mut temp = open_at(
            &self.directory,
            &name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
        )?;
        let result = (|| -> Result<()> {
            temp.write_all(&bytes)?;
            temp.sync_all()?;
            self.validate_paths()?;
            rustix::fs::renameat(&self.directory, &name, &self.directory, "runtime.json")
                .map_err(io)?;
            self.directory.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&self.directory, &name, rustix::fs::AtFlags::empty());
        }
        result?;
        self.registration = registration;
        Ok(())
    }
    /// Owner's current registration; an endpoint is available after register_endpoint.
    pub fn registration(&self) -> &Registration {
        &self.registration
    }
    /// Open the sole owned store. Its mutable borrow keeps this guard alive and prevents a second open.
    pub async fn open_store(&mut self) -> Result<OwnedStore<'_>> {
        self.validate_paths()?;
        let store =
            tf_store::Store::open(&self.registration.runtime.path.join("catalog.sqlite")).await?;
        Ok(OwnedStore { store, owner: self })
    }
}
/// Database lifetime tied to its OS-held ownership guard.
pub struct OwnedStore<'a> {
    store: tf_store::Store,
    owner: &'a mut RuntimeOwner,
}
impl OwnedStore<'_> {
    /// Access the serialized repository after revalidating directory/lock identity.
    pub fn repository(&mut self) -> Result<&mut tf_store::Store> {
        self.owner.validate_paths()?;
        Ok(&mut self.store)
    }
    /// Close the database before releasing the borrowed ownership guard.
    pub async fn close(self) -> Result<()> {
        self.store.close().await?;
        Ok(())
    }
}
/// Inspect only this explicit local runtime; never search globally by durable workspace UUID.
/// Returns None for an unlocked runtime, even when a stale PID/endpoint file exists.
/// A returned candidate still needs the local API's session-authenticated handshake.
pub fn discover(root: &Path, workspace: WorkspaceId) -> Result<Option<Registration>> {
    let (root, runtime, directory) = directory(root)?;
    let lock_file = match open_at(&directory, "lock", OFlags::RDONLY) {
        Ok(f) => f,
        Err(OwnershipError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    regular(&lock_file)?;
    if !held_else_release(&lock_file)? {
        return Ok(None);
    }
    let metadata = open_at(&directory, "runtime.json", OFlags::RDONLY)?;
    regular(&metadata)?;
    if metadata.metadata()?.len() > 16 * 1024 {
        return Err(OwnershipError::Invalid);
    }
    let mut bytes = Vec::new();
    metadata.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Err(OwnershipError::Invalid);
    }
    let registration: Registration =
        serde_json::from_slice(&bytes).map_err(|_| OwnershipError::Invalid)?;
    if registration.format_version != 1
        || registration.protocol_major != 1
        || registration.protocol_minor != 0
    {
        return Err(OwnershipError::Incompatible);
    }
    if registration.workspace_id != workspace.to_string()
        || registration.root != root
        || registration.runtime != runtime
        || registration.nonce.len() != 64
        || !registration
            .nonce
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || !registration.endpoint.ip().is_loopback()
        || registration.endpoint.port() == 0
        || registration.process_start != process_start(registration.pid)?
    {
        return Err(OwnershipError::Invalid);
    }
    registration.session()?;
    let current = open_at(&directory, "lock", OFlags::RDONLY)?;
    let a = current.metadata()?;
    let b = lock_file.metadata()?;
    if a.dev() != b.dev() || a.ino() != b.ino() || !held_else_release(&lock_file)? {
        return Err(OwnershipError::Invalid);
    }
    Ok(Some(registration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
    };

    #[test]
    fn explicit_unlock_releases_a_descriptor_inherited_by_a_live_child()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("tf-inherited-lock-{}", std::process::id()));
        fs::create_dir(&root)?;
        struct Root(PathBuf);
        impl Drop for Root {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _root = Root(root.clone());
        fs::create_dir_all(root.join(".transflow/runtime"))?;
        fs::set_permissions(
            root.join(".transflow/runtime"),
            fs::Permissions::from_mode(0o700),
        )?;
        let owner = RuntimeOwner::acquire(
            &root,
            WorkspaceId::from_bytes([1; 16]),
            CoordinatorMode::Temporary,
        )?;
        // Child deliberately retains a duplicate of the SAME locked file description
        // as stdout, while waiting for input. This makes the fork inheritance window
        // deterministic without unsafe fork calls or timing-dependent sleeps.
        let child = Command::new("/bin/sh")
            .args(["-c", "printf ready >&2; read value"])
            .stdin(Stdio::piped())
            .stdout(Stdio::from(owner.lock.try_clone()?))
            .stderr(Stdio::piped())
            .spawn()?;
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Child(child);
        let stderr = child
            .0
            .stderr
            .take()
            .ok_or("Missing child readiness pipe")?;
        let mut ready = std::io::BufReader::new(stderr);
        let mut signal = [0; 5];
        ready.read_exact(&mut signal)?;
        assert_eq!(&signal, b"ready");
        assert!(child.0.try_wait()?.is_none());
        drop(owner);
        let _replacement = RuntimeOwner::acquire(
            &root,
            WorkspaceId::from_bytes([1; 16]),
            CoordinatorMode::Temporary,
        )?;
        assert!(child.0.try_wait()?.is_none());
        Ok(())
    }
}
