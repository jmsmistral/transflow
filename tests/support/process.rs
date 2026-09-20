//! Supervised direct test children. Fixture children do not spawn descendants.
//! Stdout is only a test readiness pipe, not the future worker control protocol.
use super::faults::WATCHDOG;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread::{self, JoinHandle},
};

pub struct TestChild {
    child: Child,
    input: Option<ChildStdin>,
    ready: mpsc::Receiver<io::Result<()>>,
    reader: Option<JoinHandle<()>>,
    done: mpsc::Receiver<io::Result<()>>,
}

impl TestChild {
    pub fn spawn(root: &Path) -> io::Result<Self> {
        Self::spawn_named(root, "infrastructure_child", None)
    }
    /// Reuse bounded readiness/control/reaping for an actual service crash fixture.
    pub fn spawn_named(root: &Path, entry: &str, boundary: Option<&str>) -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .args(["--exact", entry, "--nocapture", "--format=terse"])
            .env("TRANSFLOW_TEST_CHILD_ROOT", root)
            .env("TRANSFLOW_TEST_CHILD_BOUNDARY", boundary.unwrap_or(""))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(root.join("child.stderr"))?)
            .spawn()?;
        let input = child.stdin.take();
        let output = child.stdout.take();
        let (sender, ready) = mpsc::sync_channel(1);
        let (finished, done) = mpsc::sync_channel(1);
        let mut fixture = Self {
            child,
            input,
            ready,
            reader: None,
            done,
        };
        let reader = thread::Builder::new()
            .name("fixture-output".into())
            .spawn(move || {
                let result = (|| {
                    let output =
                        output.ok_or_else(|| io::Error::other("Missing fixture stdout"))?;
                    let mut reader = BufReader::new(output).take(4096);
                    let mut line = String::new();
                    let mut seen_ready = false;
                    while reader.read_line(&mut line)? > 0 {
                        if line.trim() == "TRANSFLOW_FIXTURE_READY" && !seen_ready {
                            seen_ready = true;
                            let _ = sender.send(Ok(()));
                        }
                        line.clear();
                    }
                    if !seen_ready {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Child exited before readiness",
                        ));
                    }
                    if reader.limit() == 0 {
                        return Err(io::Error::other("Fixture stdout exceeded 4096-byte budget"));
                    }
                    Ok(())
                })();
                if result.is_err() {
                    let _ =
                        sender.try_send(Err(io::Error::other("Fixture readiness/output failed")));
                }
                let _ = finished.send(result);
            })?;
        fixture.reader = Some(reader);
        Ok(fixture)
    }
    pub fn wait_ready(&self) -> io::Result<()> {
        self.ready.recv_timeout(WATCHDOG).map_err(|error| {
            io::Error::new(io::ErrorKind::TimedOut, format!("Child readiness: {error}"))
        })?
    }
    pub fn release(&mut self, command: u8) -> io::Result<()> {
        self.input
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "Child control pipe closed"))?
            .write_all(&[command])
    }
    pub fn disconnect(&mut self) {
        self.input.take();
    }
    pub fn kill_and_wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(status);
        }
        let termination = self.child.kill();
        // Even a failed signal attempt must not skip reaping the owned child.
        let status = self.child.wait()?;
        termination?;
        Ok(status)
    }
    /// Wait with a bounded watchdog; a hung child is killed and reaped.
    pub fn finish(mut self) -> io::Result<ExitStatus> {
        self.done.recv_timeout(WATCHDOG).map_err(|error| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Fixture child did not close output: {error}"),
            )
        })??;
        // These controlled fixtures retain stdout until process exit. EOF is the
        // completion event; wait reaps the exited direct child. General worker
        // supervision (including descendants) belongs to the execution layer.
        self.child.wait()
    }
}

impl Drop for TestChild {
    fn drop(&mut self) {
        self.disconnect();
        if let Err(error) = self.kill_and_wait() {
            eprintln!("Could not reap fixture child: {error}");
        }
        if let Some(reader) = self.reader.take()
            && reader.join().is_err()
        {
            eprintln!("Fixture readiness reader panicked");
        }
    }
}
