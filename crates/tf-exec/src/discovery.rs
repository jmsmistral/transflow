//! One authenticated discovery subprocess. Never invokes producer functions.
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt},
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tf_protocol::{
    MessageType, Operation, Session, canonical::file_digest, decode_payload, validate_document,
};

/// Safe operational failure; arbitrary subprocess output is discarded.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// Filesystem or process failure.
    #[error("Discovery could not access its private files or matched interpreter")]
    Io(#[from] std::io::Error),
    /// Invalid worker contract or ordering.
    #[error("Discovery returned an invalid or incomplete result")]
    Protocol,
    /// Finite coordinator deadline expired.
    #[error("Discovery exceeded its deadline; the worker process group was stopped")]
    Timeout,
    /// Worker supplied a bounded schema-validated diagnostic.
    #[error("Discovery failed: {0}")]
    Rejected(String),
}
impl From<tf_protocol::ProtocolError> for DiscoveryError {
    fn from(_: tf_protocol::ProtocolError) -> Self {
        Self::Protocol
    }
}
/// Operating-system random bytes, used only for identities and channel authentication.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], std::io::Error> {
    let mut bytes = [0; N];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}
/// New UUID v4 text. Domain callers parse it into their specific identity type.
pub fn random_id() -> Result<String, std::io::Error> {
    let mut bytes = random_bytes::<16>()?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}
/// Private request scratch directory, outside the workspace and removed on every return.
pub struct Scratch(PathBuf);
impl Scratch {
    /// Use the short system temporary root because Unix socket paths are bounded.
    pub fn create() -> Result<Self, std::io::Error> {
        let path = Path::new("/tmp").join(format!("tf-{}", random_id()?));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(fs::canonicalize(path)?))
    }
    /// Private directory location.
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.0.wait();
    }
}
fn read_result(path: &Path) -> Result<Vec<u8>, DiscoveryError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(DiscoveryError::Protocol);
    }
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(DiscoveryError::Protocol);
    }
    Ok(bytes)
}
/// Launch only a previously verified managed interpreter. Request and output live in scratch.
/// stdout/stderr go directly to the null device: imports cannot fill pipes or corrupt JSON.
pub fn discover(
    python: &Path,
    scratch: &Scratch,
    mut request: Value,
    timeout: Duration,
) -> Result<Value, DiscoveryError> {
    let token = random_bytes::<32>()?;
    request["auth_token"] = token
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .into();
    request["result_directory"] = scratch.path().to_string_lossy().to_string().into();
    validate_document("DiscoveryRequestV1", &request)?;
    let bytes = serde_json::to_vec(&request).map_err(|_| DiscoveryError::Protocol)?;
    if bytes.len() > 1024 * 1024 {
        return Err(DiscoveryError::Protocol);
    }
    let request_path = scratch.path().join("request.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&request_path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    let socket = scratch.path().join("control.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let mut child = Process(
        Command::new(python)
            .args([
                "-I",
                "-B",
                "-m",
                "transflow_worker",
                "discover",
                "--request",
            ])
            .arg(request_path)
            .arg("--control-socket")
            .arg(socket)
            .current_dir(scratch.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?,
    );
    let deadline = Instant::now() + timeout;
    let startup = deadline.min(Instant::now() + Duration::from_secs(10));
    let mut channel = loop {
        match listener.accept() {
            Ok((channel, _)) => break channel,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        if child.0.try_wait()?.is_some() {
            return Err(DiscoveryError::Protocol);
        }
        if Instant::now() >= startup {
            return Err(DiscoveryError::Timeout);
        }
        thread::sleep(Duration::from_millis(10));
    };
    channel.set_write_timeout(Some(Duration::from_secs(1)))?;
    channel.set_nonblocking(false)?;
    channel.write_all(&token)?;
    channel.set_nonblocking(true)?;
    let mut session = Session::new(
        request["request_id"]
            .as_str()
            .ok_or(DiscoveryError::Protocol)?
            .parse()
            .map_err(|_| DiscoveryError::Protocol)?,
        request["attempt_id"]
            .as_str()
            .ok_or(DiscoveryError::Protocol)?
            .parse()
            .map_err(|_| DiscoveryError::Protocol)?,
        Operation::Discover,
        ["discovery.v1".to_owned()].into(),
    )?;
    let mut digest = None;
    loop {
        let frame = next_frame(&mut channel, deadline)?;
        session.accept(&frame)?;
        match frame.message_type() {
            MessageType::DiscoveryReady => {
                if digest.is_some() || frame.as_json()["message"]["result_path"] != "discovery.json"
                {
                    return Err(DiscoveryError::Protocol);
                }
                digest = frame.as_json()["message"]["result_digest"]
                    .as_str()
                    .map(str::to_owned);
            }
            MessageType::Completed => break,
            MessageType::Error => {
                let diagnostic: Value = serde_json::from_slice(&read_result(
                    &scratch.path().join("discovery-error.json"),
                )?)
                .map_err(|_| DiscoveryError::Protocol)?;
                validate_document("DiscoveryDiagnosticV1", &diagnostic)?;
                let message = diagnostic["message"]
                    .as_str()
                    .ok_or(DiscoveryError::Protocol)?;
                let path = diagnostic["path"].as_str().unwrap_or("unknown source");
                if message.len() > 1024 || path.len() > 1024 {
                    return Err(DiscoveryError::Protocol);
                }
                return Err(DiscoveryError::Rejected(format!(
                    "{path}:{}: {message}",
                    diagnostic["line"].as_u64().unwrap_or(1)
                )));
            }
            MessageType::Hello | MessageType::Heartbeat | MessageType::Phase => {}
            _ => return Err(DiscoveryError::Protocol),
        }
    }
    loop {
        if let Some(status) = child.0.try_wait()? {
            if !status.success() {
                return Err(DiscoveryError::Protocol);
            }
            break;
        }
        if Instant::now() >= deadline {
            return Err(DiscoveryError::Timeout);
        }
        thread::sleep(Duration::from_millis(10));
    }
    let bytes = read_result(&scratch.path().join("discovery.json"))?;
    let actual = file_digest(&mut bytes.as_slice())
        .map_err(|_| DiscoveryError::Protocol)?
        .hex();
    if digest.as_deref() != Some(actual.as_str()) {
        return Err(DiscoveryError::Protocol);
    }
    let result = serde_json::from_slice(&bytes).map_err(|_| DiscoveryError::Protocol)?;
    validate_document("DiscoveryResultV1", &result)?;
    Ok(result)
}

// A nonblocking incremental read keeps the absolute deadline even for a peer that
// dribbles a partial prefix/payload. Socket read timeouts alone reset per syscall.
fn next_frame(
    channel: &mut UnixStream,
    deadline: Instant,
) -> Result<tf_protocol::ControlFrame, DiscoveryError> {
    fn exact(
        channel: &mut UnixStream,
        mut bytes: &mut [u8],
        deadline: Instant,
    ) -> Result<(), DiscoveryError> {
        while !bytes.is_empty() {
            if Instant::now() >= deadline {
                return Err(DiscoveryError::Timeout);
            }
            match channel.read(bytes) {
                Ok(0) => return Err(DiscoveryError::Protocol),
                Ok(n) => {
                    bytes = &mut bytes[n..];
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    let mut prefix = [0; 4];
    exact(channel, &mut prefix, deadline)?;
    let size = u32::from_be_bytes(prefix) as usize;
    if size == 0 || size > tf_protocol::MAX_FRAME_BYTES {
        return Err(DiscoveryError::Protocol);
    }
    let mut payload = vec![0; size];
    exact(channel, &mut payload, deadline)?;
    Ok(decode_payload(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_frames_obey_the_absolute_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let (mut reader, mut writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.write_all(&[0, 0])?;
        let began = Instant::now();
        assert!(matches!(
            next_frame(&mut reader, began + Duration::from_millis(30)),
            Err(DiscoveryError::Timeout)
        ));
        assert!(began.elapsed() < Duration::from_secs(1));
        Ok(())
    }
    #[test]
    fn oversized_frame_is_rejected_before_allocating_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut reader, mut writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.write_all(&u32::MAX.to_be_bytes())?;
        assert!(matches!(
            next_frame(&mut reader, Instant::now() + Duration::from_secs(1)),
            Err(DiscoveryError::Protocol)
        ));
        Ok(())
    }
    #[test]
    fn scratch_is_private_and_removed_on_drop() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::create()?;
        let path = scratch.path().to_owned();
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o700);
        fs::write(path.join("request.json"), b"private")?;
        drop(scratch);
        assert!(!path.exists());
        Ok(())
    }
}
