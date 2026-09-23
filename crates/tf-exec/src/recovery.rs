//! Private worker capabilities for emergency self-termination after coordinator loss.
//! Never signal a persisted PID: the live worker authenticates and kills its own group.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    attempt: String,
    pid: u32,
    start: String,
    socket: PathBuf,
    token: String,
}
fn invalid() -> std::io::Error {
    std::io::Error::other(
        "Worker recovery identity or private capability is unavailable; execution remains fenced",
    )
}
pub(crate) fn persist(
    directory: &Path,
    attempt: &str,
    pid: u32,
    start: &str,
    socket: PathBuf,
    token: &str,
) -> std::io::Result<()> {
    let record = Record {
        attempt: attempt.into(),
        pid,
        start: start.into(),
        socket,
        token: token.into(),
    };
    let tmp = directory.join("recovery.tmp");
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    serde_json::to_writer(&mut f, &record)?;
    f.sync_all()?;
    fs::rename(tmp, directory.join("recovery.json"))?;
    File::open(directory)?.sync_all()
}
pub(crate) fn remove(directory: &Path) -> std::io::Result<()> {
    match fs::remove_file(directory.join("recovery.json")) {
        Ok(()) => File::open(directory)?.sync_all(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
fn private(path: &Path, directory: bool) -> std::io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if m.is_symlink()
        || (directory && !m.is_dir())
        || (!directory && !m.is_file())
        || m.uid() != rustix::process::geteuid().as_raw()
        || m.mode() & 0o077 != 0
        || fs::canonicalize(path)? != path
    {
        return Err(invalid());
    }
    Ok(())
}
/// Stop a worker using its private mutual-authentication capability. PID/start evidence
/// only avoids contacting a stale endpoint; it never grants authority to signal a process.
pub fn stop(directory: &Path, attempt: tf_domain::AttemptId) -> std::io::Result<bool> {
    private(directory, true)?;
    let path = directory.join("recovery.json");
    if !path.try_exists()? {
        return Ok(false);
    }
    private(&path, false)?;
    let mut bytes = Vec::new();
    crate::retained_logs::open(&path)?
        .take(8193)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return Err(invalid());
    }
    let r: Record = serde_json::from_slice(&bytes)?;
    if r.attempt != attempt.to_string()
        || r.token.len() != 64
        || !r.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid());
    }
    if r.pid == 0 || crate::ownership::process_start(r.pid).ok().as_deref() != Some(&r.start) {
        remove(directory)?;
        return Ok(false);
    }
    private(r.socket.parent().ok_or_else(invalid)?, true)?;
    if r.socket.file_name().and_then(|n| n.to_str()) != Some("recovery.sock") {
        return Err(invalid());
    }
    let token = (0..32)
        .map(|i| u8::from_str_radix(&r.token[i * 2..i * 2 + 2], 16).map_err(|_| invalid()))
        .collect::<std::io::Result<Vec<_>>>()?;
    let began = Instant::now();
    let mut channel = loop {
        match UnixStream::connect(&r.socket) {
            Ok(channel) => break channel,
            Err(error) => {
                if crate::ownership::process_start(r.pid).ok().as_deref() != Some(&r.start) {
                    remove(directory)?;
                    return Ok(false);
                }
                if began.elapsed() >= Duration::from_secs(2) {
                    return Err(error);
                }
                // The coordinator may have died between spawn and worker listener startup.
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    channel.set_read_timeout(Some(Duration::from_secs(2)))?;
    channel.set_write_timeout(Some(Duration::from_secs(2)))?;
    let handshake = (|| {
        channel.write_all(&token)?;
        let mut proof = [0; 32];
        channel.read_exact(&mut proof)?;
        if proof.as_slice() != token {
            return Err(invalid());
        }
        channel.write_all(b"K")
    })();
    if let Err(error) = handshake {
        // The worker may concurrently detect coordinator loss and clean its own
        // group. That completed cleanup is not an authentication failure to retry.
        if crate::ownership::process_start(r.pid).ok().as_deref() != Some(&r.start) {
            remove(directory)?;
            return Ok(false);
        }
        return Err(error);
    }
    let began = Instant::now();
    while crate::ownership::process_start(r.pid).ok().as_deref() == Some(&r.start) {
        if began.elapsed() >= Duration::from_secs(10) {
            return Err(invalid());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    remove(directory)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "Private synthetic process recovery fixtures"
    )]
    use super::*;
    #[test]
    fn stale_identity_never_signals_an_unrelated_live_process() {
        let scratch = crate::discovery::Scratch::create().unwrap();
        let attempt = tf_domain::AttemptId::from_bytes([41; 16]);
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        persist(
            scratch.path(),
            &attempt.to_string(),
            child.id(),
            "a different start",
            scratch.path().join("recovery.sock"),
            &"ab".repeat(32),
        )
        .unwrap();
        assert!(!stop(scratch.path(), attempt).unwrap());
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }
    #[test]
    fn matching_pid_without_authenticated_endpoint_cannot_authorize_a_signal() {
        let scratch = crate::discovery::Scratch::create().unwrap();
        let attempt = tf_domain::AttemptId::from_bytes([42; 16]);
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        persist(
            scratch.path(),
            &attempt.to_string(),
            child.id(),
            &crate::ownership::process_start(child.id()).unwrap(),
            scratch.path().join("recovery.sock"),
            &"cd".repeat(32),
        )
        .unwrap();
        assert!(stop(scratch.path(), attempt).is_err());
        assert!(child.try_wait().unwrap().is_none());
        assert!(scratch.path().join("recovery.json").exists());
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
