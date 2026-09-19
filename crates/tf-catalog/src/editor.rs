//! Typing-only, immutable catalogue generations and an atomic editor pointer.
//! The caller serializes refreshes under workspace ownership; runtime never reads this cache.
use rustix::fs::{AtFlags, Mode, OFlags};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use tf_domain::RequestId;
use tf_protocol::{
    canonical::{catalog_fingerprint, file_digest},
    validate_document,
};

const ROOT: &str = include_str!("editor_root.pyi");
const LIMIT: u64 = 32 * 1024 * 1024;
/// A malformed snapshot, modified generation or unsafe filesystem cannot activate an overlay.
#[derive(Debug, thiserror::Error)]
pub enum EditorError {
    /// Snapshot validation failed before any file was written.
    #[error("The catalogue snapshot is invalid or stale; capture it again")]
    Snapshot,
    /// Existing generated content or a directory identity changed.
    #[error("Editor references are stale or modified; regenerate the workspace editor cache")]
    Conflict,
    /// A filesystem operation failed; the old pointer is retained until atomic replacement.
    #[error("Editor references could not be refreshed")]
    Io(#[from] std::io::Error),
}
/// Editor generation result.
pub type Result<T> = std::result::Result<T, EditorError>;
/// Fully rendered, verified snapshot metadata. It contains no executable Python files.
#[derive(Clone, Debug)]
pub struct Overlay {
    fingerprint: String,
    files: BTreeMap<String, String>,
}
impl Overlay {
    /// Render the exact public SDK namespace, including aliases and dataset-prefix children.
    pub fn render(snapshot: &Value) -> Result<Self> {
        validate_document("CatalogSnapshotV1", snapshot).map_err(|_| EditorError::Snapshot)?;
        let fp = catalog_fingerprint(snapshot)
            .map_err(|_| EditorError::Snapshot)?
            .hex();
        if snapshot["catalog_fingerprint"] != fp {
            return Err(EditorError::Snapshot);
        }
        let entries = snapshot["entries"]
            .as_array()
            .ok_or(EditorError::Snapshot)?;
        let aliases = snapshot.get("aliases").and_then(Value::as_array);
        if entries.len() + aliases.map_or(0, Vec::len) > 100_000 {
            return Err(EditorError::Snapshot);
        }
        let mut tree: BTreeMap<String, BTreeSet<String>> =
            BTreeMap::from([(String::new(), BTreeSet::new())]);
        for record in entries.iter().chain(aliases.into_iter().flatten()) {
            let path = record["path"].as_str().ok_or(EditorError::Snapshot)?;
            let parts: Vec<_> = path.split('/').collect();
            if path.chars().count() > 4096 || parts.len() > 64 {
                return Err(EditorError::Snapshot);
            }
            for (index, part) in parts.iter().enumerate() {
                tree.entry(parts[..index].join("/"))
                    .or_default()
                    .insert((*part).to_owned());
            }
            tree.entry(path.to_owned()).or_default();
        }
        let classes: BTreeMap<_, _> = tree
            .keys()
            .enumerate()
            .map(|(i, p)| (p.clone(), format!("_Node{i}")))
            .collect();
        let mut catalog = format!(
            "# Catalogue fingerprint: {fp}\nfrom transflow._catalog import (\n    CatalogNode, CatalogSnapshot as CatalogSnapshot, DatasetRef as DatasetRef,\n    CatalogContextError as CatalogContextError, CatalogLookupError as CatalogLookupError,\n    resolve_reference as resolve_reference,\n)\n\n"
        );
        for (path, children) in &tree {
            let class = classes.get(path).ok_or(EditorError::Snapshot)?;
            catalog.push_str(&format!("class {class}(CatalogNode):\n"));
            for child in children {
                let target = if path.is_empty() {
                    child.clone()
                } else {
                    format!("{path}/{child}")
                };
                let target = classes.get(&target).ok_or(EditorError::Snapshot)?;
                catalog.push_str(&format!(
                    "    @property\n    def {child}(self) -> {target}: ...\n"
                ));
            }
            if children.is_empty() {
                catalog.push_str("    pass\n");
            }
            catalog.push('\n');
            if catalog.len() as u64 > LIMIT {
                return Err(EditorError::Snapshot);
            }
        }
        catalog.push_str("C: _Node0\n");
        let mut files: BTreeMap<String, String> = BTreeMap::from([
            ("transflow/__init__.pyi".into(), ROOT.into()),
            ("transflow/catalog.pyi".into(), catalog),
        ]);
        let mut hashes = BTreeMap::new();
        for (name, value) in &files {
            hashes.insert(
                name.clone(),
                file_digest(&mut value.as_bytes())
                    .map_err(|_| EditorError::Snapshot)?
                    .hex(),
            );
        }
        let manifest = json!({"generator_format":1,"catalog_fingerprint":fp,"workspace_id":snapshot["workspace_id"],"files":hashes});
        files.insert(
            "manifest.json".into(),
            serde_json::to_string_pretty(&manifest).map_err(|_| EditorError::Snapshot)? + "\n",
        );
        Ok(Self {
            fingerprint: fp,
            files,
        })
    }
    /// Fingerprint shown by the editor and retained in the manifest.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    /// Deterministic typing files plus their checked manifest; useful for checker qualification.
    pub fn files(&self) -> &BTreeMap<String, String> {
        &self.files
    }
}
fn dir_at(parent: &File, name: &str, create: bool) -> Result<File> {
    if create {
        match rustix::fs::mkdirat(parent, name, Mode::RWXU) {
            Ok(()) => parent.sync_all()?,
            Err(rustix::io::Errno::EXIST) => (),
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
fn root(path: &Path) -> Result<File> {
    if !path.is_absolute() || path.canonicalize()? != path {
        return Err(EditorError::Conflict);
    }
    Ok(File::from(
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    ))
}
fn identity(file: &File) -> Result<(u64, u64)> {
    let m = file.metadata()?;
    Ok((m.dev(), m.ino()))
}
fn names(dir: &File) -> Result<BTreeSet<String>> {
    let mut result = BTreeSet::new();
    let entries = rustix::fs::Dir::read_from(dir).map_err(std::io::Error::from)?;
    for entry in entries {
        let entry = entry.map_err(std::io::Error::from)?;
        let name = entry
            .file_name()
            .to_str()
            .map_err(|_| EditorError::Conflict)?;
        if name != "." && name != ".." {
            result.insert(name.to_owned());
        }
        if result.len() > 4 {
            return Err(EditorError::Conflict);
        }
    }
    Ok(result)
}
fn verify(dir: &File, overlay: &Overlay) -> Result<()> {
    if names(dir)? != BTreeSet::from(["manifest.json".into(), "transflow".into()]) {
        return Err(EditorError::Conflict);
    }
    let sdk = dir_at(dir, "transflow", false)?;
    if names(&sdk)? != BTreeSet::from(["__init__.pyi".into(), "catalog.pyi".into()]) {
        return Err(EditorError::Conflict);
    }
    for (name, expected) in &overlay.files {
        let (parent, name) = match name.strip_prefix("transflow/") {
            Some(n) => (&sdk, n),
            None => (dir, name.as_str()),
        };
        let file = File::from(
            rustix::fs::openat(
                parent,
                name,
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        if !file.metadata()?.is_file() || file.metadata()?.len() != expected.len() as u64 {
            return Err(EditorError::Conflict);
        }
        let mut data = String::new();
        file.take(LIMIT + 1).read_to_string(&mut data)?;
        if data != *expected {
            return Err(EditorError::Conflict);
        }
    }
    Ok(())
}
/// File synchronization points for deterministic failure tests and cancellation checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Boundary {
    /// Complete staging files are durable; current is unchanged.
    Staged,
    /// Immutable generation is installed; current is unchanged.
    Installed,
    /// Immediately before activating a checked pointer.
    BeforeActivate,
}
/// Descriptor-bound editor cache. Use on a blocking worker while holding workspace ownership.
pub struct EditorCache {
    root: PathBuf,
    chain: Vec<File>,
}
impl EditorCache {
    /// Open/create only real directories under an explicit canonical workspace.
    pub fn open(workspace: &Path) -> Result<Self> {
        let mut chain = vec![root(workspace)?];
        for name in [".transflow", "runtime", "generated"] {
            let parent = chain.last().ok_or(EditorError::Conflict)?;
            chain.push(dir_at(parent, name, true)?);
        }
        Ok(Self {
            root: workspace.to_owned(),
            chain,
        })
    }
    fn generated(&self) -> Result<&File> {
        self.chain.last().ok_or(EditorError::Conflict)
    }
    fn guard(&self) -> Result<()> {
        let mut next = root(&self.root)?;
        for (index, old) in self.chain.iter().enumerate() {
            if identity(&next)? != identity(old)? {
                return Err(EditorError::Conflict);
            }
            if let Some(name) = [".transflow", "runtime", "generated"].get(index) {
                next = dir_at(&next, name, false)?;
            }
        }
        Ok(())
    }
    fn pointer(&self) -> Result<Option<String>> {
        let target = match rustix::fs::readlinkat(self.generated()?, "current", Vec::new()) {
            Ok(target) => target,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(e) => return Err(std::io::Error::from(e).into()),
        };
        let target = target.to_str().map_err(|_| EditorError::Conflict)?;
        let Some(fp) = target.strip_suffix("/type-stubs") else {
            return Err(EditorError::Conflict);
        };
        if fp.len() != 64
            || !fp
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(EditorError::Conflict);
        }
        Ok(Some(fp.to_owned()))
    }
    /// Verify both pointer identity and expected immutable bytes; no silent stale success.
    pub fn check(&self, overlay: &Overlay) -> Result<PathBuf> {
        self.guard()?;
        if self.pointer()?.as_deref() != Some(&overlay.fingerprint) {
            return Err(EditorError::Conflict);
        }
        let generation = dir_at(self.generated()?, &overlay.fingerprint, false)?;
        verify(&dir_at(&generation, "type-stubs", false)?, overlay)?;
        self.guard()?;
        Ok(self.root.join(".transflow/runtime/generated/current"))
    }
    /// Install a complete generation then atomically activate it. Interrupted staging is ignored.
    /// Request IDs must be unique. Callback failures before activation preserve the old pointer.
    pub fn refresh(
        &self,
        overlay: &Overlay,
        request: RequestId,
        mut boundary: impl FnMut(Boundary) -> Result<()>,
    ) -> Result<PathBuf> {
        self.guard()?;
        let previous = self.pointer()?;
        let generated = self.generated()?;
        let generation = dir_at(generated, &overlay.fingerprint, true)?;
        match dir_at(&generation, "type-stubs", false) {
            Ok(existing) => verify(&existing, overlay)?,
            Err(EditorError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                let temporary = format!(".overlay-{request}");
                rustix::fs::mkdirat(&generation, temporary.as_str(), Mode::RWXU)
                    .map_err(std::io::Error::from)?;
                let stage = dir_at(&generation, &temporary, false)?;
                let sdk = dir_at(&stage, "transflow", true)?;
                // Interrupted private staging is retained for explicit cache cleanup.
                let written: Result<()> = (|| {
                    for (name, content) in &overlay.files {
                        let (dir, name) = match name.strip_prefix("transflow/") {
                            Some(n) => (&sdk, n),
                            None => (&stage, name.as_str()),
                        };
                        let mut file = File::from(
                            rustix::fs::openat(
                                dir,
                                name,
                                OFlags::WRONLY
                                    | OFlags::CREATE
                                    | OFlags::EXCL
                                    | OFlags::NOFOLLOW
                                    | OFlags::CLOEXEC,
                                Mode::RUSR | Mode::WUSR,
                            )
                            .map_err(std::io::Error::from)?,
                        );
                        file.write_all(content.as_bytes())?;
                        file.sync_all()?;
                    }
                    sdk.sync_all()?;
                    stage.sync_all()?;
                    verify(&stage, overlay)?;
                    boundary(Boundary::Staged)?;
                    self.guard()?;
                    rustix::fs::renameat(
                        &generation,
                        temporary.as_str(),
                        &generation,
                        "type-stubs",
                    )
                    .map_err(std::io::Error::from)?;
                    generation.sync_all()?;
                    Ok(())
                })();
                // Failed generations are private, non-authoritative and never traversed by editors.
                written?;
            }
            Err(e) => return Err(e),
        }
        verify(&dir_at(&generation, "type-stubs", false)?, overlay)?;
        boundary(Boundary::Installed)?;
        let temporary = format!(".current-{request}");
        rustix::fs::symlinkat(
            format!("{}/type-stubs", overlay.fingerprint),
            generated,
            temporary.as_str(),
        )
        .map_err(std::io::Error::from)?;
        let activated = (|| {
            boundary(Boundary::BeforeActivate)?;
            self.guard()?;
            if self.pointer()? != previous {
                return Err(EditorError::Conflict);
            }
            verify(&dir_at(&generation, "type-stubs", false)?, overlay)?;
            rustix::fs::renameat(generated, temporary.as_str(), generated, "current")
                .map_err(std::io::Error::from)?;
            generated.sync_all()?;
            self.check(overlay)
        })();
        if activated.is_err() {
            let _ = rustix::fs::unlinkat(generated, temporary.as_str(), AtFlags::empty());
        }
        activated
    }
}
