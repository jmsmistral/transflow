//! Explicit contributor UI bundle, frozen before listening. Never serves workspace files.
use crate::ApiError;
use std::{collections::BTreeMap, io::Read, path::Path};

/// Bounded immutable files from an explicitly selected Vite production build.
#[derive(Default)]
pub struct Assets(BTreeMap<String, (&'static str, Vec<u8>)>);
impl Assets {
    /// Load index.html and flat assets/*.js/css only. Symlinks and oversized bundles fail closed.
    pub fn load(root: &Path) -> Result<Self, ApiError> {
        let fail = || {
            ApiError::new(
                400,
                "TF_UI_BUNDLE",
                "Select a regular built UI directory containing index.html and assets",
            )
        };
        let metadata = std::fs::symlink_metadata(root).map_err(|_| fail())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(fail());
        }
        let directory = root.join("assets");
        let metadata = std::fs::symlink_metadata(&directory).map_err(|_| fail())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(fail());
        }
        let mut files = vec![(
            "/".to_owned(),
            root.join("index.html"),
            "text/html; charset=utf-8",
        )];
        for entry in std::fs::read_dir(directory).map_err(|_| fail())? {
            if files.len() >= 64 {
                return Err(fail());
            }
            let entry = entry.map_err(|_| fail())?;
            let name = entry.file_name().into_string().map_err(|_| fail())?;
            if !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err(fail());
            }
            let mime = if name.ends_with(".js") {
                "text/javascript; charset=utf-8"
            } else if name.ends_with(".css") {
                "text/css; charset=utf-8"
            } else {
                return Err(fail());
            };
            files.push((format!("/assets/{name}"), entry.path(), mime));
        }
        let mut result = Self::default();
        let mut remaining = 8 * 1024 * 1024usize;
        for (url, path, mime) in files {
            let before = std::fs::symlink_metadata(&path).map_err(|_| fail())?;
            if !before.is_file() || before.file_type().is_symlink() {
                return Err(fail());
            }
            let canonical = path.canonicalize().map_err(|_| fail())?;
            if !canonical.starts_with(root.canonicalize().map_err(|_| fail())?) {
                return Err(fail());
            }
            let file = std::fs::File::open(&path).map_err(|_| fail())?;
            let opened = file.metadata().map_err(|_| fail())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if before.dev() != opened.dev() || before.ino() != opened.ino() {
                    return Err(fail());
                }
            }
            if !opened.is_file() {
                return Err(fail());
            }
            let mut bytes = Vec::new();
            file.take((remaining + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| fail())?;
            if bytes.len() > remaining {
                return Err(fail());
            }
            remaining -= bytes.len();
            result.0.insert(url, (mime, bytes));
        }
        Ok(result)
    }
    pub(crate) fn get(&self, path: &str) -> Option<(&'static str, Vec<u8>)> {
        self.0.get(path).cloned()
    }
}
