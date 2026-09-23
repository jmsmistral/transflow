//! Real executable smoke tests, independent of a specification/data workspace.

use std::{
    error::Error,
    ffi::OsStr,
    process::{Command, Output},
};

fn invoke(args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_transflow"))
        .args(args)
        .current_dir(std::env::temp_dir())
        // The executable does not need Python, Git or another subprocess.
        .env("PATH", OsStr::new(""))
        .output()
}

#[test]
fn help_works_outside_a_checkout_without_other_tools() -> Result<(), Box<dyn Error>> {
    for arguments in [vec![], vec!["--help"], vec!["-h"]] {
        let output = invoke(&arguments)?;
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let help = String::from_utf8(output.stdout)?;
        assert!(help.contains("validate and catalog sync commands are available"));
        assert!(help.contains("not implemented yet"));
    }
    Ok(())
}

#[test]
fn version_is_the_actual_development_package_version() -> Result<(), Box<dyn Error>> {
    let output = invoke(&["--version"])?;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!("transflow {}\n", env!("CARGO_PKG_VERSION"))
    );
    Ok(())
}

#[test]
fn unavailable_commands_and_invalid_flags_fail() -> Result<(), Box<dyn Error>> {
    for args in [
        vec!["schedule", "list"],
        vec!["build", "x", "--unknown"],
        vec!["--unknown"],
    ] {
        let output = invoke(&args)?;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)?.starts_with("The command could not be understood")
        );
    }
    Ok(())
}

#[test]
fn inspection_help_and_invalid_options_need_no_workspace() -> Result<(), Box<dyn Error>> {
    for name in ["plan", "why", "upstream", "downstream"] {
        let output = invoke(&[name, "--help"])?;
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout)?;
        assert!(help.contains("--git-ref"));
        assert!(help.contains("--branch"));
    }
    for args in [
        vec!["plan", "x", "--wait"],
        vec!["why", "x", "--no-wait"],
        vec!["why", "x", "--target", "y"],
        vec!["plan", "x", "--fallback", "master", "--no-fallback"],
        vec!["plan", "x", "--git-ref", "HEAD"],
        vec!["plan", "x", "--pin", "x=bad"],
        vec!["plan", "x", "--param", "count=invalid"],
        vec!["upstream", "x", "--depth", "-1"],
        vec!["downstream", "x", "--depth", "1.5"],
        vec!["upstream", "x", "--force"],
    ] {
        let mut args = args;
        args.push("--json");
        let output = invoke(&args)?;
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stderr.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        tf_protocol::validate_document("CliEnvelopeV1", &value)?;
        assert_eq!(value["exit_status"], 2);
    }
    Ok(())
}
