//! Supervised Polars materialization followed by independent Rust byte validation.
//! This service never publishes. Call on a bounded blocking owner with input leases
//! and resource permits held; required input checks precede this execute operation.
use crate::supervisor::{self, Cancellation, Launch, Report};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use tf_domain::RequestId;
use tf_protocol::{Operation, canonical::artifact_digest, validate_document};
use tf_store::artifacts::{ArtifactError, ArtifactStore, Candidate, VerifiedArtifact};

/// An invalid request/result cannot acquire publication authority.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Matched request/manifest/identity evidence is missing or changed.
    #[error("Polars execution requires matching pinned inputs and candidate evidence")]
    Evidence,
    /// Worker failed; inspect the separately retained bounded report/logs.
    #[error("Polars worker failed before a validated candidate was available")]
    Worker,
    /// Candidate bytes, normalization or path containment failed.
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    /// Private result file could not be read.
    #[error("Polars result files could not be inspected")]
    Io(#[from] std::io::Error),
}
/// Candidate is an invisible, closed staging object, ready for separate output checks.
pub struct Completion<'a> {
    /// Retained logs/outcome, including on a candidate verification failure.
    pub report: Option<Report>,
    /// Independently validated exact bytes or typed refusal; never a head/version.
    pub candidate: Result<Candidate<'a>, Error>,
}
fn private_directory(path: &Path) -> Result<File, Error> {
    if path.canonicalize()? != path {
        return Err(Error::Evidence);
    }
    let file = File::open(path)?;
    let m = file.metadata()?;
    if !m.is_dir() || m.mode() & 0o077 != 0 {
        return Err(Error::Evidence);
    }
    Ok(file)
}
fn manifest(directory: &Path) -> Result<Value, Error> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(directory.join("artifact.json"))?;
    let m = file.metadata()?;
    if !m.is_file() || m.nlink() != 1 || m.len() > 16 * 1024 * 1024 {
        return Err(Error::Evidence);
    }
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(Error::Evidence);
    }
    let result = serde_json::from_slice(&bytes).map_err(|_| Error::Evidence)?;
    validate_document("ArtifactManifestV1", &result).map_err(|_| Error::Evidence)?;
    Ok(result)
}
/// Execute one accepted producer. Inputs are already selected/leased; every binding
/// must match a strictly verified immutable object (including repeated aliases).
/// Caller verifies the installed environment with T025 before admission. Publication
/// and expectation evaluation require their separate services after this returns.
pub fn run<'a>(
    artifacts: &'a ArtifactStore,
    launch: Launch,
    inputs: &[VerifiedArtifact],
    cancel: Cancellation,
    staging: RequestId,
) -> Completion<'a> {
    let mut report = None;
    let candidate = materialize(artifacts, launch, inputs, cancel, staging, &mut report);
    Completion { report, candidate }
}
fn materialize<'a>(
    artifacts: &'a ArtifactStore,
    launch: Launch,
    inputs: &[VerifiedArtifact],
    cancel: Cancellation,
    staging: RequestId,
    report: &mut Option<Report>,
) -> Result<Candidate<'a>, Error> {
    if launch.operation != Operation::Execute || launch.request_schema != "PolarsExecutionRequestV1"
    {
        return Err(Error::Evidence);
    }
    validate_document("PolarsExecutionRequestV1", &launch.request).map_err(|_| Error::Evidence)?;
    let request = launch.request.clone();
    let threads = launch
        .reservation
        .as_ref()
        .map(|p| p.threads())
        .unwrap_or(launch.threads);
    if request["threads"].as_str() != Some(&threads.to_string()) {
        return Err(Error::Evidence);
    }
    let bindings = request["inputs"].as_array().ok_or(Error::Evidence)?;
    if bindings.len() != inputs.len() {
        return Err(Error::Evidence);
    }
    for (binding, input) in bindings.iter().zip(inputs) {
        let hex = input.digest().hex();
        let path = input
            .workspace()
            .join(".transflow/runtime/objects")
            .join(&hex[..2])
            .join(&hex);
        if binding["artifact_digest"].as_str() != Some(&hex)
            || binding["manifest"] != *input.manifest()
            || binding["artifact_root"].as_str() != path.to_str()
        {
            return Err(Error::Evidence);
        }
        // Recheck before process launch; immutable bindings/leases remain the caller's.
        let store = ArtifactStore::open(input.workspace())?;
        if store.verify(input.digest())?.manifest() != input.manifest() {
            return Err(Error::Evidence);
        }
    }
    let directory = PathBuf::from(
        request["result_directory"]
            .as_str()
            .ok_or(Error::Evidence)?,
    );
    let root = private_directory(&directory)?;
    if directory.join("artifact.json").try_exists()?
        || directory.join("part-00000.parquet").try_exists()?
    {
        return Err(Error::Evidence);
    }
    let outcome = supervisor::run(launch, cancel);
    let success = outcome.outcome.is_ok();
    *report = Some(outcome);
    if !success {
        return Err(Error::Worker);
    }
    let outcome = report.as_ref().ok_or(Error::Evidence)?;
    let result = outcome.result.as_ref().ok_or(Error::Evidence)?.as_json();
    if result["message"]["manifest_path"] != "artifact.json" {
        return Err(Error::Evidence);
    }
    let value = manifest(&directory)?;
    let digest = artifact_digest(&value).map_err(|_| Error::Evidence)?;
    if result["message"]["artifact_digest"].as_str() != Some(&digest.hex())
        || value["files"].as_array().map(Vec::len) != Some(1)
        || value["files"][0]["path"] != "part-00000.parquet"
        || value["writer"]["engine"] != "polars"
        || value["writer"]["compression"] != request["compression"]
        || value["writer"]["row_group_size"] != request["row_group_size"]
    {
        return Err(Error::Evidence);
    }
    let current = fs::metadata(&directory)?;
    let pinned = root.metadata()?;
    if current.dev() != pinned.dev() || current.ino() != pinned.ino() {
        return Err(Error::Evidence);
    }
    let candidate = artifacts.prepare(
        &root,
        &["part-00000.parquet".into()],
        value["writer"].clone(),
        staging,
        |_| Ok(()),
    )?;
    if candidate.manifest() != &value || candidate.digest()? != digest {
        return Err(Error::Evidence);
    }
    let declared = &request["producer"]["output"]["schema"];
    if !declared.is_null() && declared != &candidate.manifest()["logical_schema"] {
        return Err(Error::Evidence);
    }
    Ok(candidate)
}
