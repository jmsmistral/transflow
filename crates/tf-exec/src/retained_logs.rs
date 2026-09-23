//! Descriptor-relative reads of retained logs; no path component may be a symlink.
use rustix::fs::{Mode, OFlags};
use std::{
    fs::File,
    path::{Component, Path},
};

/// Open an absolute regular log file without following substituted directories or files.
pub fn open(path: &Path) -> std::io::Result<File> {
    if !path.is_absolute() {
        return Err(std::io::Error::other("log path must be absolute"));
    }
    let mut parent = File::from(rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let mut parts = path.components().skip(1).peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(std::io::Error::other("unsafe log path component"));
        };
        let last = parts.peek().is_none();
        let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        if !last {
            flags |= OFlags::DIRECTORY;
        }
        let file = File::from(rustix::fs::openat(&parent, name, flags, Mode::empty())?);
        if last {
            if !file.metadata()?.is_file() {
                return Err(std::io::Error::other("retained log is not a regular file"));
            }
            return Ok(file);
        }
        parent = file;
    }
    Err(std::io::Error::other("missing log filename"))
}
