//! Version/capability and request/attempt guards, independent of process launching.
use crate::{ControlFrame, MessageType, PROTOCOL_MINOR, ProtocolError};
use std::{collections::BTreeSet, str::FromStr};
use tf_domain::{AttemptId, RequestId};

/// Requested operation; the codec does not implement or dispatch these operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    /// Definition discovery.
    Discover,
    /// Transform execution.
    Execute,
    /// Canonical checks.
    EvaluateChecks,
    /// Bounded interactive query.
    QueryPreview,
    /// Lineage inference.
    InferLineage,
    /// Environment inspection.
    InspectEnvironment,
}
impl FromStr for Operation {
    type Err = ProtocolError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "discover" => Self::Discover,
            "execute" => Self::Execute,
            "evaluate_checks" => Self::EvaluateChecks,
            "query_preview" => Self::QueryPreview,
            "infer_lineage" => Self::InferLineage,
            "inspect_environment" => Self::InspectEnvironment,
            _ => return Err(ProtocolError::InvalidDocument),
        })
    }
}
/// Immutable negotiated connection facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Negotiated {
    minor: u32,
    capabilities: BTreeSet<String>,
}
impl Negotiated {
    /// Lower of the local and advertised compatible minor versions.
    pub fn minor(&self) -> u32 {
        self.minor
    }
    /// Explicit capability intersection; no implied support from a minor number.
    pub fn capabilities(&self) -> &BTreeSet<String> {
        &self.capabilities
    }
}
/// Receiver state for one request/attempt and operation. Rejected input never advances it.
#[derive(Debug)]
pub struct Session {
    request: RequestId,
    attempt: AttemptId,
    operation: Operation,
    local: BTreeSet<String>,
    negotiated: Option<Negotiated>,
    last: Option<u64>,
    terminal: bool,
}
impl Session {
    /// Bind explicit identities before receiving hello; no code or source is loaded.
    pub fn new(
        request: RequestId,
        attempt: AttemptId,
        operation: Operation,
        capabilities: BTreeSet<String>,
    ) -> Result<Self, ProtocolError> {
        if capabilities.iter().any(|s| {
            !matches!(
                s.as_str(),
                "diagnostic.note.v1"
                    | "discovery.v1"
                    | "polars.execute.v1"
                    | "expectation.ast.v1"
                    | "expectation.core.v1"
                    | "duckdb.checks.v1"
                    | "duckdb.samples.v1"
            )
        }) {
            return Err(ProtocolError::Capability);
        }
        Ok(Self {
            request,
            attempt,
            operation,
            local: capabilities,
            negotiated: None,
            last: None,
            terminal: false,
        })
    }
    /// Accepted hello facts, absent until a complete valid handshake is received.
    pub fn negotiated(&self) -> Option<&Negotiated> {
        self.negotiated.as_ref()
    }
    /// Validate identity, increasing sequence, hello, negotiated features and terminal state.
    /// Nonce authentication and subprocess/log supervision remain launcher responsibilities.
    pub fn accept(&mut self, frame: &ControlFrame) -> Result<(), ProtocolError> {
        if frame.request_id() != self.request || frame.attempt_id() != self.attempt {
            return Err(ProtocolError::Identity);
        }
        if self.last.is_some_and(|last| frame.sequence() <= last) {
            return Err(ProtocolError::Sequence);
        }
        if self.terminal {
            return Err(ProtocolError::Order);
        }
        let mut negotiated = self.negotiated.clone();
        if frame.message_type() == MessageType::Hello {
            if negotiated.is_some() {
                return Err(ProtocolError::Order);
            }
            let message = &frame.as_json()["message"];
            if message["operation"]
                .as_str()
                .ok_or(ProtocolError::Order)?
                .parse::<Operation>()?
                != self.operation
            {
                return Err(ProtocolError::Order);
            }
            let values = message["capabilities"]
                .as_array()
                .ok_or(ProtocolError::InvalidDocument)?;
            let peer: BTreeSet<_> = values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if peer.len() != values.len() {
                return Err(ProtocolError::Capability);
            }
            negotiated = Some(Negotiated {
                minor: PROTOCOL_MINOR, // Local minor zero is the lowest compatible minor.
                capabilities: self.local.intersection(&peer).cloned().collect(),
            });
        }
        let facts = negotiated.as_ref().ok_or(ProtocolError::Order)?;
        if frame.message_type() == MessageType::DiscoveryReady {
            if self.operation != Operation::Discover {
                return Err(ProtocolError::Order);
            }
            if !facts.capabilities.contains("discovery.v1") {
                return Err(ProtocolError::Capability);
            }
        }
        let required = frame.as_json()["required_capabilities"]
            .as_array()
            .ok_or(ProtocolError::InvalidDocument)?;
        let extensions = frame.as_json()["extensions"]
            .as_array()
            .ok_or(ProtocolError::InvalidDocument)?;
        if required
            .iter()
            .filter_map(|v| v.as_str())
            .chain(extensions.iter().filter_map(|v| v["capability"].as_str()))
            .any(|name| !facts.capabilities.contains(name))
        {
            return Err(ProtocolError::Capability);
        }
        self.negotiated = negotiated;
        self.last = Some(frame.sequence());
        self.terminal = matches!(
            frame.message_type(),
            MessageType::Completed | MessageType::Error
        );
        Ok(())
    }
}
