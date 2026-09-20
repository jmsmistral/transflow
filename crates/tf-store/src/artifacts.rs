//! Private, ordered Parquet candidates and immutable, strictly verified objects.
//! Blocking filesystem work: call from a bounded blocking task holding runtime ownership.
use crate::normalization::inspect_parquet;
use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Seek, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};
use tf_domain::RequestId;
use tf_protocol::canonical::{
    ContentDigest, DigestKind, artifact_digest, canonical_json, file_digest,
};

/// Storage, containment or integrity failure. Corrupt objects are never repaired in place.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    /// Actual filesystem error, including missing objects and insufficient space.
    #[error("Artifact storage is unavailable")]
    Io(#[from] std::io::Error),
    /// Files, directory identity or sealed manifest disagree.
    #[error("Artifact integrity verification failed")]
    Integrity,
    /// Invalid, unbounded or unsupported manifest/candidate.
    #[error("Artifact candidate metadata is invalid")]
    Metadata,
    /// Lossless Parquet/schema validation failed.
    #[error(transparent)]
    Normalization(#[from] crate::normalization::NormalizationError),
}
type Result<T> = std::result::Result<T, ArtifactError>;
/// Observable durability boundaries, used by deterministic recovery qualification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    /// A private copy was flushed; index is its manifest order.
    FileSynced(usize),
    /// The closed canonical manifest and staging directory were flushed.
    CandidateSynced,
    /// Last checkpoint before an atomic no-replace installation.
    BeforeInstall,
    /// Directory rename completed; object may survive a process crash.
    Installed,
    /// Both rename parents were flushed; candidate bytes precede DB visibility.
    Durable,
}
fn io(e: rustix::io::Errno) -> std::io::Error {
    e.into()
}
fn directory(parent: &File, name: &str, create: bool) -> Result<File> {
    if create {
        match rustix::fs::mkdirat(parent, name, Mode::RWXU) {
            Ok(()) => parent.sync_all()?,
            Err(rustix::io::Errno::EXIST) => (),
            Err(e) => return Err(io(e).into()),
        }
    }
    Ok(File::from(
        rustix::fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?,
    ))
}
fn open_root(path: &Path) -> Result<File> {
    if path.canonicalize()? != path {
        return Err(ArtifactError::Integrity);
    }
    Ok(File::from(
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?,
    ))
}
fn file(parent: &File, path: &Path) -> Result<File> {
    let mut parent = parent.try_clone()?;
    let mut parts = path.components().peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(ArtifactError::Metadata);
        };
        let name = name.to_str().ok_or(ArtifactError::Metadata)?;
        if parts.peek().is_some() {
            parent = directory(&parent, name, false)?;
        } else {
            let f = File::from(
                rustix::fs::openat(
                    &parent,
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(io)?,
            );
            guard(&f)?;
            return Ok(f);
        }
    }
    Err(ArtifactError::Metadata)
}
fn guard(f: &File) -> Result<(u64, u64, u64, i64, i64, i64, i64)> {
    let m = f.metadata()?;
    if !m.is_file() || m.nlink() != 1 {
        return Err(ArtifactError::Integrity);
    }
    Ok((
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}
fn same(a: &File, b: &File) -> Result<bool> {
    let a = a.metadata()?;
    let b = b.metadata()?;
    Ok((a.dev(), a.ino()) == (b.dev(), b.ino()))
}
fn hash(f: &mut File) -> Result<String> {
    let g = guard(f)?;
    f.rewind()?;
    let h = file_digest(&mut Read::by_ref(f).take(g.2.saturating_add(1)))
        .map_err(|_| ArtifactError::Integrity)?
        .hex();
    if guard(f)? != g {
        return Err(ArtifactError::Integrity);
    }
    Ok(h)
}
fn create(parent: &File, name: &str) -> Result<File> {
    Ok(File::from(
        rustix::fs::openat(
            parent,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io)?,
    ))
}
fn names(dir: &File) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for entry in rustix::fs::Dir::read_from(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let n = entry
            .file_name()
            .to_str()
            .map_err(|_| ArtifactError::Integrity)?;
        if n != "." && n != ".." {
            if names.len() > 10_000 {
                return Err(ArtifactError::Metadata);
            }
            names.insert(n.into());
        }
    }
    Ok(names)
}
/// Descriptor-pinned object namespace; caller must hold exclusive runtime ownership to write.
pub struct ArtifactStore {
    workspace: PathBuf,
    chain: Vec<File>,
}
impl ArtifactStore {
    /// Open/create the objects namespace beneath an existing local runtime directory.
    pub fn open(workspace: &Path) -> Result<Self> {
        let mut chain = vec![open_root(workspace)?];
        for name in [".transflow", "runtime", "objects"] {
            let parent = chain.last().ok_or(ArtifactError::Integrity)?;
            chain.push(directory(parent, name, name == "objects")?);
        }
        let this = Self {
            workspace: workspace.into(),
            chain,
        };
        this.validate()?;
        Ok(this)
    }
    fn root(&self) -> Result<&File> {
        self.chain.last().ok_or(ArtifactError::Integrity)
    }
    fn validate(&self) -> Result<()> {
        let mut current = open_root(&self.workspace)?;
        let dev = current.metadata()?.dev();
        for (i, old) in self.chain.iter().enumerate() {
            if !same(old, &current)? || old.metadata()?.dev() != dev {
                return Err(ArtifactError::Integrity);
            }
            if let Some(name) = [".transflow", "runtime", "objects"].get(i) {
                current = directory(&current, name, false)?;
            }
        }
        Ok(())
    }
    /// Copy an explicitly ordered file list into private staging, decoding every row.
    /// Writer configuration is caller-supplied provenance, validated against the closed
    /// manifest contract; it does not claim to infer historical producer settings.
    pub fn prepare(
        &self,
        source: &File,
        paths: &[PathBuf],
        writer: Value,
        id: RequestId,
        mut observer: impl FnMut(Boundary) -> std::io::Result<()>,
    ) -> Result<Candidate<'_>> {
        if paths.is_empty()
            || paths.len() > 10_000
            || paths.iter().collect::<BTreeSet<_>>().len() != paths.len()
        {
            return Err(ArtifactError::Metadata);
        }
        self.validate()?;
        let name = format!(".staging-{id}");
        rustix::fs::mkdirat(self.root()?, name.as_str(), Mode::RWXU).map_err(io)?;
        let directory = directory(self.root()?, &name, false)?;
        let mut candidate = Candidate {
            store: self,
            directory,
            name,
            created: vec![],
            manifest: Value::Null,
            digest: None,
            installed: false,
        };
        let mut entries = vec![];
        let mut schema = None;
        let mut sources = vec![];
        for (i, path) in paths.iter().enumerate() {
            let mut input = file(source, path)?;
            let before = guard(&input)?;
            let name = format!("part-{i:05}.parquet");
            let mut output = create(&candidate.directory, &name)?;
            candidate.created.push(name.clone());
            if std::io::copy(
                &mut Read::by_ref(&mut input).take(before.2.saturating_add(1)),
                &mut output,
            )? != before.2
            {
                return Err(ArtifactError::Integrity);
            }
            let digest = hash(&mut output)?;
            if hash(&mut input)? != digest
                || guard(&input)? != before
                || guard(&file(source, path)?)? != before
            {
                return Err(ArtifactError::Integrity);
            }
            let inspected = inspect_parquet(output.try_clone()?)?;
            if let Some(ref expected) = schema {
                if expected != inspected.schema.value() {
                    return Err(ArtifactError::Integrity);
                }
            } else {
                schema = Some(inspected.schema.value().clone());
            }
            entries.push(json!({"path":name,"sha256":digest,"byte_length":before.2.to_string(),"row_count":inspected.rows.to_string()}));
            sources.push((path, before, digest));
            rustix::fs::fchmod(&output, Mode::RUSR).map_err(io)?;
            output.sync_all()?;
            observer(Boundary::FileSynced(i))?;
        }
        for (path, before, digest) in sources {
            let mut input = file(source, path)?;
            if guard(&input)? != before || hash(&mut input)? != digest || guard(&input)? != before {
                return Err(ArtifactError::Integrity);
            }
        }
        let schema = schema.ok_or(ArtifactError::Metadata)?;
        let fingerprint = tf_protocol::canonical::schema_digest(&schema)
            .map_err(|_| ArtifactError::Metadata)?
            .hex();
        candidate.manifest = json!({"format_version":1,"logical_schema":schema,"schema_fingerprint":fingerprint,"files":entries,"writer":writer});
        let digest = artifact_digest(&candidate.manifest).map_err(|_| ArtifactError::Metadata)?;
        candidate.digest = Some(digest);
        let mut manifest = create(&candidate.directory, "manifest.json")?;
        candidate.created.push("manifest.json".into());
        manifest.write_all(
            &canonical_json(&candidate.manifest).map_err(|_| ArtifactError::Metadata)?,
        )?;
        rustix::fs::fchmod(&manifest, Mode::RUSR).map_err(io)?;
        manifest.sync_all()?;
        candidate.directory.sync_all()?;
        candidate.verify()?;
        observer(Boundary::CandidateSynced)?;
        Ok(candidate)
    }
    /// Full hashes, exact membership, schema and decoded row counts. No cross-build cache.
    pub fn verify(&self, digest: ContentDigest) -> Result<VerifiedArtifact> {
        if digest.kind() != DigestKind::Artifact {
            return Err(ArtifactError::Metadata);
        }
        self.validate()?;
        let hex = digest.hex();
        let prefix = directory(self.root()?, &hex[..2], false)?;
        let object = directory(&prefix, &hex, false)?;
        let manifest = verify_directory(&object, digest)?;
        self.validate()?;
        if !same(&prefix, &directory(self.root()?, &hex[..2], false)?)?
            || !same(&object, &directory(&prefix, &hex, false)?)?
        {
            return Err(ArtifactError::Integrity);
        }
        Ok(VerifiedArtifact {
            digest,
            manifest,
            workspace: self.workspace.clone(),
        })
    }
}
/// Private validated bytes; dropping removes only this candidate's own created files.
pub struct Candidate<'a> {
    store: &'a ArtifactStore,
    directory: File,
    name: String,
    created: Vec<String>,
    manifest: Value,
    digest: Option<ContentDigest>,
    installed: bool,
}
impl Candidate<'_> {
    /// Canonical, ordered manifest over the exact candidate bytes.
    pub fn manifest(&self) -> &Value {
        &self.manifest
    }
    /// Artifact identity, independent of planned version/attempt identity.
    pub fn digest(&self) -> Result<ContentDigest> {
        self.digest.ok_or(ArtifactError::Metadata)
    }
    fn verify(&self) -> Result<()> {
        self.store.validate()?;
        if !same(
            &self.directory,
            &directory(self.store.root()?, &self.name, false)?,
        )? {
            return Err(ArtifactError::Integrity);
        }
        if verify_directory(&self.directory, self.digest()?)? != self.manifest {
            return Err(ArtifactError::Integrity);
        }
        Ok(())
    }
    /// Seal and atomically install without replacement, then flush both rename parents.
    /// An error after rename intentionally leaves an invisible orphan for later GC.
    pub fn install(
        mut self,
        mut observer: impl FnMut(Boundary) -> std::io::Result<()>,
    ) -> Result<VerifiedArtifact> {
        self.verify()?;
        let digest = self.digest()?;
        let hex = digest.hex();
        let prefix = directory(self.store.root()?, &hex[..2], true)?;
        if prefix.metadata()?.dev() != self.directory.metadata()?.dev() {
            return Err(ArtifactError::Integrity);
        }
        self.directory.sync_all()?;
        observer(Boundary::BeforeInstall)?;
        self.verify()?;
        match rustix::fs::renameat_with(
            self.store.root()?,
            self.name.as_str(),
            &prefix,
            hex.as_str(),
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => {
                self.installed = true;
                rustix::fs::fchmod(&self.directory, Mode::RUSR | Mode::XUSR).map_err(io)?;
                self.directory.sync_all()?;
                observer(Boundary::Installed)?;
            }
            Err(rustix::io::Errno::EXIST) => {
                self.store.verify(digest)?;
            }
            Err(e) => return Err(io(e).into()),
        }
        prefix.sync_all()?;
        self.store.root()?.sync_all()?;
        observer(Boundary::Durable)?;
        self.store.verify(digest)
    }
}
impl Drop for Candidate<'_> {
    fn drop(&mut self) {
        if !self.installed {
            let _ = rustix::fs::fchmod(&self.directory, Mode::RWXU);
            for n in &self.created {
                let _ = rustix::fs::unlinkat(&self.directory, n.as_str(), AtFlags::empty());
            }
            if let Ok(parent) = self.store.root()
                && let Ok(current) = directory(parent, &self.name, false)
                && same(&current, &self.directory).unwrap_or(false)
            {
                let _ = rustix::fs::unlinkat(parent, self.name.as_str(), AtFlags::REMOVEDIR);
            }
        }
    }
}
/// Successful strict verification. Construction is private; contains no staging path.
#[derive(Debug)]
pub struct VerifiedArtifact {
    digest: ContentDigest,
    manifest: Value,
    workspace: PathBuf,
}
impl VerifiedArtifact {
    /// Physical identity (not a dataset version).
    pub fn digest(&self) -> ContentDigest {
        self.digest
    }
    /// Closed canonical metadata verified against actual Parquet bytes.
    pub fn manifest(&self) -> &Value {
        &self.manifest
    }
    /// Owning canonical workspace, for coordinator binding checks.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
}
fn verify_directory(dir: &File, digest: ContentDigest) -> Result<Value> {
    let mut input = file(dir, Path::new("manifest.json"))?;
    let g = guard(&input)?;
    if g.2 > 16 * 1024 * 1024 {
        return Err(ArtifactError::Metadata);
    }
    let mut bytes = vec![];
    Read::by_ref(&mut input)
        .take(g.2 + 1)
        .read_to_end(&mut bytes)?;
    let manifest: Value = serde_json::from_slice(&bytes).map_err(|_| ArtifactError::Integrity)?;
    if guard(&input)? != g
        || canonical_json(&manifest).map_err(|_| ArtifactError::Integrity)? != bytes
        || artifact_digest(&manifest).map_err(|_| ArtifactError::Integrity)? != digest
    {
        return Err(ArtifactError::Integrity);
    }
    let entries = manifest["files"]
        .as_array()
        .ok_or(ArtifactError::Integrity)?;
    if entries.is_empty() || entries.len() > 10_000 {
        return Err(ArtifactError::Metadata);
    }
    let mut expected = BTreeSet::from(["manifest.json".into()]);
    for (i, entry) in entries.iter().enumerate() {
        let name = entry["path"].as_str().ok_or(ArtifactError::Integrity)?;
        // The installer produces flat ordinal names, so nested/aliased layouts cannot enter it.
        if name != format!("part-{i:05}.parquet") || !expected.insert(name.into()) {
            return Err(ArtifactError::Integrity);
        }
        let mut f = file(dir, Path::new(name))?;
        let before = guard(&f)?;
        if entry["byte_length"]
            .as_str()
            .and_then(|n| n.parse::<u64>().ok())
            != Some(before.2)
            || entry["sha256"] != hash(&mut f)?
        {
            return Err(ArtifactError::Integrity);
        }
        let inspected = inspect_parquet(f.try_clone()?)?;
        if inspected.schema.value() != &manifest["logical_schema"]
            || entry["row_count"]
                .as_str()
                .and_then(|n| n.parse::<u64>().ok())
                != Some(inspected.rows)
            || guard(&f)? != before
            || guard(&file(dir, Path::new(name))?)? != before
        {
            return Err(ArtifactError::Integrity);
        }
    }
    if names(dir)? != expected || guard(&file(dir, Path::new("manifest.json"))?)? != g {
        return Err(ArtifactError::Integrity);
    }
    Ok(manifest)
}
