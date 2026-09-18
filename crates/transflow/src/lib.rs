//! Command-line entry point. Only bootstrap help/version are implemented.
//!
//! Application services will be composed here as their contracts are delivered.
//! Merely inspecting help or version does not inspect a workspace or start work.

use std::{ffi::OsString, io, process::ExitCode};

/// A failed output operation, preserving the original I/O error for callers.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Writing normal command output failed.
    #[error("Could not write command output: {0}")]
    Stdout(#[source] io::Error),
    /// Writing a command-line diagnostic failed.
    #[error("Could not write command diagnostic: {0}")]
    Stderr(#[source] io::Error),
}

fn command() -> clap::Command {
    clap::Command::new("transflow")
        .version(env!("CARGO_PKG_VERSION"))
        .about("A local build system for dataframe datasets (development scaffold)")
        .after_help("Only help and version are available. Dataset builds and the coordinator are not implemented yet.")
        .disable_help_subcommand(true)
}

/// Parse command-line arguments and write to the supplied output streams.
///
/// Help, version and invocation without arguments return success. Invalid flags
/// or unavailable commands produce a diagnostic and exit code 2. I/O failures
/// return a typed error; no workspace, environment or service is initialized.
pub fn run(
    args: impl IntoIterator<Item = OsString>,
    stdout: &mut dyn io::Write,
    stderr: &mut dyn io::Write,
) -> Result<ExitCode, CliError> {
    match command().try_get_matches_from(args) {
        Ok(_) => {
            writeln!(stdout, "{}", command().render_help()).map_err(CliError::Stdout)?;
            Ok(ExitCode::SUCCESS)
        }
        Err(error) if error.use_stderr() => {
            write!(stderr, "{error}").map_err(CliError::Stderr)?;
            Ok(ExitCode::from(2))
        }
        Err(error) => {
            write!(stdout, "{error}").map_err(CliError::Stdout)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    struct ClosedPipe;

    impl io::Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "synthetic closed pipe",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stdout_failure_preserves_typed_source() -> Result<(), Box<dyn Error>> {
        let result = run(
            ["transflow", "--help"].map(OsString::from),
            &mut ClosedPipe,
            &mut Vec::new(),
        );
        let error = match result {
            Err(error @ CliError::Stdout(_)) => error,
            other => return Err(format!("Expected a stdout failure, got {other:?}").into()),
        };
        let source = error
            .source()
            .and_then(|source| source.downcast_ref::<io::Error>());
        assert_eq!(source.map(io::Error::kind), Some(io::ErrorKind::BrokenPipe));
        assert!(
            error
                .to_string()
                .starts_with("Could not write command output:")
        );
        Ok(())
    }

    #[test]
    fn diagnostic_failure_is_not_success() {
        let result = run(
            ["transflow", "build"].map(OsString::from),
            &mut Vec::new(),
            &mut ClosedPipe,
        );
        assert!(matches!(result, Err(CliError::Stderr(_))));
    }
}
