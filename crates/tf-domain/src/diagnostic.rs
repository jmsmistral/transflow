//! Human explanations, safe text and stable machine categories. No I/O or raw error dumps.
use crate::RequestId;
use std::fmt;

/// Rejected diagnostic construction; offending input is never echoed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticError;
impl fmt::Display for DiagnosticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Diagnostic content exceeds its limits or has an invalid source range")
    }
}
impl std::error::Error for DiagnosticError {}

/// Stable initial diagnostic codes, independent of headings or localization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCode {
    /// A local dependency cycle blocks validation.
    GraphCycle,
    /// Invalid CLI syntax or option value.
    CliUsage,
    /// Requested operation is unavailable in this build.
    UnsupportedCommand,
    /// An operation failed; a safe cause explains why.
    OperationFailed,
    /// The user interrupted or canceled a waiting operation.
    Interrupted,
    /// A domain transition violates its state/evidence contract.
    StateTransition,
}
impl DiagnosticCode {
    /// Stable JSON/log code. Normal human headings do not begin with it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GraphCycle => "TF_GRAPH_CYCLE",
            Self::CliUsage => "TF_CLI_USAGE",
            Self::UnsupportedCommand => "TF_COMMAND_UNAVAILABLE",
            Self::OperationFailed => "TF_OPERATION_FAILED",
            Self::Interrupted => "TF_INTERRUPTED",
            Self::StateTransition => "TF_STATE_TRANSITION",
        }
    }
}
/// The intentionally small shell exit contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitStatus {
    /// Requested operation succeeded (including accepted warnings).
    Success,
    /// Operational, validation or execution failure.
    Failure,
    /// Invalid invocation.
    Usage,
    /// Explicit user interruption of a waiting command.
    Interrupted,
}
impl ExitStatus {
    /// Numeric process status.
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Failure => 1,
            Self::Usage => 2,
            Self::Interrupted => 130,
        }
    }
}
/// Sanitized printable text. Construction requires an explicit redaction policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafeText(String);
impl SafeText {
    /// Already redacted and escaped text, safe for terminal text and JSON strings.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
/// Known-value redaction plus conservative sensitive-line filtering.
/// Best effort only: arbitrary unknown secrets cannot be detected reliably.
#[derive(Default)]
pub struct Redactor {
    secrets: Vec<String>,
}
impl Redactor {
    /// At most 128 nonempty known credentials, each at most 4096 UTF-8 bytes.
    /// Secrets are deliberately excluded from Debug/Display implementations.
    pub fn new(secrets: Vec<String>) -> Result<Self, DiagnosticError> {
        if secrets.len() > 128 || secrets.iter().any(|s| s.is_empty() || s.len() > 4096) {
            return Err(DiagnosticError);
        }
        let mut secrets = secrets;
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Ok(Self { secrets })
    }
    /// Redact before escaping, never truncate a credential to evade exact matching.
    /// C0/C1 controls, bidi controls and Unicode line separators become visible escapes.
    pub fn text(&self, input: &str) -> Result<SafeText, DiagnosticError> {
        if input.is_empty() || input.len() > 4096 {
            return Err(DiagnosticError);
        }
        let mut redacted = String::new();
        let mut rest = input;
        while !rest.is_empty() {
            if let Some(secret) = self.secrets.iter().find(|s| rest.starts_with(s.as_str())) {
                redacted.push_str("[REDACTED]");
                rest = rest.get(secret.len()..).ok_or(DiagnosticError)?;
            } else if let Some(ch) = rest.chars().next() {
                redacted.push(ch);
                rest = rest.get(ch.len_utf8()..).ok_or(DiagnosticError)?;
            }
        }
        let mut output = String::new();
        for (index, line) in redacted.split('\n').enumerate() {
            if index > 0 {
                output.push_str("\\u{a}");
            }
            let lower = line.to_ascii_lowercase();
            if [
                "password",
                "authorization",
                "cookie",
                "api_key",
                "api-key",
                "access_token",
                "secret=",
                "token=",
                "credential=",
            ]
            .iter()
            .any(|key| lower.contains(key))
            {
                output.push_str("[REDACTED sensitive detail]");
            } else {
                for ch in line.chars() {
                    if unsafe_character(ch) {
                        output.push_str(&format!("\\u{{{:x}}}", ch as u32));
                    } else {
                        output.push(ch);
                    }
                }
            }
        }
        if output.len() > 32768 {
            return Err(DiagnosticError);
        }
        Ok(SafeText(output))
    }
}
/// True for text controls that may alter terminal output or visual ordering.
pub fn unsafe_character(ch: char) -> bool {
    ch.is_control()
        || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}
/// One-based line and Unicode-scalar column, with an exclusive end position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRange {
    path: SafeText,
    start: (u32, u32),
    end: (u32, u32),
}
impl SourceRange {
    /// Check ordering/positive coordinates; no source file is opened.
    pub fn new(
        path: SafeText,
        start: (u32, u32),
        end: (u32, u32),
    ) -> Result<Self, DiagnosticError> {
        if start.0 == 0 || start.1 == 0 || end.0 == 0 || end.1 == 0 || start > end {
            return Err(DiagnosticError);
        }
        Ok(Self { path, start, end })
    }
    /// Safe display path.
    pub fn path(&self) -> &SafeText {
        &self.path
    }
    /// Inclusive start (line, scalar column).
    pub fn start(&self) -> (u32, u32) {
        self.start
    }
    /// Exclusive end (line, scalar column).
    pub fn end(&self) -> (u32, u32) {
        self.end
    }
}
/// Selected request context; absence is explicit, never inferred from a dataset path.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestContext {
    /// Selected workspace, when known.
    pub workspace: Option<SafeText>,
    /// Selected captured source/ref, when known.
    pub source: Option<SafeText>,
    /// Correlation identity, when allocated by the calling service.
    pub request_id: Option<RequestId>,
}
/// Immutable explanation with bounded safe nested causes and source references.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    code: DiagnosticCode,
    heading: SafeText,
    reason: SafeText,
    remediation: SafeText,
    sources: Vec<SourceRange>,
    affected: Vec<SafeText>,
    causes: Vec<Diagnostic>,
}
impl Diagnostic {
    /// Construct a human-first explanation from sanitized text.
    pub fn new(
        code: DiagnosticCode,
        heading: SafeText,
        reason: SafeText,
        remediation: SafeText,
    ) -> Self {
        Self {
            code,
            heading,
            reason,
            remediation,
            sources: Vec::new(),
            affected: Vec::new(),
            causes: Vec::new(),
        }
    }
    /// Attach up to 32 safe ranges.
    pub fn with_sources(mut self, sources: Vec<SourceRange>) -> Result<Self, DiagnosticError> {
        if sources.len() > 32 {
            return Err(DiagnosticError);
        }
        self.sources = sources;
        Ok(self)
    }
    /// Attach up to 32 affected display references, not identity lookup instructions.
    pub fn with_affected(mut self, affected: Vec<SafeText>) -> Result<Self, DiagnosticError> {
        if affected.len() > 32 {
            return Err(DiagnosticError);
        }
        self.affected = affected;
        Ok(self)
    }
    /// Attach up to 8 children per node, depth 4 and 64 nodes for the whole explanation.
    pub fn with_causes(mut self, causes: Vec<Self>) -> Result<Self, DiagnosticError> {
        if causes.len() > 8 {
            return Err(DiagnosticError);
        }
        self.causes = causes;
        let (depth, count) = self.size();
        if depth > 4 || count > 64 {
            return Err(DiagnosticError);
        }
        Ok(self)
    }
    fn size(&self) -> (usize, usize) {
        self.causes
            .iter()
            .map(Self::size)
            .fold((1, 1), |(depth, count), (d, n)| {
                (depth.max(d + 1), count + n)
            })
    }
    /// Stable code.
    pub fn code(&self) -> DiagnosticCode {
        self.code
    }
    /// Human heading.
    pub fn heading(&self) -> &SafeText {
        &self.heading
    }
    /// Explanation.
    pub fn reason(&self) -> &SafeText {
        &self.reason
    }
    /// Corrective action.
    pub fn remediation(&self) -> &SafeText {
        &self.remediation
    }
    /// Source ranges.
    pub fn sources(&self) -> &[SourceRange] {
        &self.sources
    }
    /// Affected display references.
    pub fn affected(&self) -> &[SafeText] {
        &self.affected
    }
    /// Safe causes; raw error chains are never automatically dumped.
    pub fn causes(&self) -> &[Self] {
        &self.causes
    }
}
