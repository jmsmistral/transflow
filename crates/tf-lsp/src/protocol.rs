//! Finite LSP Content-Length framing, independent of product/worker envelopes.
use serde_json::Value;
use std::io::{self, BufRead, Read, Write};
pub(super) const MAX_FRAME: usize = 1024 * 1024;
pub(super) fn read(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut size = None;
    let mut total = 0;
    loop {
        let mut raw = Vec::new();
        let n = (&mut *input).take(8193).read_until(b'\n', &mut raw)?;
        if n == 0 && total == 0 {
            return Ok(None);
        }
        total += n;
        if n == 0 || raw.len() > 8192 || total > 16384 || !raw.ends_with(b"\n") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid LSP header",
            ));
        }
        let line = std::str::from_utf8(&raw)
            .map_err(|_| io::ErrorKind::InvalidData)?
            .trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or(io::ErrorKind::InvalidData)?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if size.is_some() {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let n = value
                .trim()
                .parse::<usize>()
                .map_err(|_| io::ErrorKind::InvalidData)?;
            if n == 0 || n > MAX_FRAME {
                return Err(io::ErrorKind::InvalidData.into());
            }
            size = Some(n);
        }
    }
    let mut body = vec![0; size.ok_or(io::ErrorKind::InvalidData)?];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|_| io::ErrorKind::InvalidData.into())
}
pub(super) fn write(output: &mut dyn Write, value: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::ErrorKind::InvalidData.into());
    }
    write!(output, "Content-Length: {}\r\n\r\n", bytes.len())?;
    output.write_all(&bytes)?;
    output.flush()
}
/// Local file URIs only; percent escapes decode before canonical containment checks.
pub(super) fn path(uri: &str) -> Option<std::path::PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    if !encoded.starts_with('/') || encoded.contains(['?', '#']) {
        return None;
    }
    let mut bytes = Vec::new();
    let mut rest = encoded.as_bytes();
    while !rest.is_empty() {
        if rest[0] == b'%' {
            let digits = std::str::from_utf8(rest.get(1..3)?).ok()?;
            let n = u8::from_str_radix(digits, 16).ok()?;
            if n == 0 {
                return None;
            }
            bytes.push(n);
            rest = &rest[3..];
        } else {
            bytes.push(rest[0]);
            rest = &rest[1..];
        }
    }
    Some(std::path::PathBuf::from(String::from_utf8(bytes).ok()?))
}
pub(super) fn uri(path: &std::path::Path) -> Option<String> {
    let mut out = String::from("file://");
    for b in path.to_str()?.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Some(out)
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "Synthetic framing assertions")]
    use super::*;
    #[test]
    fn finite_framing_and_uris() {
        let mut bytes = Vec::new();
        write(&mut bytes, &serde_json::json!({"id":1})).unwrap();
        assert_eq!(read(&mut bytes.as_slice()).unwrap().unwrap()["id"], 1);
        for bad in [
            "Content-Length: 100000000\r\n\r\n",
            "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "Content-Length: 5\r\n\r\n{}",
        ] {
            assert!(read(&mut bad.as_bytes()).is_err());
        }
        let p = std::path::Path::new("/tmp/a space/é.py");
        assert_eq!(path(&uri(p).unwrap()).unwrap(), p);
        assert!(path("file://host/tmp/a").is_none());
        assert!(path("file:///tmp/%00").is_none());
    }
}
