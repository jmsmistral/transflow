//! Git-free durable captures. Copied bytes, never the later working tree, are execution authority.
use crate::{
    source::{SourceError, SourceIndex, hard_excluded},
    workspace::{ConfigError, Workspace, relative_path},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tf_domain::{SourceSnapshotId, WorkspaceId};
use tf_protocol::canonical::{DigestKind, content_digest, file_digest};

/// Capture failures do not expose file contents or machine-local secret locators.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// A file, enumeration or source configuration changed during capture.
    #[error("Source changed during capture; stop editing and retry")]
    Changed,
    /// Capture and manifest bounds are enforced independently of worker memory policy.
    #[error("Source capture exceeds its configured file, byte or manifest limit")]
    Limit,
    /// Missing/unreadable/unsafe filesystem state.
    #[error("Source capture could not safely access a required regular file or directory")]
    Io(#[from] std::io::Error),
    /// A retained manifest or file does not match its declared identity/digest.
    #[error("Retained source capture is incomplete, incompatible or corrupt")]
    Integrity,
    /// Workspace policy cannot be loaded.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Source allowlisting failed before copying.
    #[error(transparent)]
    Source(#[from] SourceError),
}
/// Explicit source-copy limits; these are not transform memory limits.
#[derive(Clone, Copy, Debug)]
pub struct CaptureLimits {
    /// Maximum size of any source/resource file.
    pub file_bytes: u64,
    /// Maximum combined copied source bytes.
    pub total_bytes: u64,
}
impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            file_bytes: 256 * 1024 * 1024,
            total_bytes: 1024 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    bytes: u64,
    sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format_version: u32,
    snapshot_id: String,
    workspace_id: String,
    source_roots: Vec<String>,
    files: Vec<Entry>,
    source_digest: String,
    git: Option<()>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Guard {
    device: u64,
    inode: u64,
    bytes: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
}
impl Guard {
    fn from(file: &File) -> Result<Self, CaptureError> {
        let m = file.metadata()?;
        if !m.is_file() {
            return Err(CaptureError::Integrity);
        }
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
            bytes: m.len(),
            mtime: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            ctime: m.ctime(),
            ctime_ns: m.ctime_nsec(),
        })
    }
}
/// Retained source bytes and validated identity. Every reopen verifies all manifest-listed bytes.
#[derive(Debug)]
pub struct SourceSnapshot {
    directory: PathBuf,
    manifest: Manifest,
}
impl SourceSnapshot {
    /// Copy configured sources plus workspace config, registry, dependency input and lock.
    /// The caller owns coordinator authority when used in a mutating application operation.
    /// No Git executable, Python import, package installation or database is involved.
    pub fn capture(workspace: &Workspace, limits: CaptureLimits) -> Result<Self, CaptureError> {
        Self::capture_with_observer(workspace, limits, |_| Ok(()))
    }
    /// Capture with a synchronous progress/cancellation observer after each copied file.
    /// Returning an I/O error aborts and removes only this capture's private staging directory.
    pub fn capture_with_observer(
        workspace: &Workspace,
        limits: CaptureLimits,
        mut observer: impl FnMut(&Path) -> std::io::Result<()>,
    ) -> Result<Self, CaptureError> {
        let before = SourceIndex::enumerate(workspace)?;
        let files = allowlist(workspace, &before)?;
        let source_root = open_directory(workspace.root())?;
        let runtime = workspace.root().join(".transflow/runtime");
        let _runtime = open_relative(&source_root, Path::new(".transflow/runtime"), true)?;
        let parent = runtime.join("source-snapshots");
        match fs::symlink_metadata(&parent) {
            Ok(m) if !m.is_dir() || m.file_type().is_symlink() => {
                return Err(CaptureError::Integrity);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new().mode(0o700).create(&parent)?
            }
            Err(e) => return Err(e.into()),
        }
        let id = random_id()?;
        let staging = parent.join(format!(".staging-{id}"));
        let final_path = parent.join(id.to_string());
        fs::DirBuilder::new().mode(0o700).create(&staging)?;
        let result = (|| {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(staging.join("files"))?;
            let mut entries = Vec::new();
            let guards: BTreeMap<_, _> = files
                .iter()
                .map(|relative| {
                    Ok((
                        relative.clone(),
                        Guard::from(&open_source(workspace, &source_root, relative)?)?,
                    ))
                })
                .collect::<Result<_, CaptureError>>()?;
            let mut total = 0u64;
            for relative in &files {
                let mut input = open_source(workspace, &source_root, relative)?;
                let guard = guards.get(relative).ok_or(CaptureError::Integrity)?;
                if &Guard::from(&input)? != guard {
                    return Err(CaptureError::Changed);
                }
                if guard.bytes > limits.file_bytes {
                    return Err(CaptureError::Limit);
                }
                let destination = staging.join("files").join(relative);
                let directory = destination.parent().ok_or(CaptureError::Integrity)?;
                fs::create_dir_all(directory)?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&destination)?;
                let copied = std::io::copy(
                    &mut Read::by_ref(&mut input).take(limits.file_bytes.saturating_add(1)),
                    &mut output,
                )?;
                total = total.checked_add(copied).ok_or(CaptureError::Limit)?;
                if copied > limits.file_bytes || total > limits.total_bytes {
                    return Err(CaptureError::Limit);
                }
                if copied != guard.bytes || &Guard::from(&input)? != guard {
                    return Err(CaptureError::Changed);
                }
                output.sync_all()?;
                drop(output);
                let digest = file_digest(&mut File::open(&destination)?)
                    .map_err(|_| CaptureError::Integrity)?
                    .hex();
                fs::set_permissions(&destination, fs::Permissions::from_mode(0o400))?;
                entries.push(Entry {
                    path: path_text(relative)?,
                    bytes: copied,
                    sha256: digest,
                });
                observer(relative)?;
            }
            let fresh = Workspace::load(workspace.root(), Some(workspace.root()))?;
            if fresh.config().id() != workspace.config().id()
                || SourceIndex::enumerate(&fresh)? != before
                || allowlist(&fresh, &before)? != files
            {
                return Err(CaptureError::Changed);
            }
            for (relative, guard) in guards {
                if Guard::from(&open_source(&fresh, &source_root, &relative)?)? != guard {
                    return Err(CaptureError::Changed);
                }
            }
            let mut manifest = Manifest {
                format_version: 1,
                snapshot_id: id.to_string(),
                workspace_id: workspace.config().id().to_string(),
                source_roots: before
                    .roots()
                    .iter()
                    .map(|p| path_text(p))
                    .collect::<Result<_, _>>()?,
                files: entries,
                source_digest: String::new(),
                git: None,
            };
            manifest.source_digest = source_digest(&manifest)?;
            let bytes = serde_json::to_vec(&manifest).map_err(|_| CaptureError::Integrity)?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(CaptureError::Limit);
            }
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o400)
                .open(staging.join("manifest.json"))?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            sync_directories(&staging)?;
            fs::rename(&staging, &final_path)?;
            File::open(&parent)?.sync_all()?;
            Ok(Self {
                directory: final_path,
                manifest,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }
    /// Open one exact retained ID and verify strict manifest, complete file set and all hashes.
    pub fn open(parent: &Path, id: SourceSnapshotId) -> Result<Self, CaptureError> {
        let directory = parent.join(id.to_string());
        let root = open_directory(&directory)?;
        let mut bytes = Vec::new();
        open_relative(&root, Path::new("manifest.json"), false)?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(CaptureError::Limit);
        }
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|_| CaptureError::Integrity)?;
        if manifest.format_version != 1
            || manifest.snapshot_id != id.to_string()
            || manifest.workspace_id.parse::<WorkspaceId>().is_err()
            || manifest.git.is_some()
            || manifest.files.len() > 100_004
            || source_digest(&manifest)? != manifest.source_digest
        {
            return Err(CaptureError::Integrity);
        }
        let file_root = open_relative(&root, Path::new("files"), true)?;
        let mut prior: Option<&str> = None;
        for entry in &manifest.files {
            if prior.is_some_and(|p| p >= entry.path.as_str()) {
                return Err(CaptureError::Integrity);
            }
            prior = Some(&entry.path);
            validate_capture_path(&entry.path)?;
            let mut file = open_relative(&file_root, Path::new(&entry.path), false)?;
            if file.metadata()?.len() != entry.bytes
                || file_digest(&mut file)
                    .map_err(|_| CaptureError::Integrity)?
                    .hex()
                    != entry.sha256
            {
                return Err(CaptureError::Integrity);
            }
        }
        // Reject added files/symlinks which could otherwise be imported without a manifest digest.
        let mut actual = Vec::new();
        retained_files(
            &directory.join("files"),
            Path::new(""),
            &mut actual,
            0,
            &mut 0,
        )?;
        actual.sort();
        if actual
            != manifest
                .files
                .iter()
                .map(|e| e.path.clone())
                .collect::<Vec<_>>()
        {
            return Err(CaptureError::Integrity);
        }
        let config = crate::workspace::WorkspaceConfig::parse(&crate::workspace::read_authoring(
            &directory.join("files/workspace.toml"),
        )?)?;
        if config.id().to_string() != manifest.workspace_id
            || config
                .source_roots()
                .iter()
                .map(|p| path_text(p))
                .collect::<Result<Vec<_>, _>>()?
                != manifest.source_roots
        {
            return Err(CaptureError::Integrity);
        }
        Ok(Self {
            directory,
            manifest,
        })
    }
    /// Opaque random identity independent of the content digest.
    pub fn id(&self) -> Result<SourceSnapshotId, CaptureError> {
        self.manifest
            .snapshot_id
            .parse()
            .map_err(|_| CaptureError::Integrity)
    }
    /// Source-purpose fingerprint over exact relative paths, sizes and copied file digests.
    pub fn digest(&self) -> &str {
        &self.manifest.source_digest
    }
    /// Directory containing immutable copied authoring files (never the live workspace).
    pub fn files_root(&self) -> PathBuf {
        self.directory.join("files")
    }
    /// Read an exact manifested file with a caller-specified bounded result size and hash check.
    pub fn read(&self, path: &Path, max_bytes: u64) -> Result<Vec<u8>, CaptureError> {
        let name = path_text(path)?;
        let entry = self
            .manifest
            .files
            .iter()
            .find(|e| e.path == name)
            .ok_or(CaptureError::Integrity)?;
        if entry.bytes > max_bytes {
            return Err(CaptureError::Limit);
        }
        let root = open_directory(&self.files_root())?;
        let mut file = open_relative(&root, path, false)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != entry.bytes
            || file_digest(&mut bytes.as_slice())
                .map_err(|_| CaptureError::Integrity)?
                .hex()
                != entry.sha256
        {
            return Err(CaptureError::Integrity);
        }
        Ok(bytes)
    }
}
fn random_id() -> Result<SourceSnapshotId, CaptureError> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    Ok(SourceSnapshotId::from_bytes(bytes))
}
fn path_text(path: &Path) -> Result<String, CaptureError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(CaptureError::Integrity)
}
fn allowlist(workspace: &Workspace, index: &SourceIndex) -> Result<Vec<PathBuf>, CaptureError> {
    let mut files: Vec<_> = index.files().iter().map(|f| f.path().to_owned()).collect();
    let (input, lock) = workspace.config().dependency_paths();
    files.extend([
        PathBuf::from("workspace.toml"),
        PathBuf::from(".transflow/catalog.toml"),
        PathBuf::from(input),
        PathBuf::from(lock),
    ]);
    files.sort();
    files.dedup();
    for path in &files {
        validate_capture_path(&path_text(path)?)?;
    }
    Ok(files)
}
fn validate_capture_path(path: &str) -> Result<(), CaptureError> {
    if path == ".transflow/catalog.toml" {
        return Ok(());
    }
    relative_path(path)?;
    if hard_excluded(Path::new(path)) {
        return Err(CaptureError::Integrity);
    }
    Ok(())
}
fn source_digest(manifest: &Manifest) -> Result<String, CaptureError> {
    content_digest(DigestKind::Source,&serde_json::json!({"capture_format":1,"workspace_id":manifest.workspace_id,"source_roots":manifest.source_roots,"files":manifest.files})).map(|d|d.hex()).map_err(|_|CaptureError::Integrity)
}
fn open_directory(path: &Path) -> Result<File, CaptureError> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    Ok(File::from(fd))
}
fn open_relative(root: &File, path: &Path, directory: bool) -> Result<File, CaptureError> {
    let parts: Vec<_> = path.components().collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
    {
        return Err(CaptureError::Integrity);
    }
    let mut parent = root.try_clone()?;
    for (i, part) in parts.iter().enumerate() {
        let is_dir = directory || i + 1 < parts.len();
        let mut flags = rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC;
        if is_dir {
            flags |= rustix::fs::OFlags::DIRECTORY;
        }
        parent = File::from(
            rustix::fs::openat(&parent, part.as_os_str(), flags, rustix::fs::Mode::empty())
                .map_err(std::io::Error::from)?,
        );
    }
    if !directory && !parent.metadata()?.is_file() {
        return Err(CaptureError::Integrity);
    }
    Ok(parent)
}
fn open_source(workspace: &Workspace, root: &File, relative: &Path) -> Result<File, CaptureError> {
    let absolute = workspace.root().join(relative).canonicalize()?;
    let resolved = absolute
        .strip_prefix(workspace.root())
        .map_err(|_| CaptureError::Changed)?;
    validate_capture_path(&path_text(resolved)?)?;
    open_relative(root, resolved, false)
}
fn sync_directories(path: &Path) -> Result<(), CaptureError> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_directories(&entry.path())?;
        }
    }
    File::open(path)?.sync_all()?;
    Ok(())
}
fn retained_files(
    root: &Path,
    relative: &Path,
    files: &mut Vec<String>,
    depth: usize,
    count: &mut usize,
) -> Result<(), CaptureError> {
    *count += 1;
    if depth > 68 || files.len() > 100_004 || *count > 200_008 {
        return Err(CaptureError::Limit);
    }
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = relative.join(entry.file_name());
        if kind.is_symlink() {
            return Err(CaptureError::Integrity);
        }
        if kind.is_dir() {
            retained_files(root, &path, files, depth + 1, count)?;
        } else if kind.is_file() {
            files.push(path_text(&path)?);
        } else {
            return Err(CaptureError::Integrity);
        }
    }
    Ok(())
}
