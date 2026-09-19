//! Copy-only Parquet import staging. These files are not published dataset versions.
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use rustix::fs::{AtFlags, Mode, OFlags};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};
use tf_domain::RequestId;
use tf_protocol::canonical::{canonical_json, file_digest};

/// Staging rejects incomplete or changing external data rather than sealing it as valid.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// Filesystem/copy error.
    #[error("Import files could not be copied into private staging")]
    Io(#[from] std::io::Error),
    /// Explicit containment or state guard failure.
    #[error("Import source or staging changed; provide a stable producer snapshot and retry")]
    Changed,
    /// Unsupported/corrupt Parquet input.
    #[error(
        "Import requires valid compatible Parquet files (uncompressed, Snappy or Zstandard); schema normalization remains a later stage"
    )]
    Parquet,
    /// Bounded manifest/metadata requirements.
    #[error("Import metadata exceeds its limit or has an unsupported shape")]
    Metadata,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Guard {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
}
impl Guard {
    fn read(file: &File) -> Result<Self, ImportError> {
        let m = file.metadata()?;
        if !m.is_file() {
            return Err(ImportError::Changed);
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            mtime: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            ctime: m.ctime(),
            ctime_ns: m.ctime_nsec(),
        })
    }
}
fn dir(parent: &File, name: &str, create: bool) -> Result<File, ImportError> {
    if create {
        match rustix::fs::mkdirat(parent, name, Mode::RWXU) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(e) => return Err(std::io::Error::from(e).into()),
        }
    }
    Ok(File::from(
        rustix::fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    ))
}
fn source(root: &File, path: &Path) -> Result<File, ImportError> {
    let parts: Vec<_> = path.components().collect();
    if parts.is_empty() || parts.iter().any(|p| !matches!(p, Component::Normal(_))) {
        return Err(ImportError::Changed);
    }
    let mut parent = root.try_clone()?;
    for part in &parts[..parts.len() - 1] {
        parent = dir(
            &parent,
            part.as_os_str().to_str().ok_or(ImportError::Changed)?,
            false,
        )?;
    }
    Ok(File::from(
        rustix::fs::openat(
            &parent,
            parts.last().ok_or(ImportError::Changed)?.as_os_str(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    ))
}
fn digest(file: &mut File, limit: u64) -> Result<String, ImportError> {
    file.rewind()?;
    file_digest(&mut file.take(limit.saturating_add(1)))
        .map(|d| d.hex())
        .map_err(|_| ImportError::Changed)
}
struct Copied {
    source: PathBuf,
    guard: Guard,
    name: String,
    digest: String,
    rows: u64,
}
/// Private copied files whose final manifest has not yet been accepted.
/// Dropping an unretained stage removes only its own descriptor-bound files.
pub struct PreparedFiles {
    workspace: PathBuf,
    chain: Vec<File>,
    source: File,
    directory: File,
    name: String,
    id: RequestId,
    copied: Vec<Copied>,
    created: Vec<String>,
    retained: bool,
}
impl PreparedFiles {
    /// Copy a frozen list through no-follow descriptor-relative paths, then decode all rows.
    /// The caller holds exclusive runtime ownership and verifies the selection-root identity.
    pub fn copy(
        workspace: &Path,
        source_root: &File,
        files: &[PathBuf],
        id: RequestId,
        mut observer: impl FnMut(usize) -> std::io::Result<()>,
    ) -> Result<Self, ImportError> {
        if files.is_empty() || files.len() > 10_000 || files.windows(2).any(|w| w[0] >= w[1]) {
            return Err(ImportError::Metadata);
        }
        if workspace.canonicalize()? != workspace {
            return Err(ImportError::Changed);
        }
        let mut chain = vec![File::from(
            rustix::fs::open(
                workspace,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        )];
        for name in [".transflow", "runtime", "import-staging"] {
            chain.push(dir(
                chain.last().ok_or(ImportError::Changed)?,
                name,
                name == "import-staging",
            )?);
        }
        let name = format!(".staging-{id}");
        let parent = chain.last().ok_or(ImportError::Changed)?;
        rustix::fs::mkdirat(parent, name.as_str(), Mode::RWXU).map_err(std::io::Error::from)?;
        let directory = dir(parent, &name, false)?;
        let mut stage = Self {
            workspace: workspace.into(),
            chain,
            source: source_root.try_clone()?,
            directory,
            name,
            id,
            copied: vec![],
            created: vec![],
            retained: false,
        };
        let mut schema = None;
        for (index, path) in files.iter().enumerate() {
            stage.guard()?;
            let mut input = source(&stage.source, path)?;
            let guard = Guard::read(&input)?;
            let name = format!("part-{index:05}.parquet");
            let mut output = File::from(
                rustix::fs::openat(
                    &stage.directory,
                    name.as_str(),
                    OFlags::RDWR
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )
                .map_err(std::io::Error::from)?,
            );
            stage.created.push(name.clone());
            let bytes = std::io::copy(
                &mut Read::by_ref(&mut input).take(guard.len.saturating_add(1)),
                &mut output,
            )?;
            if bytes != guard.len || Guard::read(&input)? != guard {
                return Err(ImportError::Changed);
            }
            output.sync_all()?;
            let hash = digest(&mut output, guard.len)?;
            if digest(&mut input, guard.len)? != hash
                || Guard::read(&input)? != guard
                || Guard::read(&source(&stage.source, path)?)? != guard
            {
                return Err(ImportError::Changed);
            }
            // Bound the footer before handing metadata to the qualified Parquet decoder.
            if guard.len < 12 {
                return Err(ImportError::Parquet);
            }
            output.seek(SeekFrom::End(-8))?;
            let mut footer = [0; 8];
            output.read_exact(&mut footer)?;
            let metadata =
                u32::from_le_bytes(footer[..4].try_into().map_err(|_| ImportError::Parquet)?)
                    as u64;
            if &footer[4..] != b"PAR1" || metadata > 16 * 1024 * 1024 || metadata > guard.len - 12 {
                return Err(ImportError::Parquet);
            }
            output.rewind()?;
            let builder = ParquetRecordBatchReaderBuilder::try_new(output.try_clone()?)
                .map_err(|_| ImportError::Parquet)?;
            if schema
                .as_ref()
                .is_some_and(|expected| expected != builder.schema())
            {
                return Err(ImportError::Parquet);
            }
            schema = Some(builder.schema().clone());
            let expected = u64::try_from(builder.metadata().file_metadata().num_rows())
                .map_err(|_| ImportError::Parquet)?;
            let mut rows = 0u64;
            for batch in builder
                .with_batch_size(8192)
                .build()
                .map_err(|_| ImportError::Parquet)?
            {
                rows = rows
                    .checked_add(batch.map_err(|_| ImportError::Parquet)?.num_rows() as u64)
                    .ok_or(ImportError::Metadata)?;
            }
            if rows != expected || digest(&mut output, guard.len)? != hash {
                return Err(ImportError::Parquet);
            }
            stage.copied.push(Copied {
                source: path.clone(),
                guard,
                name,
                digest: hash,
                rows,
            });
            observer(index)?;
        }
        stage.verify_sources()?;
        stage.directory.sync_all()?;
        stage.guard()?;
        Ok(stage)
    }
    fn guard(&self) -> Result<(), ImportError> {
        let mut current = File::from(
            rustix::fs::open(
                &self.workspace,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        for (i, old) in self.chain.iter().enumerate() {
            let a = old.metadata()?;
            let b = current.metadata()?;
            if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
                return Err(ImportError::Changed);
            }
            if let Some(name) = [".transflow", "runtime", "import-staging"].get(i) {
                current = dir(&current, name, false)?;
            }
        }
        let current = dir(&current, &self.name, false)?;
        let a = current.metadata()?;
        let b = self.directory.metadata()?;
        if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
            return Err(ImportError::Changed);
        }
        Ok(())
    }
    /// Recheck frozen files, never the glob. Atomic producer snapshots provide stronger consistency.
    pub fn verify_sources(&self) -> Result<(), ImportError> {
        for copied in &self.copied {
            let mut input = source(&self.source, &copied.source)?;
            if Guard::read(&input)? != copied.guard
                || digest(&mut input, copied.guard.len)? != copied.digest
                || Guard::read(&input)? != copied.guard
            {
                return Err(ImportError::Changed);
            }
        }
        self.guard()
    }
    /// Recheck copied bytes and exact membership before retaining metadata.
    pub fn verify_copies(&self) -> Result<(), ImportError> {
        let mut names = std::collections::BTreeSet::new();
        for entry in rustix::fs::Dir::read_from(&self.directory).map_err(std::io::Error::from)? {
            let entry = entry.map_err(std::io::Error::from)?;
            let name = entry
                .file_name()
                .to_str()
                .map_err(|_| ImportError::Changed)?;
            if name != "." && name != ".." {
                names.insert(name.to_owned());
            }
        }
        if names != self.created.iter().cloned().collect() {
            return Err(ImportError::Changed);
        }
        for copied in &self.copied {
            let mut file = source(&self.directory, Path::new(&copied.name))?;
            if Guard::read(&file)?.len != copied.guard.len
                || digest(&mut file, copied.guard.len)? != copied.digest
            {
                return Err(ImportError::Changed);
            }
        }
        self.guard()
    }
    /// Exact copied manifest entries, ready for later normalization/check/publication services.
    pub fn files(&self) -> Value {
        json!(self.copied.iter().map(|f|json!({"path":f.name,"sha256":f.digest,"byte_length":f.guard.len.to_string(),"row_count":f.rows.to_string()})).collect::<Vec<_>>())
    }
    /// Total decoded rows; a valid zero-row Parquet input is allowed.
    pub fn rows(&self) -> Result<u64, ImportError> {
        self.copied.iter().try_fold(0u64, |n, f| {
            n.checked_add(f.rows).ok_or(ImportError::Metadata)
        })
    }
    /// Total physical bytes copied, independently of row counts.
    pub fn bytes(&self) -> Result<u64, ImportError> {
        self.copied.iter().try_fold(0u64, |n, f| {
            n.checked_add(f.guard.len).ok_or(ImportError::Metadata)
        })
    }
    /// Write the closed preparation manifest and atomically retain its private directory.
    /// No artifact, version, data branch or head is published here.
    pub fn retain(mut self, manifest: &Value) -> Result<PathBuf, ImportError> {
        tf_protocol::validate_document("ImportStagingManifestV1", manifest)
            .map_err(|_| ImportError::Metadata)?;
        if manifest["import_id"] != self.id.to_string() || manifest["files"] != self.files() {
            return Err(ImportError::Metadata);
        }
        if manifest["source_files"]
            != json!(
                self.copied
                    .iter()
                    .map(|file| &file.source)
                    .collect::<Vec<_>>()
            )
        {
            return Err(ImportError::Metadata);
        }
        manifest["branch"]
            .as_str()
            .ok_or(ImportError::Metadata)?
            .parse::<tf_domain::BranchName>()
            .map_err(|_| ImportError::Metadata)?;
        tf_domain::DatasetPath::parse_for_scope(
            manifest["path"].as_str().ok_or(ImportError::Metadata)?,
            tf_domain::DatasetScope::Local,
        )
        .map_err(|_| ImportError::Metadata)?;
        let root = Path::new(
            manifest["source_root"]
                .as_str()
                .ok_or(ImportError::Metadata)?,
        );
        if root.canonicalize()? != root {
            return Err(ImportError::Changed);
        }
        let current = File::from(
            rustix::fs::open(
                root,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        )
        .metadata()?;
        let original = self.source.metadata()?;
        if (current.dev(), current.ino()) != (original.dev(), original.ino()) {
            return Err(ImportError::Changed);
        }
        self.verify_sources()?;
        self.verify_copies()?;
        let bytes = canonical_json(manifest).map_err(|_| ImportError::Metadata)?;
        let mut file = File::from(
            rustix::fs::openat(
                &self.directory,
                "prepared.json",
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?,
        );
        self.created.push("prepared.json".into());
        file.write_all(&bytes)?;
        file.sync_all()?;
        self.directory.sync_all()?;
        self.verify_copies()?;
        self.guard()?;
        let parent = self.chain.last().ok_or(ImportError::Changed)?;
        let final_name = self.id.to_string();
        match rustix::fs::statat(parent, final_name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => {}
            _ => return Err(ImportError::Changed),
        }
        rustix::fs::renameat(parent, self.name.as_str(), parent, final_name.as_str())
            .map_err(std::io::Error::from)?;
        self.name = final_name;
        parent.sync_all()?;
        self.guard()?;
        self.retained = true;
        Ok(self
            .workspace
            .join(".transflow/runtime/import-staging")
            .join(&self.name))
    }
}
impl Drop for PreparedFiles {
    fn drop(&mut self) {
        if self.retained {
            return;
        }
        for name in &self.created {
            let _ = rustix::fs::unlinkat(&self.directory, name.as_str(), AtFlags::empty());
        }
        if self.guard().is_ok()
            && let Some(parent) = self.chain.last()
        {
            let _ = rustix::fs::unlinkat(parent, self.name.as_str(), AtFlags::REMOVEDIR);
        }
    }
}
