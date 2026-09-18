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
        assert!(help.contains("Only help and version are available"));
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
        vec!["build", "synthetic/orders"],
        vec!["serve"],
        vec!["--unknown"],
    ] {
        let output = invoke(&args)?;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8(output.stderr)?.contains("error:"));
    }
    Ok(())
}
