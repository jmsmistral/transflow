use crate::{Result, StoreError};
use std::{
    fs,
    path::{Path, PathBuf},
};
/// Reject symlink database files and unknown/network filesystems before SQLite opens them.
pub(crate) fn validate(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().ok_or(StoreError::InvalidRequest)?;
    let parent = path
        .parent()
        .ok_or(StoreError::InvalidRequest)?
        .canonicalize()?;
    let file = parent.join(name);
    match fs::symlink_metadata(&file) {
        Ok(m) if !m.is_file() || m.file_type().is_symlink() => return Err(StoreError::Filesystem),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let stat = rustix::fs::statfs(if file.exists() { &file } else { &parent })
        .map_err(std::io::Error::from)?;
    #[cfg(target_os = "macos")]
    let supported = {
        let name: Vec<u8> = stat
            .f_fstypename
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        name == b"apfs" || name == b"hfs"
    };
    #[cfg(target_os = "linux")]
    let supported = matches!(stat.f_type as u64, 0xef53 | 0x58465342 | 0x9123683e); // ext4, XFS, Btrfs
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let supported = false;
    if !supported {
        return Err(StoreError::Filesystem);
    }
    Ok(file)
}
