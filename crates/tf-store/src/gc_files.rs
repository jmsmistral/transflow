//! Descriptor-relative retirement into an intent-specific directory, then bounded removal.
//! Only the runtime owner may call `remove`; a durable matching DB intent must exist.
use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags};
use std::{
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};
use tf_domain::RequestId;
/// Auditable physical deletion boundaries. Interrupted intents remain fenced in SQLite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Boundary {
    /// Before moving the exact previously verified directory.
    BeforeRename,
    /// The intent-specific rename is durable; recovery can safely resume partial removal.
    Renamed,
    /// The private retired tree is empty.
    FilesRemoved,
    /// Removed directory entry and its parent are durable.
    Durable,
}
/// Identity of the directory verified before the DB claim; cannot be fabricated externally.
#[derive(Clone, Copy)]
pub struct Identity {
    dev: u64,
    ino: u64,
}
/// Pinned local namespace. Construction does not create files/directories.
pub struct Directory {
    workspace: PathBuf,
    parts: Vec<String>,
    chain: Vec<File>,
}
fn invalid() -> std::io::Error {
    std::io::Error::other("GC namespace, file identity or finite tree limit is invalid")
}
fn name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 4096
        && s != "."
        && s != ".."
        && !s.contains(['/', '\\'])
        && !s.chars().any(char::is_control)
}
fn open(parent: &File, key: &str, dir: bool) -> std::io::Result<File> {
    if !name(key) {
        return Err(invalid());
    }
    let flags = OFlags::RDONLY
        | OFlags::NOFOLLOW
        | OFlags::NONBLOCK
        | OFlags::CLOEXEC
        | if dir {
            OFlags::DIRECTORY
        } else {
            OFlags::empty()
        };
    Ok(File::from(rustix::fs::openat(
        parent,
        key,
        flags,
        Mode::empty(),
    )?))
}
fn entries(parent: &File) -> std::io::Result<Vec<String>> {
    let mut names = Vec::new();
    let mut size = 0usize;
    for entry in rustix::fs::Dir::read_from(parent)? {
        let entry = entry?;
        let key = entry.file_name().to_str().map_err(|_| invalid())?;
        if key == "." || key == ".." {
            continue;
        }
        if names.len() >= 100_000 || !name(key) {
            return Err(invalid());
        }
        size = size.checked_add(key.len()).ok_or_else(invalid)?;
        if size > 16 * 1024 * 1024 {
            return Err(invalid());
        }
        names.push(key.to_string());
    }
    names.sort();
    Ok(names)
}
fn same(a: &File, b: &File) -> std::io::Result<bool> {
    let (a, b) = (a.metadata()?, b.metadata()?);
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}
#[derive(Default)]
struct Budget {
    entries: usize,
    names: usize,
}
fn inspect(dir: &File, depth: usize, budget: &mut Budget, remove: bool) -> std::io::Result<u64> {
    if depth > 64 {
        return Err(invalid());
    }
    let mut bytes = 0u64;
    if remove {
        rustix::fs::fchmod(dir, Mode::RWXU)?;
    }
    let keys = entries(dir)?;
    budget.entries += keys.len();
    budget.names += keys.iter().map(String::len).sum::<usize>();
    if budget.entries > 100_000 || budget.names > 16 * 1024 * 1024 {
        return Err(invalid());
    }
    for key in keys {
        let child = open(dir, &key, false)?;
        let meta = child.metadata()?;
        if meta.dev() != dir.metadata()?.dev() {
            return Err(invalid());
        }
        if meta.is_dir() {
            bytes = bytes
                .checked_add(inspect(&child, depth + 1, budget, remove)?)
                .ok_or_else(invalid)?;
            if remove {
                if !same(&child, &open(dir, &key, true)?)? {
                    return Err(invalid());
                }
                rustix::fs::unlinkat(dir, key.as_str(), AtFlags::REMOVEDIR)?;
            }
        } else if meta.is_file() && meta.nlink() == 1 {
            bytes = bytes.checked_add(meta.len()).ok_or_else(invalid)?;
            if remove {
                if !same(&child, &open(dir, &key, false)?)? {
                    return Err(invalid());
                }
                rustix::fs::unlinkat(dir, key.as_str(), AtFlags::empty())?;
            }
        } else {
            return Err(invalid());
        }
    }
    if remove {
        dir.sync_all()?;
    }
    Ok(bytes)
}
impl Directory {
    /// Open one existing local runtime namespace without following any substituted component.
    pub fn open(workspace: &Path, relative: &Path) -> std::io::Result<Self> {
        if workspace.canonicalize()? != workspace {
            return Err(invalid());
        }
        let root = File::from(rustix::fs::open(
            workspace,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let mut chain = vec![root];
        let mut parts = vec![".transflow".to_string(), "runtime".to_string()];
        for part in relative.components() {
            let Component::Normal(part) = part else {
                return Err(invalid());
            };
            parts.push(part.to_str().ok_or_else(invalid)?.to_string());
        }
        for part in &parts {
            chain.push(open(chain.last().ok_or_else(invalid)?, part, true)?);
        }
        let value = Self {
            workspace: workspace.to_path_buf(),
            parts,
            chain,
        };
        value.validate()?;
        Ok(value)
    }
    fn root(&self) -> std::io::Result<&File> {
        self.chain.last().ok_or_else(invalid)
    }
    fn validate(&self) -> std::io::Result<()> {
        let mut current = File::from(rustix::fs::open(
            &self.workspace,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let dev = current.metadata()?.dev();
        for (i, old) in self.chain.iter().enumerate() {
            if !same(old, &current)? || old.metadata()?.dev() != dev {
                return Err(invalid());
            }
            if let Some(part) = self.parts.get(i) {
                current = open(&current, part, true)?;
            }
        }
        Ok(())
    }
    /// Finite sorted directory names; callers retain unknown formats explicitly.
    pub fn entries(&self) -> std::io::Result<Vec<String>> {
        self.validate()?;
        entries(self.root()?)
    }
    /// Read-only size/identity inspection; special files, symlinks and hardlinks are refused.
    pub fn inspect(&self, key: &str) -> std::io::Result<(Identity, u64)> {
        self.validate()?;
        let dir = open(self.root()?, key, true)?;
        let meta = dir.metadata()?;
        let bytes = inspect(&dir, 0, &mut Budget::default(), false)?;
        self.validate()?;
        if !same(&dir, &open(self.root()?, key, true)?)? {
            return Err(invalid());
        }
        Ok((
            Identity {
                dev: meta.dev(),
                ino: meta.ino(),
            },
            bytes,
        ))
    }
    /// Whether a matching retired intent directory exists. Unknown private names are never swept.
    pub fn retired(&self, id: RequestId) -> std::io::Result<bool> {
        self.validate()?;
        match open(self.root()?, &format!(".gc-{id}"), true) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
    /// Retire exactly the verified identity, or resume the exact durable intent's partial tree.
    pub fn remove(
        &self,
        key: &str,
        id: RequestId,
        expected: Option<Identity>,
        mut observer: impl FnMut(Boundary) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.validate()?;
        if !name(key) {
            return Err(invalid());
        }
        let trash = format!(".gc-{id}");
        if !self.retired(id)? {
            let original = match open(self.root()?, key, true) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.root()?.sync_all()?;
                    return observer(Boundary::Durable);
                }
                Err(e) => return Err(e),
            };
            let meta = original.metadata()?;
            let expected = expected.ok_or_else(invalid)?;
            if meta.dev() != expected.dev || meta.ino() != expected.ino {
                return Err(invalid());
            }
            inspect(&original, 0, &mut Budget::default(), false)?;
            observer(Boundary::BeforeRename)?;
            self.validate()?;
            if !same(&original, &open(self.root()?, key, true)?)? {
                return Err(invalid());
            }
            // Unseal the claimed directory before retirement and traversal.
            // The durable DB claim already excludes new readers/publication.
            rustix::fs::fchmod(&original, Mode::RWXU)?;
            original.sync_all()?;
            rustix::fs::renameat_with(
                self.root()?,
                key,
                self.root()?,
                trash.as_str(),
                RenameFlags::NOREPLACE,
            )?;
            self.root()?.sync_all()?;
        }
        observer(Boundary::Renamed)?;
        let dir = open(self.root()?, &trash, true)?;
        inspect(&dir, 0, &mut Budget::default(), false)?; // Audit every entry before starting/resuming deletion.
        inspect(&dir, 0, &mut Budget::default(), true)?;
        observer(Boundary::FilesRemoved)?;
        self.validate()?;
        if !same(&dir, &open(self.root()?, &trash, true)?)? {
            return Err(invalid());
        }
        rustix::fs::unlinkat(self.root()?, trash.as_str(), AtFlags::REMOVEDIR)?;
        self.root()?.sync_all()?;
        observer(Boundary::Durable)
    }
}
