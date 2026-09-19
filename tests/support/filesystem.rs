//! Owned disposable paths and a narrow durability boundary over real files.

use super::faults::{Boundary, Checkpoint};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

pub struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    pub fn new() -> io::Result<Self> {
        for _ in 0..100 {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("tf-test-{}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Unable to allocate an owned test directory",
        ))
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn close(self) -> io::Result<()> {
        fs::remove_dir_all(&self.path)
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "Could not clean test directory {}: {error}",
                self.path.display()
            );
        }
    }
}

pub trait DurableFiles {
    fn create_synced(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()>;
}

pub struct RealFiles;
impl DurableFiles for RealFiles {
    fn create_synced(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }
}

/// Inject only at I/O boundaries while retaining actual filesystem writes/fsync.
pub struct FaultFiles<C> {
    pub checkpoint: C,
}
impl<C: Checkpoint> DurableFiles for FaultFiles<C> {
    fn create_synced(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.checkpoint.reach(Boundary::BeforeStagingWrite)?;
        RealFiles.create_synced(path, bytes)?;
        self.checkpoint.reach(Boundary::AfterArtifactSync)
    }
}
