//! Freeze an explicitly selected file/glob once. No dataset registration or broad discovery.
use std::{
    fs,
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};
/// Frozen paths beneath one explicitly selected canonical root.
#[derive(Debug)]
pub struct FileSelection {
    /// Canonical non-wildcard directory prefix; the containment boundary.
    root: PathBuf,
    /// Deterministically sorted relative regular files, never re-expanded during copy.
    files: Vec<PathBuf>,
    directory: File,
}
/// Selection failures never fall back to another directory or broaden a glob.
#[derive(Debug, thiserror::Error)]
pub enum SelectionError {
    /// Invalid or unsupported selection.
    #[error(
        "Import requires a supported explicit file/glob with at least one regular file; symlinks and special files are refused"
    )]
    Invalid,
    /// Explicit enumeration limits.
    #[error(
        "Import selection exceeds 10,000 matches, 100,000 visited entries or 64 directory levels"
    )]
    Limit,
    /// Filesystem failure.
    #[error("Import source selection could not be read")]
    Io(#[from] std::io::Error),
}
impl FileSelection {
    /// Canonical explicit containment root, used only as provenance.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Frozen sorted relative paths.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
    /// Pinned containment directory; copy services traverse relative to this descriptor.
    pub fn descriptor(&self) -> &File {
        &self.directory
    }
    /// Reject replacement of the selected root before accepting the prepared import.
    pub fn verify_root(&self) -> Result<(), SelectionError> {
        let current = directory(&self.root)?.metadata()?;
        let old = self.directory.metadata()?;
        if (current.dev(), current.ino()) != (old.dev(), old.ino()) {
            return Err(SelectionError::Invalid);
        }
        Ok(())
    }
    /// Resolve relative paths against the caller's directory. Never execute a shell glob.
    pub fn freeze(path: &Path) -> Result<Self, SelectionError> {
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()?.join(path)
        };
        let parts: Vec<_> = absolute.components().collect();
        let wildcard = parts.iter().position(|part| {
            part.as_os_str()
                .to_str()
                .is_some_and(|s| s.contains(['*', '?', '[']))
        });
        let split = wildcard.unwrap_or_else(|| parts.len().saturating_sub(1));
        let prefix: PathBuf = parts[..split].iter().collect();
        let root = prefix.canonicalize()?;
        if !root.is_dir() {
            return Err(SelectionError::Invalid);
        }
        let suffix: PathBuf = parts[split..].iter().collect();
        if suffix
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(SelectionError::Invalid);
        }
        let pattern = suffix.to_str().ok_or(SelectionError::Invalid)?;
        if pattern.len() > 4096 {
            return Err(SelectionError::Limit);
        }
        crate::source::validate_glob(pattern).map_err(|_| SelectionError::Invalid)?;
        let directory = directory(&root)?;
        let mut files = Vec::new();
        if wildcard.is_none() {
            let metadata = fs::symlink_metadata(root.join(&suffix))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(SelectionError::Invalid);
            }
            files.push(suffix);
        } else {
            scan(&directory, Path::new(""), pattern, &mut files, &mut 0, 0)?;
        }
        if files.is_empty() {
            return Err(SelectionError::Invalid);
        }
        files.sort();
        Ok(Self {
            root,
            files,
            directory,
        })
    }
}
fn directory(path: &Path) -> Result<File, SelectionError> {
    Ok(File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    ))
}
fn scan(
    dir: &File,
    relative: &Path,
    pattern: &str,
    files: &mut Vec<PathBuf>,
    visited: &mut usize,
    depth: usize,
) -> Result<(), SelectionError> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    if depth > 64 {
        return Err(SelectionError::Limit);
    }
    for entry in rustix::fs::Dir::read_from(dir).map_err(std::io::Error::from)? {
        let entry = entry.map_err(std::io::Error::from)?;
        let name = entry
            .file_name()
            .to_str()
            .map_err(|_| SelectionError::Invalid)?;
        if name == "." || name == ".." {
            continue;
        }
        *visited += 1;
        if *visited > 100_000 {
            return Err(SelectionError::Limit);
        }
        let rel = relative.join(name);
        let display = rel.to_str().ok_or(SelectionError::Invalid)?;
        let stat = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(std::io::Error::from)?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        if kind == FileType::Symlink {
            if pattern.contains('/') || crate::source::glob_matches(pattern, display) {
                return Err(SelectionError::Invalid);
            }
        } else if kind == FileType::Directory {
            if pattern.contains('/') {
                let child = File::from(
                    rustix::fs::openat(
                        dir,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(std::io::Error::from)?,
                );
                scan(&child, &rel, pattern, files, visited, depth + 1)?;
            }
        } else if crate::source::glob_matches(pattern, display) {
            if kind != FileType::RegularFile {
                return Err(SelectionError::Invalid);
            }
            files.push(rel);
            if files.len() > 10_000 {
                return Err(SelectionError::Limit);
            }
        }
    }
    Ok(())
}
