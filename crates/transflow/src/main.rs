//! Transflow executable composition entry point.

use std::{io, io::Write, process::ExitCode};

fn main() -> ExitCode {
    // Progress is emitted by an owned worker thread while the command runs.
    // Lock each write, not the whole invocation, so shutdown can join that thread.
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    match transflow::run(std::env::args_os(), &mut stdout, &mut stderr) {
        Ok(code) => code,
        Err(error) => {
            if writeln!(stderr, "{error}").is_err() {
                return ExitCode::FAILURE;
            }
            ExitCode::FAILURE
        }
    }
}
