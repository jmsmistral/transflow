//! CLI argument handling, human diagnostics and versioned JSON results.
//! Help/version require no workspace I/O. Application services remain later work.
mod env;
mod init;
use std::{
    ffi::OsString,
    io::{self, IsTerminal},
    process::ExitCode,
};
use tf_domain::diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticError, ExitStatus, Redactor, RequestContext,
};
use tf_protocol::diagnostic::{CliEnvelope, InformationKind};

/// Failed output or diagnostic construction, preserving a typed internal cause.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Normal output failed.
    #[error("Could not write command output")]
    Stdout(#[source] io::Error),
    /// Diagnostic output failed.
    #[error("Could not write command diagnostic")]
    Stderr(#[source] io::Error),
    /// Safe diagnostic construction failed.
    #[error("Could not construct a safe command diagnostic")]
    Diagnostic(#[from] DiagnosticError),
    /// Result encoding failed.
    #[error("Could not encode the command result")]
    Protocol(#[from] tf_protocol::ProtocolError),
}
fn command() -> clap::Command {
    use clap::{Arg, ArgAction};
    clap::Command::new("transflow")
        .about("A local build system for dataframe datasets (development scaffold)")
        .after_help("Help, version, workspace initialization and explicit environment commands are available. Dataset builds and the coordinator are not implemented yet.")
        .disable_help_subcommand(true).disable_help_flag(true).disable_version_flag(true)
        .subcommand(clap::Command::new("env").disable_help_flag(true).about("Explicit environment lock, sync and drift verification")
            .arg(Arg::new("python").long("python").global(true).value_name("EXECUTABLE"))
            .arg(Arg::new("runtime-wheel").long("runtime-wheel").global(true).value_name("WHEEL"))
            .arg(Arg::new("wheelhouse").long("wheelhouse").global(true).value_name("DIRECTORY"))
            .arg(Arg::new("no-index").long("no-index").global(true).action(ArgAction::SetTrue))
            .subcommand(clap::Command::new("lock").disable_help_flag(true).about("Resolve exact hashed dependencies using qualified tooling"))
            .subcommand(clap::Command::new("sync").disable_help_flag(true).about("Install locked wheels and the explicit matched Transflow wheel"))
            .subcommand(clap::Command::new("check").disable_help_flag(true).about("Reject interpreter, lock or installed-file drift without installation")))
        .subcommand(clap::Command::new("init").disable_help_flag(true).about("Initialize a workspace without Git, Python or package installation").arg(Arg::new("directory").value_name("DIRECTORY")))
        .arg(Arg::new("help").global(true).long("help").short('h').action(ArgAction::SetTrue).help("Show implemented commands and options"))
        .arg(Arg::new("version").long("version").short('V').action(ArgAction::SetTrue).help("Show product version"))
        .arg(Arg::new("json").long("json").global(true).action(ArgAction::SetTrue).help("Emit one versioned result envelope on stdout"))
        .arg(Arg::new("workspace").long("workspace").value_name("DIRECTORY").help("Select root workspace context before a command (no discovery for help/version)"))
        .arg(Arg::new("verbose").long("verbose").global(true).action(ArgAction::SetTrue).help("Include stable codes in human diagnostics"))
        .arg(Arg::new("color").long("color").global(true).value_parser(["auto","always","never"]).default_value("auto").help("Human diagnostic colour; JSON always disables it"))
}
/// Render sanitized domain diagnostics. Colour wraps only the heading with fixed escapes.
pub fn render_diagnostic(
    d: &Diagnostic,
    ctx: &RequestContext,
    verbose: bool,
    color: bool,
) -> String {
    fn render(d: &Diagnostic, verbose: bool, color: bool, depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth);
        let heading = if color {
            format!("\x1b[1;31m{}\x1b[0m", d.heading().as_str())
        } else {
            d.heading().as_str().to_owned()
        };
        out.push_str(&format!("{pad}{heading}\n\n{pad}{}\n", d.reason().as_str()));
        for reference in d.affected() {
            out.push_str(&format!("{pad}  {}\n", reference.as_str()));
        }
        for source in d.sources() {
            out.push_str(&format!(
                "{pad}  {}:{}:{}–{}:{}\n",
                source.path().as_str(),
                source.start().0,
                source.start().1,
                source.end().0,
                source.end().1
            ));
        }
        out.push_str(&format!("\n{pad}{}\n", d.remediation().as_str()));
        if verbose {
            out.push_str(&format!("{pad}Code: {}\n", d.code().as_str()));
        }
        for cause in d.causes() {
            out.push_str(&format!("\n{pad}Caused by:\n"));
            render(cause, verbose, false, depth + 1, out);
        }
    }
    let mut out = String::new();
    render(d, verbose, color, 0, &mut out);
    if let Some(path) = &ctx.workspace {
        out.push_str(&format!("Workspace: {}\n", path.as_str()));
    }
    if let Some(source) = &ctx.source {
        out.push_str(&format!("Source: {}\n", source.as_str()));
    }
    if let Some(id) = ctx.request_id {
        out.push_str(&format!("Request: {id}\n"));
    }
    out
}
// On parse failure Clap cannot provide matches. Recognize output intent without
// echoing argv, honoring option values and `--`. Root workspace is never inherited
// into a later command's provider --workspace option.
#[derive(Default)]
struct Intent {
    json: bool,
    verbose: bool,
    color: Option<String>,
    workspace: Option<String>,
}
fn intent(args: &[OsString]) -> Intent {
    let mut intent = Intent::default();
    let mut iter = args.iter().skip(1);
    let mut root = true;
    while let Some(arg) = iter.next() {
        let Some(text) = arg.to_str() else { continue };
        if text == "--" {
            break;
        }
        match text {
            "--json" => intent.json = true,
            "--verbose" => intent.verbose = true,
            "--workspace" | "--color" => {
                if let Some(value) = iter.next().and_then(|v| v.to_str()) {
                    if text == "--color" {
                        intent.color = Some(value.to_owned());
                    } else if root {
                        intent.workspace = Some(value.to_owned());
                    }
                }
            }
            _ => {
                if let Some(value) = text.strip_prefix("--workspace=") {
                    if root {
                        intent.workspace = Some(value.to_owned());
                    }
                } else if let Some(value) = text.strip_prefix("--color=") {
                    intent.color = Some(value.to_owned());
                } else if !text.starts_with('-') {
                    root = false;
                }
            }
        }
    }
    intent
}
/// Parse CLI arguments and emit either human output or exactly one JSON envelope.
/// No user argv is copied into a parser error, and no workspace is opened here.
pub fn run(
    args: impl IntoIterator<Item = OsString>,
    stdout: &mut dyn io::Write,
    stderr: &mut dyn io::Write,
) -> Result<ExitCode, CliError> {
    let args: Vec<_> = args.into_iter().collect();
    let intent = intent(&args);
    let redactor = Redactor::default();
    let context = RequestContext {
        workspace: intent
            .workspace
            .as_ref()
            .map(|s| match redactor.text(s) {
                Ok(safe) => Ok(safe),
                Err(_) => {
                    redactor.text("[workspace context is empty or exceeds the display limit]")
                }
            })
            .transpose()?,
        ..RequestContext::default()
    };
    let version = redactor.text(env!("CARGO_PKG_VERSION"))?;
    match command().try_get_matches_from(args) {
        Ok(matches) => {
            if let Some(("env", environment)) = matches.subcommand()
                && !matches.get_flag("help")
                && !matches.get_flag("version")
            {
                return match env::execute(environment, matches.get_one::<String>("workspace")) {
                    Ok(text) => {
                        if intent.json {
                            stdout
                                .write_all(
                                    CliEnvelope::success(
                                        &version,
                                        InformationKind::Environment,
                                        &redactor.text(&text)?,
                                        &context,
                                    )?
                                    .as_bytes(),
                                )
                                .map_err(CliError::Stdout)?;
                        } else {
                            writeln!(stdout, "{text}").map_err(CliError::Stdout)?;
                        }
                        Ok(ExitCode::SUCCESS)
                    }
                    Err(error) => {
                        let status = if matches!(error, env::EnvError::Usage) {
                            ExitStatus::Usage
                        } else {
                            ExitStatus::Failure
                        };
                        let d=Diagnostic::new(DiagnosticCode::OperationFailed,redactor.text("Environment preparation could not finish")?,redactor.text(&error.to_string())?,redactor.text("Prepare the qualified Python tooling, then run the explicit env lock, env sync or env check command. Builds never install dependencies automatically.")?);
                        if intent.json {
                            stdout
                                .write_all(
                                    CliEnvelope::failure(&version, status, &context, &[d])?
                                        .as_bytes(),
                                )
                                .map_err(CliError::Stdout)?;
                        } else {
                            stderr
                                .write_all(
                                    render_diagnostic(&d, &context, intent.verbose, false)
                                        .as_bytes(),
                                )
                                .map_err(CliError::Stderr)?;
                        }
                        Ok(ExitCode::from(status.code()))
                    }
                };
            }
            if let Some(("init", init)) = matches.subcommand()
                && !matches.get_flag("help")
                && !matches.get_flag("version")
            {
                let destination = init
                    .get_one::<String>("directory")
                    .map(std::path::PathBuf::from)
                    .or_else(|| {
                        matches
                            .get_one::<String>("workspace")
                            .map(std::path::PathBuf::from)
                    })
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                return match init::initialize(&destination) {
                    Ok(text) => {
                        if intent.json {
                            stdout
                                .write_all(
                                    CliEnvelope::success(
                                        &version,
                                        InformationKind::WorkspaceInit,
                                        &redactor.text(&text)?,
                                        &context,
                                    )?
                                    .as_bytes(),
                                )
                                .map_err(CliError::Stdout)?;
                        } else {
                            writeln!(stdout, "{text}").map_err(CliError::Stdout)?;
                        }
                        Ok(ExitCode::SUCCESS)
                    }
                    Err(error) => {
                        let d=Diagnostic::new(DiagnosticCode::OperationFailed,redactor.text("Workspace initialization could not finish")?,redactor.text(&error.to_string())?,redactor.text("Correct the reported files without deleting the durable registry, then run transflow init again.")?);
                        if intent.json {
                            stdout
                                .write_all(
                                    CliEnvelope::failure(
                                        &version,
                                        ExitStatus::Failure,
                                        &context,
                                        &[d],
                                    )?
                                    .as_bytes(),
                                )
                                .map_err(CliError::Stdout)?;
                        } else {
                            stderr
                                .write_all(
                                    render_diagnostic(&d, &context, intent.verbose, false)
                                        .as_bytes(),
                                )
                                .map_err(CliError::Stderr)?;
                        }
                        Ok(ExitCode::FAILURE)
                    }
                };
            }
            let (kind, text) = if matches.get_flag("version") {
                (
                    InformationKind::Version,
                    format!("transflow {}", env!("CARGO_PKG_VERSION")),
                )
            } else {
                (InformationKind::Help, command().render_help().to_string())
            };
            if matches.get_flag("json") {
                let envelope =
                    CliEnvelope::success(&version, kind, &redactor.text(&text)?, &context)?;
                stdout
                    .write_all(envelope.as_bytes())
                    .map_err(CliError::Stdout)?;
            } else {
                writeln!(stdout, "{text}").map_err(CliError::Stdout)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(_error) => {
            // Never dump Clap's error: it can echo secret values or terminal controls.
            let d=Diagnostic::new(DiagnosticCode::CliUsage,redactor.text("The command could not be understood")?,
                redactor.text("An argument is unknown, missing, or invalid. Run help to inspect the implemented commands.")?,
                redactor.text("Run transflow --help to see the available options. Dataset builds and the coordinator are not implemented yet.")?);
            if intent.json {
                let envelope = CliEnvelope::failure(&version, ExitStatus::Usage, &context, &[d])?;
                stdout
                    .write_all(envelope.as_bytes())
                    .map_err(CliError::Stdout)?;
            } else {
                let color = match intent.color.as_deref() {
                    Some("always") => true,
                    Some("never") => false,
                    _ => io::stderr().is_terminal(),
                };
                stderr
                    .write_all(render_diagnostic(&d, &context, intent.verbose, color).as_bytes())
                    .map_err(CliError::Stderr)?;
            }
            Ok(ExitCode::from(ExitStatus::Usage.code()))
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
                .starts_with("Could not write command output")
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
