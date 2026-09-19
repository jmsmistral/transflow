//! Shared CLI JSON projection. Domain construction supplies sanitized immutable values.
use crate::{ProtocolError, validate_document};
use serde_json::{Value, json};
use tf_domain::diagnostic::{Diagnostic, ExitStatus, RequestContext, SafeText};

/// Initial complete-result envelope format, independent of the worker protocol.
pub const CLI_ENVELOPE_VERSION: u32 = 1;
/// Implemented CLI operations. This does not advertise dataset/coordinator capabilities.
pub const CLI_CAPABILITIES: [&str; 7] = [
    "cli.help",
    "cli.version",
    "cli.diagnostics.v1",
    "workspace.init",
    "env.lock",
    "env.sync",
    "env.check",
];
/// Successful informational operation.
#[derive(Clone, Copy, Debug)]
pub enum InformationKind {
    /// Command help.
    Help,
    /// Initialize or verify a durable workspace.
    WorkspaceInit,
    /// Explicit environment operation.
    Environment,
    /// Installed product version.
    Version,
}
/// Complete immutable result; construction checks schema and aggregate size.
#[derive(Debug)]
pub struct CliEnvelope {
    bytes: Vec<u8>,
    status: ExitStatus,
}
fn context(value: &RequestContext) -> Value {
    json!({"workspace":value.workspace.as_ref().map(SafeText::as_str),
        "source":value.source.as_ref().map(SafeText::as_str),
        "request_id":value.request_id.map(|id| id.to_string())})
}
fn diagnostic(value: &Diagnostic) -> Value {
    json!({"code":value.code().as_str(),"heading":value.heading().as_str(),
        "reason":value.reason().as_str(),"remediation":value.remediation().as_str(),
        "sources":value.sources().iter().map(|s|json!({"path":s.path().as_str(),
            "start":{"line":s.start().0,"column":s.start().1},
            "end":{"line":s.end().0,"column":s.end().1}})).collect::<Vec<_>>(),
        "affected":value.affected().iter().map(SafeText::as_str).collect::<Vec<_>>(),
        "causes":value.causes().iter().map(diagnostic).collect::<Vec<_>>()})
}
fn text_bytes(value: &Diagnostic) -> usize {
    value.heading().as_str().len()
        + value.reason().as_str().len()
        + value.remediation().as_str().len()
        + value
            .sources()
            .iter()
            .map(|s| s.path().as_str().len())
            .sum::<usize>()
        + value
            .affected()
            .iter()
            .map(|s| s.as_str().len())
            .sum::<usize>()
        + value.causes().iter().map(text_bytes).sum::<usize>()
}
impl CliEnvelope {
    /// Successful help/version result. Every dynamic string is sanitized first.
    pub fn success(
        version: &SafeText,
        kind: InformationKind,
        text: &SafeText,
        ctx: &RequestContext,
    ) -> Result<Self, ProtocolError> {
        Self::encode(
            version,
            ExitStatus::Success,
            ctx,
            json!({"kind":match kind {InformationKind::Help=>"help",InformationKind::Version=>"version",InformationKind::WorkspaceInit=>"workspace_init",InformationKind::Environment=>"environment"},"text":text.as_str()}),
            Vec::new(),
        )
    }
    /// A non-success result with at least one diagnostic. JSON is never mixed with logs.
    pub fn failure(
        version: &SafeText,
        status: ExitStatus,
        ctx: &RequestContext,
        errors: &[Diagnostic],
    ) -> Result<Self, ProtocolError> {
        if status == ExitStatus::Success
            || errors.is_empty()
            || errors.len() > 16
            || errors.iter().map(text_bytes).sum::<usize>() > 128 * 1024
        {
            return Err(ProtocolError::InvalidDocument);
        }
        Self::encode(
            version,
            status,
            ctx,
            Value::Null,
            errors.iter().map(diagnostic).collect(),
        )
    }
    fn encode(
        version: &SafeText,
        status: ExitStatus,
        ctx: &RequestContext,
        result: Value,
        diagnostics: Vec<Value>,
    ) -> Result<Self, ProtocolError> {
        let value = json!({"format_version":CLI_ENVELOPE_VERSION,"product_version":version.as_str(),
            "capabilities":CLI_CAPABILITIES,"outcome":match status {ExitStatus::Success=>"success",ExitStatus::Interrupted=>"canceled",_=>"failure"},
            "exit_status":status.code(),"context":context(ctx),"result":result,"diagnostics":diagnostics});
        validate_document("CliEnvelopeV1", &value)?;
        let mut bytes = serde_json::to_vec(&value).map_err(|_| ProtocolError::Json)?;
        if bytes.len() >= 1024 * 1024 {
            return Err(ProtocolError::Size);
        }
        bytes.push(b'\n');
        Ok(Self { bytes, status })
    }
    /// One complete newline-terminated JSON object, already validated and bounded.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Status matching the serialized envelope.
    pub fn status(&self) -> ExitStatus {
        self.status
    }
}
