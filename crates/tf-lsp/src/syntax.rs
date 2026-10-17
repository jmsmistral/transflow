//! Bounded isolated stdlib parser. Producer source is data, never imported or evaluated.
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
pub(super) fn inspect(python: &Path, text: &str) -> Result<Value, &'static str> {
    if text.len() > 524_288 {
        return Err("Document exceeds the 512 KiB editor limit");
    }
    let mut child=Command::new(python).args(["-I","-S","-c",include_str!("syntax.py")]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_| "Python 3.14 syntax parser unavailable; configure --python or the workspace interpreter")?;
    let input = serde_json::to_vec(text).map_err(|_| "Cannot encode document")?;
    let mut stdin = child.stdin.take().ok_or("Parser input unavailable")?;
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let stdout = child.stdout.take().ok_or("Parser output unavailable")?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout
            .take(super::protocol::MAX_FRAME as u64 + 1)
            .read_to_end(&mut out)
            .map(|_| out)
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let _ = writer.join();
    let out = reader
        .join()
        .map_err(|_| "Syntax parser output unavailable")?
        .map_err(|_| "Syntax parser output unavailable")?;
    if !status || out.len() > super::protocol::MAX_FRAME {
        return Err("Syntax parser failed, timed out or exceeded document/reference limits");
    }
    serde_json::from_slice(&out).map_err(|_| "Syntax parser returned invalid metadata")
}
