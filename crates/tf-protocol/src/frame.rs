//! Strict length-prefixed UTF-8 JSON. Callers supply the dedicated private channel.
use crate::{ProtocolError, validate_document};
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::{
    fmt,
    io::{self, Read, Write},
};
use tf_domain::{AttemptId, RequestId};

/// Maximum JSON payload size, excluding the four-byte prefix.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
/// Current supported protocol major; capability-compatible higher minors may be received.
pub const PROTOCOL_MAJOR: u32 = 1;
/// Initial worker protocol minor.
pub const PROTOCOL_MINOR: u32 = 0;

/// Discriminant of a schema-validated control message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageType {
    /// Initial operation and capability advertisement.
    Hello,
    /// Work phase update.
    Phase,
    /// Liveness signal.
    Heartbeat,
    /// Scalar metric, never dataframe rows.
    Metric,
    /// Staged artifact reference.
    ArtifactReady,
    /// Bounded results-file reference.
    CheckResults,
    /// Captured discovery metadata file.
    DiscoveryReady,
    /// Terminal successful result.
    Completed,
    /// Terminal failure.
    Error,
}
/// Validated immutable frame; construction and output enforce the same schema.
#[derive(Clone, Debug)]
pub struct ControlFrame {
    value: Value,
    request: RequestId,
    attempt: AttemptId,
    sequence: u64,
    kind: MessageType,
    minor: u32,
}
impl ControlFrame {
    /// Construct from decoded JSON, checking shape, version and asserted formats.
    pub fn from_json(value: Value) -> Result<Self, ProtocolError> {
        if value["protocol"]["major"]
            .as_f64()
            .is_some_and(|n| n != f64::from(PROTOCOL_MAJOR))
        {
            return Err(ProtocolError::Version);
        }
        validate_document("ControlFrameV1", &value)?;
        let request = value["request_id"]
            .as_str()
            .ok_or(ProtocolError::InvalidDocument)?
            .parse()
            .map_err(|_| ProtocolError::InvalidDocument)?;
        let attempt = value["attempt_id"]
            .as_str()
            .ok_or(ProtocolError::InvalidDocument)?
            .parse()
            .map_err(|_| ProtocolError::InvalidDocument)?;
        let sequence = value["sequence"]
            .as_str()
            .ok_or(ProtocolError::InvalidDocument)?
            .parse()
            .map_err(|_| ProtocolError::InvalidDocument)?;
        // Schema has already restricted this exact integer to the u32 range.
        let minor = value["protocol"]["minor"]
            .as_f64()
            .ok_or(ProtocolError::InvalidDocument)? as u32;
        let kind = match value["message"]["type"].as_str() {
            Some("hello") => MessageType::Hello,
            Some("phase") => MessageType::Phase,
            Some("heartbeat") => MessageType::Heartbeat,
            Some("metric") => MessageType::Metric,
            Some("artifact_ready") => MessageType::ArtifactReady,
            Some("check_results") => MessageType::CheckResults,
            Some("discovery_ready") => MessageType::DiscoveryReady,
            Some("completed") => MessageType::Completed,
            Some("error") => MessageType::Error,
            _ => return Err(ProtocolError::InvalidDocument),
        };
        Ok(Self {
            value,
            request,
            attempt,
            sequence,
            kind,
            minor,
        })
    }
    /// Immutable, validated carrier for generated clients or explicit field inspection.
    pub fn as_json(&self) -> &Value {
        &self.value
    }
    /// Exact request identity.
    pub fn request_id(&self) -> RequestId {
        self.request
    }
    /// Exact attempt identity.
    pub fn attempt_id(&self) -> AttemptId {
        self.attempt
    }
    /// Monotonic sequence carrier decoded without floating-point conversion.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// Validated message discriminant.
    pub fn message_type(&self) -> MessageType {
        self.kind
    }
    /// Advertised protocol minor; required features are checked separately.
    pub fn minor(&self) -> u32 {
        self.minor
    }
}

// serde_json::Value normally accepts duplicate object keys. Reject them recursively
// while decoding, before schema validation can lose the earlier value.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("strict JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Strict, E> {
                Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(Value::String(v.to_owned())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Strict, E> {
                Ok(Strict(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut values = Vec::new();
                while let Some(Strict(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Strict(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut values = Map::new();
                while let Some((key, Strict(value))) = map.next_entry::<String, Strict>()? {
                    if values.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate object key"));
                    }
                }
                Ok(Strict(Value::Object(values)))
            }
        }
        d.deserialize_any(StrictVisitor)
    }
}
fn bounded_json(value: &Value, depth: usize) -> bool {
    if depth > 64 {
        return false;
    }
    match value {
        Value::Array(items) => items.iter().all(|v| bounded_json(v, depth + 1)),
        Value::Object(items) => items.values().all(|v| bounded_json(v, depth + 1)),
        _ => true,
    }
}
/// Decode one JSON payload. No imports, callbacks or operation dispatch occur here.
pub fn decode_payload(payload: &[u8]) -> Result<ControlFrame, ProtocolError> {
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::Size);
    }
    let Strict(value) = serde_json::from_slice(payload).map_err(|_| ProtocolError::Json)?;
    if !bounded_json(&value, 0) {
        return Err(ProtocolError::Json);
    }
    ControlFrame::from_json(value)
}
fn exact<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<(), ProtocolError> {
    reader.read_exact(buffer).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            ProtocolError::Truncated
        } else {
            ProtocolError::Io(e)
        }
    })
}
/// Read one frame; clean EOF before a prefix is None, partial EOF is an error.
/// This blocking primitive relies on its owning supervisor for deadlines/cancellation.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Option<ControlFrame>, ProtocolError> {
    let mut prefix = [0; 4];
    loop {
        match reader.read(&mut prefix[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ProtocolError::Io(e)),
        }
    }
    exact(reader, &mut prefix[1..])?;
    let size = u32::from_be_bytes(prefix) as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(ProtocolError::Size);
    }
    let mut payload = vec![0; size];
    exact(reader, &mut payload)?;
    decode_payload(&payload).map(Some)
}
struct BoundedBuffer(Vec<u8>);
impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_FRAME_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("frame byte cap"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
/// Encode a frame with a bounded buffer; no bytes are written on encoding failure.
pub fn write_frame<W: Write>(writer: &mut W, frame: &ControlFrame) -> Result<(), ProtocolError> {
    let mut payload = BoundedBuffer(Vec::new());
    serde_json::to_writer(&mut payload, frame.as_json()).map_err(|_| ProtocolError::Size)?;
    let size = u32::try_from(payload.0.len()).map_err(|_| ProtocolError::Size)?;
    writer.write_all(&size.to_be_bytes())?;
    writer.write_all(&payload.0)?;
    Ok(())
}
