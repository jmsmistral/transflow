//! Descriptor-bound atomic replacement of machine-local configuration.
//! A locator can safely exist without a registration; callers install it before registry intent.
use crate::{
    registry_write::{RegistryWriteError, Result},
    workspace::LocalConfig,
};
use rustix::fs::{Mode, OFlags};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use tf_domain::{RequestId, WorkspaceId};

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
fn identity(file: &File) -> Result<(u64, u64)> {
    let m = file.metadata()?;
    Ok((m.dev(), m.ino()))
}
fn read(dir: &File) -> Result<Option<String>> {
    let fd = match rustix::fs::openat(
        dir,
        "transflow.local.toml",
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(e) => return Err(std::io::Error::from(e).into()),
    };
    let file = File::from(fd);
    if !file.metadata()?.is_file() || file.metadata()?.len() > 16 * 1024 * 1024 {
        return Err(RegistryWriteError::Conflict);
    }
    let mut text = String::new();
    file.take(16 * 1024 * 1024 + 1).read_to_string(&mut text)?;
    if text.len() > 16 * 1024 * 1024 {
        return Err(RegistryWriteError::Conflict);
    }
    Ok(Some(text))
}
/// One guarded local update. No private path is copied to the registry or recovery journal.
pub struct LocatorWrite {
    root: PathBuf,
    dir: File,
    old: Option<String>,
    new: String,
    temporary: String,
    staged: bool,
}
impl LocatorWrite {
    /// Parse/validate existing local settings and bind the exact expected bytes and directory.
    pub fn prepare(root: &Path, provider: WorkspaceId, path: &Path, id: RequestId) -> Result<Self> {
        let dir = directory(root)?;
        let old = read(&dir)?;
        let new = LocalConfig::render_provider(old.as_deref().unwrap_or(""), provider, path)
            .map_err(|_| RegistryWriteError::Conflict)?;
        Ok(Self {
            root: root.to_owned(),
            dir,
            old,
            new,
            temporary: format!(".transflow-local-{id}.tmp"),
            staged: false,
        })
    }
    /// Compare expected bytes immediately before same-directory rename; preserve user edits.
    pub fn apply(mut self, guard: impl Fn() -> Result<()>) -> Result<()> {
        guard()?;
        if identity(&directory(&self.root)?)? != identity(&self.dir)?
            || read(&self.dir)? != self.old
        {
            return Err(RegistryWriteError::Conflict);
        }
        if self.old.as_deref() == Some(&self.new) {
            return Ok(());
        }
        let mut file = File::from(
            rustix::fs::openat(
                &self.dir,
                self.temporary.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?,
        );
        self.staged = true;
        file.write_all(self.new.as_bytes())?;
        file.sync_all()?;
        guard()?;
        if identity(&directory(&self.root)?)? != identity(&self.dir)?
            || read(&self.dir)? != self.old
        {
            return Err(RegistryWriteError::Conflict);
        }
        rustix::fs::renameat(
            &self.dir,
            self.temporary.as_str(),
            &self.dir,
            "transflow.local.toml",
        )
        .map_err(std::io::Error::from)?;
        self.staged = false;
        self.dir.sync_all()?;
        Ok(())
    }
}
impl Drop for LocatorWrite {
    fn drop(&mut self) {
        if self.staged {
            let _ = rustix::fs::unlinkat(
                &self.dir,
                self.temporary.as_str(),
                rustix::fs::AtFlags::empty(),
            );
        }
    }
}
