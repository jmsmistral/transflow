//! Bounded byte logs. Redaction holds only the longest possible secret suffix.
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::AsFd,
};

const HEADERS: &[&[u8]] = &[
    b"authorization:",
    b"proxy-authorization:",
    b"cookie:",
    b"set-cookie:",
    b"x-api-key:",
];

pub const MARKER: &[u8] = b"\n[transflow: log truncated]\n";

/// Retained bytes and exact raw input accounting, independently for each stream.
#[derive(Default)]
pub struct Log {
    /// Redacted prefix, with an explicit marker when retention was exhausted.
    pub bytes: Vec<u8>,
    /// Total raw bytes drained (saturating only at u64::MAX).
    pub observed_bytes: u64,
    /// At least one redacted output byte exceeded the retention budget.
    pub truncated: bool,
}
impl std::fmt::Debug for Log {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Log")
            .field("retained_bytes", &self.bytes.len())
            .field("observed_bytes", &self.observed_bytes)
            .field("truncated", &self.truncated)
            .finish()
    }
}
pub struct Drain<R> {
    reader: R,
    pub log: Log,
    pending: Vec<u8>,
    secrets: Vec<Vec<u8>>,
    limit: usize,
    file: Option<File>,
    pub eof: bool,
    sensitive_line: bool,
}
impl<R: Read + AsFd> Drain<R> {
    pub fn new(
        reader: R,
        limit: usize,
        secrets: Vec<Vec<u8>>,
        file: Option<File>,
    ) -> io::Result<Self> {
        let flags = rustix::fs::fcntl_getfl(&reader)?;
        rustix::fs::fcntl_setfl(&reader, flags | rustix::fs::OFlags::NONBLOCK)?;
        Ok(Self {
            reader,
            log: Log::default(),
            pending: Vec::new(),
            secrets,
            limit,
            file,
            eof: false,
            sensitive_line: false,
        })
    }
    // A fixed per-tick budget prevents a flooding stream from starving control/cancel.
    pub fn tick(&mut self) -> io::Result<()> {
        let mut buffer = [0; 8192];
        for _ in 0..8 {
            match self.reader.read(&mut buffer) {
                Ok(0) => {
                    self.eof = true;
                    self.redact(true)?;
                    break;
                }
                Ok(n) => {
                    self.log.observed_bytes = self.log.observed_bytes.saturating_add(n as u64);
                    if !self.log.truncated {
                        self.pending.extend_from_slice(&buffer[..n]);
                        self.redact(false)?;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn redact(&mut self, flush: bool) -> io::Result<()> {
        let mut used = 0;
        let mut output = Vec::with_capacity(self.pending.len());
        while used < self.pending.len() {
            let remaining = &self.pending[used..];
            if self.sensitive_line {
                if remaining[0] == b'\n' || remaining[0] == b'\r' {
                    self.sensitive_line = false;
                    output.push(remaining[0]);
                }
                used += 1;
                continue;
            }
            if !flush
                && HEADERS.iter().any(|h| {
                    h.len() > remaining.len()
                        && h[..remaining.len()].eq_ignore_ascii_case(remaining)
                })
            {
                break;
            }
            if let Some(header) = HEADERS.iter().find(|h| {
                remaining.len() >= h.len() && remaining[..h.len()].eq_ignore_ascii_case(h)
            }) {
                output.extend_from_slice(&remaining[..header.len()]);
                output.extend_from_slice(b" [REDACTED]");
                used += header.len();
                self.sensitive_line = true;
                continue;
            }
            // Wait if this may be a split secret, even when a shorter secret matches.
            if !flush
                && self
                    .secrets
                    .iter()
                    .any(|s| s.len() > remaining.len() && s.starts_with(remaining))
            {
                break;
            }
            if let Some(size) = self
                .secrets
                .iter()
                .filter(|s| remaining.starts_with(s))
                .map(Vec::len)
                .max()
            {
                output.extend_from_slice(b"[REDACTED]");
                used += size;
            } else {
                let byte = self.pending[used];
                output.push(byte);
                used += 1;
            }
        }
        self.pending.drain(..used);
        self.emit(&output)?;
        Ok(())
    }
    fn emit(&mut self, bytes: &[u8]) -> io::Result<()> {
        // Reserve marker space inside the byte cap; cap includes all persisted bytes.
        let available = self
            .limit
            .saturating_sub(MARKER.len())
            .saturating_sub(self.log.bytes.len());
        let n = bytes.len().min(available);
        self.log.bytes.extend_from_slice(&bytes[..n]);
        if let Some(file) = &mut self.file {
            file.write_all(&bytes[..n])?;
        }
        if n < bytes.len() {
            self.log.truncated = true;
        }
        Ok(())
    }
    pub fn finish(mut self) -> io::Result<Log> {
        self.redact(true)?;
        if self.log.truncated {
            self.log.bytes.extend_from_slice(MARKER);
            if let Some(file) = &mut self.file {
                file.write_all(MARKER)?;
            }
        }
        if let Some(file) = self.file {
            file.sync_all()?;
        }
        Ok(self.log)
    }
}
