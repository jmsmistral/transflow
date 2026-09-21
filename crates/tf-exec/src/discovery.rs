//! One authenticated discovery subprocess. Never invokes producer functions.
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tf_protocol::{Operation, canonical::file_digest, validate_document};

/// Safe operational failure; retained subprocess logs are never implicitly displayed.
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
    /// Failure evidence includes both bounded streams, available to diagnostic callers.
    #[error("Discovery failed: {message}")]
    Supervised {
        /// Safe heading or bounded validated source diagnostic.
        message: String,
        /// Process and log evidence, not automatically printed.
        report: Box<crate::supervisor::Report>,
    },
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
/// Launch a verified interpreter through the shared owner and retain bounded logs.
/// Inspection never forwards import stdout/stderr into the CLI response stream.
pub fn discover(
    python: &Path,
    scratch: &Scratch,
    mut request: Value,
    timeout: Duration,
) -> Result<Value, DiscoveryError> {
    if !request.is_object() {
        return Err(DiscoveryError::Protocol);
    }
    request["result_directory"] = scratch.path().to_string_lossy().to_string().into();
    let report = crate::supervisor::run(
        crate::supervisor::Launch {
            python: python.to_owned(),
            operation: Operation::Discover,
            request_schema: "DiscoveryRequestV1",
            request,
            policy: crate::supervisor::Policy {
                operation_timeout: Some(timeout),
                ..Default::default()
            },
            log_directory: None,
            redact: Vec::new(),
        },
        crate::supervisor::Cancellation::default(),
    );
    if let Err(failure) = report.outcome {
        if failure == crate::supervisor::Failure::Worker {
            let diagnostic: Value =
                serde_json::from_slice(&read_result(&scratch.path().join("discovery-error.json"))?)
                    .map_err(|_| DiscoveryError::Protocol)?;
            validate_document("DiscoveryDiagnosticV1", &diagnostic)?;
            let message = diagnostic["message"]
                .as_str()
                .ok_or(DiscoveryError::Protocol)?;
            let path = diagnostic["path"].as_str().unwrap_or("unknown source");
            if message.len() > 1024 || path.len() > 1024 {
                return Err(DiscoveryError::Protocol);
            }
            return Err(DiscoveryError::Supervised {
                message: format!(
                    "{path}:{}: {message}",
                    diagnostic["line"].as_u64().unwrap_or(1)
                ),
                report: Box::new(report),
            });
        }
        return Err(DiscoveryError::Supervised {
            message: failure.to_string(),
            report: Box::new(report),
        });
    }
    let result_frame = report.result.as_ref().ok_or(DiscoveryError::Protocol)?;
    if result_frame.as_json()["message"]["result_path"] != "discovery.json" {
        return Err(DiscoveryError::Protocol);
    }
    let digest = result_frame.as_json()["message"]["result_digest"].as_str();
    let bytes = read_result(&scratch.path().join("discovery.json"))?;
    let actual = file_digest(&mut bytes.as_slice())
        .map_err(|_| DiscoveryError::Protocol)?
        .hex();
    if digest != Some(actual.as_str()) {
        return Err(DiscoveryError::Protocol);
    }
    let result = serde_json::from_slice(&bytes).map_err(|_| DiscoveryError::Protocol)?;
    validate_document("DiscoveryResultV1", &result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
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
