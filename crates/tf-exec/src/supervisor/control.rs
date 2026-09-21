//! Incremental bounded control reads and operation-specific phase validation.
use super::Failure;
use std::{
    io::{self, Read},
    os::unix::net::UnixStream,
};
use tf_protocol::{ControlFrame, MessageType, Operation, Session, decode_payload};

#[derive(Default)]
pub struct Reader {
    prefix: [u8; 4],
    prefix_len: usize,
    payload: Vec<u8>,
    size: usize,
    pub eof: bool,
}
impl Reader {
    pub fn partial(&self) -> bool {
        self.prefix_len != 0
    }
    pub fn next(&mut self, stream: &mut UnixStream) -> Result<Option<ControlFrame>, Failure> {
        // At most one bounded frame per call; the owner services logs between calls.
        loop {
            let target = if self.prefix_len < 4 {
                &mut self.prefix[self.prefix_len..]
            } else {
                &mut self.payload[self.size..]
            };
            match stream.read(target) {
                Ok(0) => {
                    self.eof = true;
                    return if self.prefix_len == 0 {
                        Ok(None)
                    } else {
                        Err(Failure::Protocol)
                    };
                }
                Ok(n) if self.prefix_len < 4 => {
                    self.prefix_len += n;
                    if self.prefix_len == 4 {
                        let len = u32::from_be_bytes(self.prefix) as usize;
                        if len == 0 || len > tf_protocol::MAX_FRAME_BYTES {
                            return Err(Failure::Protocol);
                        }
                        self.payload.resize(len, 0);
                    }
                }
                Ok(n) => {
                    self.size += n;
                    if self.size == self.payload.len() {
                        let frame = decode_payload(&self.payload).map_err(|_| Failure::Protocol)?;
                        self.prefix_len = 0;
                        self.size = 0;
                        self.payload.clear();
                        return Ok(Some(frame));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Failure::Io),
            }
        }
    }
}
pub struct Guard {
    session: Session,
    operation: Operation,
    phase: Option<usize>,
    pub terminal: Option<ControlFrame>,
    pub result: Option<ControlFrame>,
}
impl Guard {
    pub fn new(session: Session, operation: Operation) -> Self {
        Self {
            session,
            operation,
            phase: None,
            terminal: None,
            result: None,
        }
    }
    pub fn accept(&mut self, frame: ControlFrame) -> Result<(), Failure> {
        self.session.accept(&frame).map_err(|_| Failure::Protocol)?;
        let phases: &[&str] = match self.operation {
            Operation::Discover => &["setup", "discovering"],
            Operation::Execute => &[
                "setup",
                "validating_inputs",
                "running",
                "materializing",
                "validating_outputs",
            ],
            Operation::EvaluateChecks => &["setup", "validating_inputs", "validating_outputs"],
            Operation::QueryPreview => &["setup", "querying"],
            Operation::InferLineage => &["setup", "inferring_lineage"],
            Operation::InspectEnvironment => &["setup", "inspecting_environment"],
        };
        match frame.message_type() {
            MessageType::Phase => {
                let phase = frame.as_json()["message"]["phase"]
                    .as_str()
                    .ok_or(Failure::Protocol)?;
                let index = phases
                    .iter()
                    .position(|p| *p == phase)
                    .ok_or(Failure::Protocol)?;
                if self.phase.is_some_and(|old| old >= index) {
                    return Err(Failure::Protocol);
                }
                self.phase = Some(index);
            }
            MessageType::DiscoveryReady
            | MessageType::ArtifactReady
            | MessageType::CheckResults => {
                let allowed = matches!(
                    (self.operation, frame.message_type()),
                    (Operation::Discover, MessageType::DiscoveryReady)
                        | (Operation::Execute, MessageType::ArtifactReady)
                        | (Operation::EvaluateChecks, MessageType::CheckResults)
                );
                let phase = self.phase.and_then(|index| phases.get(index)).copied();
                let correct_phase = matches!(
                    (frame.message_type(), phase),
                    (MessageType::DiscoveryReady, Some("discovering"))
                        | (
                            MessageType::ArtifactReady,
                            Some("materializing" | "validating_outputs")
                        )
                        | (
                            MessageType::CheckResults,
                            Some("validating_inputs" | "validating_outputs")
                        )
                );
                if !allowed || self.result.is_some() || !correct_phase {
                    return Err(Failure::Protocol);
                }
                self.result = Some(frame);
            }
            MessageType::Completed => {
                if self.phase.is_none_or(|phase| phase == 0)
                    || (matches!(
                        self.operation,
                        Operation::Discover | Operation::Execute | Operation::EvaluateChecks
                    ) && self.result.is_none())
                {
                    return Err(Failure::Protocol);
                }
                self.terminal = Some(frame);
            }
            MessageType::Error => self.terminal = Some(frame),
            MessageType::Hello | MessageType::Heartbeat | MessageType::Metric => {}
        }
        Ok(())
    }
}
