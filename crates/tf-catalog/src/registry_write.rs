//! Sibling-file replacement with captured source guards. SQLite orchestration lives above this crate.
use crate::{
    RegistrySnapshot,
    candidate::RegistryProposal,
    capture::{CaptureError, SourceSnapshot},
    workspace::Workspace,
};
use rustix::fs::{Mode, OFlags};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tf_domain::RequestId;
use tf_protocol::canonical::{ContentDigest, file_digest};

/// Safe summaries preserve detailed I/O causes without leaking file contents.
#[derive(Debug, thiserror::Error)]
pub enum RegistryWriteError {
    /// A source/registry/directory guard is stale.
    #[error("Source or registry changed; capture again before reconciling")]
    Conflict,
    /// Fixed source selections cannot update the user's checkout.
    #[error("This source selection needs registered IDs; sync and commit its registry first")]
    FixedSource,
    /// Filesystem durability could not be confirmed; consult the journal on recovery.
    #[error("The registry could not be durably reconciled")]
    Io(#[from] std::io::Error),
    /// Captured byte verification failed.
    #[error(transparent)]
    Capture(#[from] CaptureError),
}
/// Registry write result.
pub type Result<T> = std::result::Result<T, RegistryWriteError>;
/// An explicit current-working-tree authoring operation is distinct from retained execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteContext {
    /// Interactive preparation before execution, guarded against current bytes.
    WorkingTree,
    /// Historical, scheduled fixed-source or Git-ref execution; never edits a checkout.
    FixedSource,
}
/// Observable durability boundaries, also used by real process-kill tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Boundary {
    /// Durable intent exists; no temporary file has been written.
    JournalPrepared,
    /// Complete sibling temporary file is synced; original is still authoritative.
    TemporarySynced,
    /// Immediately before the final source and registry guard and rename.
    BeforeRename,
    /// Atomic rename returned; directory sync may not yet have happened.
    Renamed,
    /// Replacement and parent are durable.
    DirectorySynced,
    /// Immediately before the final guards and SQLite index transaction.
    BeforeIndex,
    /// IDs and completion evidence committed together.
    Indexed,
}
const LIMIT: u64 = 16 * 1024 * 1024;
fn directory(path: &Path) -> Result<File> {
    Ok(File::from(
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    ))
}
fn regular(dir: &File, name: &str) -> Result<File> {
    let file = File::from(
        rustix::fs::openat(
            dir,
            name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    if !file.metadata()?.is_file() {
        return Err(RegistryWriteError::Conflict);
    }
    Ok(file)
}
fn identity(f: &File) -> Result<(u64, u64)> {
    let m = f.metadata()?;
    Ok((m.dev(), m.ino()))
}
fn bytes(file: &mut File) -> Result<Vec<u8>> {
    let before = file.metadata()?;
    if before.len() > LIMIT {
        return Err(RegistryWriteError::Conflict);
    }
    let mut data = Vec::new();
    Read::by_ref(file).take(LIMIT + 1).read_to_end(&mut data)?;
    let after = file.metadata()?;
    if data.len() as u64 != before.len()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(RegistryWriteError::Conflict);
    }
    Ok(data)
}
/// Raw digest for bounded already-verified registry bytes.
pub fn registry_digest(data: &[u8]) -> Result<ContentDigest> {
    file_digest(&mut &*data).map_err(|_| RegistryWriteError::Conflict)
}
/// Descriptor-bound registry reader for reconciliation and recovery.
pub struct RegistryFile {
    root: PathBuf,
    root_fd: File,
    dir: File,
}
impl RegistryFile {
    /// Open the canonical owner root and a real `.transflow` directory without following symlinks.
    pub fn open(root: &Path) -> Result<Self> {
        let root_fd = directory(root)?;
        let dir = File::from(
            rustix::fs::openat(
                &root_fd,
                ".transflow",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        Ok(Self {
            root: root.to_owned(),
            root_fd,
            dir,
        })
    }
    fn validate(&self) -> Result<()> {
        if identity(&directory(&self.root)?)? != identity(&self.root_fd)?
            || identity(&directory(&self.root.join(".transflow"))?)? != identity(&self.dir)?
        {
            return Err(RegistryWriteError::Conflict);
        }
        Ok(())
    }
    /// Read a whole bounded regular registry and check descriptor/directory identity again.
    pub fn read(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut file = regular(&self.dir, "catalog.toml")?;
        let data = bytes(&mut file)?;
        if identity(&file)? != identity(&regular(&self.dir, "catalog.toml")?)? {
            return Err(RegistryWriteError::Conflict);
        }
        self.validate()?;
        Ok(data)
    }
    /// Confirm a byte guard without rewriting the author's file.
    pub fn verify(&self, expected: &ContentDigest) -> Result<()> {
        if registry_digest(&self.read()?)? != *expected {
            return Err(RegistryWriteError::Conflict);
        }
        Ok(())
    }
    /// Sync a recovered new registry and its parent before indexing it.
    pub fn sync(&self, expected: &ContentDigest) -> Result<()> {
        self.verify(expected)?;
        regular(&self.dir, "catalog.toml")?.sync_all()?;
        self.dir.sync_all()?;
        self.verify(expected)
    }
}
/// One proposal's filesystem half. Use on a blocking worker, while holding coordinator ownership.
/// Drop removes only a temporary file created by this object; the journal is never discarded.
pub struct RegistryWrite {
    file: RegistryFile,
    workspace: Workspace,
    capture: SourceSnapshot,
    proposal: RegistryProposal,
    temporary: String,
    staged: bool,
    mode: u32,
    new_digest: ContentDigest,
}
impl RegistryWrite {
    /// Check immutable and live context before even preparing a durable intent.
    pub fn prepare(
        root: &Path,
        capture: SourceSnapshot,
        proposal: RegistryProposal,
        id: RequestId,
        context: WriteContext,
    ) -> Result<Self> {
        if context != WriteContext::WorkingTree
            || capture.git().is_some_and(|g| g.requested_ref().is_some())
        {
            return Err(RegistryWriteError::FixedSource);
        }
        let workspace =
            Workspace::load(root, Some(root)).map_err(|_| RegistryWriteError::Conflict)?;
        if workspace.config().id() != proposal.workspace() || capture.id()? != proposal.source() {
            return Err(RegistryWriteError::Conflict);
        }
        if registry_digest(&capture.read(Path::new(".transflow/catalog.toml"), LIMIT)?)?
            != *proposal.expected_old()
        {
            return Err(RegistryWriteError::Conflict);
        }
        let file = RegistryFile::open(workspace.root())?;
        let mode = regular(&file.dir, "catalog.toml")?
            .metadata()?
            .permissions()
            .mode()
            & 0o777;
        let new_digest = registry_digest(proposal.replacement().as_bytes())?;
        let result = Self {
            file,
            workspace,
            capture,
            proposal,
            temporary: format!(".catalog-{id}.tmp"),
            staged: false,
            mode,
            new_digest,
        };
        result.guard(false)?;
        Ok(result)
    }
    /// Exact proposed bytes; caller journals this digest before staging.
    pub fn new_digest(&self) -> ContentDigest {
        self.new_digest
    }
    /// Stable local IDs in the complete replacement, including tombstones.
    pub fn index_ids(&self) -> Result<Vec<tf_domain::DatasetId>> {
        let registry =
            RegistrySnapshot::parse(self.proposal.workspace(), self.proposal.replacement())
                .map_err(|_| RegistryWriteError::Conflict)?;
        Ok(registry.datasets().map(|d| d.key().dataset_id()).collect())
    }
    /// Check source/config/lock membership and bytes, then the registry closest to the mutation.
    pub fn guard(&self, replaced: bool) -> Result<()> {
        self.capture
            .verify_working_copy(&self.workspace, replaced)?;
        self.file.verify(if replaced {
            &self.new_digest
        } else {
            self.proposal.expected_old()
        })
    }
    /// Create one exclusive same-directory temporary, preserve permissions, write and sync.
    pub fn stage(&mut self) -> Result<()> {
        self.file.validate()?;
        let mut temp = File::from(
            rustix::fs::openat(
                &self.file.dir,
                self.temporary.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?,
        );
        self.staged = true;
        temp.set_permissions(std::fs::Permissions::from_mode(self.mode))?;
        temp.write_all(self.proposal.replacement().as_bytes())?;
        temp.sync_all()?;
        Ok(())
    }
    /// Guard immediately before atomic rename. Non-cooperating editors do not share this lock.
    pub fn rename(&mut self) -> Result<()> {
        if !self.staged {
            return Err(RegistryWriteError::Conflict);
        }
        self.guard(false)?;
        let staged_bytes = bytes(&mut regular(&self.file.dir, self.temporary.as_str())?)?;
        if registry_digest(&staged_bytes)? != self.new_digest {
            return Err(RegistryWriteError::Conflict);
        }
        self.file.verify(self.proposal.expected_old())?;
        rustix::fs::renameat(
            &self.file.dir,
            self.temporary.as_str(),
            &self.file.dir,
            "catalog.toml",
        )
        .map_err(std::io::Error::from)?;
        self.staged = false;
        Ok(())
    }
    /// Flush directory entry and recheck the new bytes and all unchanged source inputs.
    pub fn sync(&self) -> Result<()> {
        self.file.sync(&self.new_digest)?;
        self.guard(true)
    }
}
impl Drop for RegistryWrite {
    fn drop(&mut self) {
        if self.staged {
            let _ = rustix::fs::unlinkat(
                &self.file.dir,
                self.temporary.as_str(),
                rustix::fs::AtFlags::empty(),
            );
        }
    }
}
