//! Retained structural graph evidence. Consumers must revalidate current source/environment.
use crate::{
    editor::{EditorError, dir_at, identity, root},
    validation::ValidatedGraph,
};
use rustix::fs::{Mode, OFlags};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};
use tf_domain::{RequestId, SourceSnapshotId};
use tf_protocol::canonical::{canonical_json, file_digest};

/// Persist only a successfully validated graph, then atomically update its display pointer.
/// This cache is not execution authority; failed validation never activates prior evidence.
pub fn retain(
    workspace: &Path,
    source: SourceSnapshotId,
    graph: &ValidatedGraph,
    discovery: &Value,
    request: RequestId,
    mut guard: impl FnMut() -> Result<(), EditorError>,
) -> Result<(), EditorError> {
    let base = root(workspace)?;
    let authoring = dir_at(&base, ".transflow", false)?;
    let runtime = dir_at(&authoring, "runtime", false)?;
    let cache = dir_at(&runtime, "graphs", true)?;
    let check = || -> Result<(), EditorError> {
        let current = root(workspace)?;
        let a = dir_at(&current, ".transflow", false)?;
        let r = dir_at(&a, "runtime", false)?;
        let c = dir_at(&r, "graphs", false)?;
        for (old, new) in [
            (&base, &current),
            (&authoring, &a),
            (&runtime, &r),
            (&cache, &c),
        ] {
            if identity(old)? != identity(new)? {
                return Err(EditorError::Conflict);
            }
        }
        Ok(())
    };
    let value = json!({"format_version":1,"source_snapshot_id":source.to_string(),"certificate_fingerprint":graph.certificate().fingerprint().hex(),"certificate":graph.certificate().evidence(),"discovery":discovery});
    let bytes = canonical_json(&value).map_err(|_| EditorError::Conflict)?;
    let digest = file_digest(&mut bytes.as_slice())
        .map_err(|_| EditorError::Conflict)?
        .hex();
    let name = format!("{digest}.json");
    let stage = format!(".graph-{request}");
    let mut file = File::from(
        rustix::fs::openat(
            &cache,
            stage.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(std::io::Error::from)?,
    );
    file.write_all(&bytes)?;
    file.sync_all()?;
    guard()?;
    check()?;
    // Content-addressed bytes make replacement idempotent; no current pointer changes yet.
    rustix::fs::renameat(&cache, stage.as_str(), &cache, name.as_str())
        .map_err(std::io::Error::from)?;
    cache.sync_all()?;
    let read = File::from(
        rustix::fs::openat(
            &cache,
            name.as_str(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    if !read.metadata()?.is_file() {
        return Err(EditorError::Conflict);
    }
    let mut actual = Vec::new();
    read.take(bytes.len() as u64 + 1).read_to_end(&mut actual)?;
    if actual != bytes {
        return Err(EditorError::Conflict);
    }
    let pointer = format!(".current-{request}");
    rustix::fs::symlinkat(name.as_str(), &cache, pointer.as_str()).map_err(std::io::Error::from)?;
    guard()?;
    check()?;
    rustix::fs::renameat(&cache, pointer.as_str(), &cache, "current")
        .map_err(std::io::Error::from)?;
    cache.sync_all()?;
    Ok(())
}
