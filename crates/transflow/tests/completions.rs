//! Exercise generated completion in a real Bash process, with no workspace services.

use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "transflow-completion-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn invoke(root: &Path, args: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_transflow"))
        .args(args)
        .current_dir(root)
        .env("PATH", "")
        .output()
}

fn script(root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let output = invoke(root, &["completions", "bash"])?;
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty());
    let path = root.join("transflow.bash");
    fs::write(&path, output.stdout)?;
    Ok(path)
}

fn bash() -> std::ffi::OsString {
    std::env::var_os("TRANSFLOW_TEST_BASH").unwrap_or_else(|| "/bin/bash".into())
}

fn candidates(root: &Path, script: &Path, words: &[&str]) -> Result<Vec<String>, Box<dyn Error>> {
    let output = Command::new(bash())
        .args([
            "--noprofile",
            "--norc",
            "-c",
            r#"
source "$1" || exit
# On Bash 3.2, filenames must be registered up front for readline to quote spaces.
if [[ ${BASH_VERSINFO[0]} -lt 4 ]]; then
    [[ $(complete -p transflow) == *'-o filenames'* ]] || exit 3
fi
shift
# compopt is otherwise restricted to an interactive readline completion.
compopt() { :; }
COMP_WORDS=("$@")
COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
_transflow "${COMP_WORDS[0]}" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD-1]}"
printf '%s\n' "${COMPREPLY[@]}"
"#,
            "completion-test",
        ])
        .arg(script)
        .args(words)
        .current_dir(root)
        .env_remove("BASH_ENV")
        .env("LC_ALL", "C")
        .env("PATH", "")
        .output()?;
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty(), "{:?}", output);
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

#[test]
fn generation_is_deterministic_and_needs_no_workspace_or_other_tools() -> Result<(), Box<dyn Error>>
{
    let temp = Temp::new()?;
    let first = invoke(&temp.0, &["completions", "bash"])?;
    assert!(first.status.success());
    assert!(first.stderr.is_empty());
    assert_eq!(fs::read_dir(&temp.0)?.count(), 0);
    // Even invalid authoring configuration must not be opened by this command.
    fs::write(temp.0.join("workspace.toml"), "not valid TOML\n")?;
    let second = invoke(&temp.0, &["--workspace", "/missing", "completions", "bash"])?;
    assert!(second.status.success());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(fs::read_dir(&temp.0)?.count(), 1);
    let path = script(&temp.0)?;
    let check = Command::new(bash()).arg("-n").arg(path).output()?;
    assert!(check.status.success(), "{:?}", check);
    Ok(())
}

#[test]
fn bash_completes_nested_commands_flags_and_known_values() -> Result<(), Box<dyn Error>> {
    let temp = Temp::new()?;
    let script = script(&temp.0)?;
    for (words, expected) in [
        (vec!["transflow", "bui"], "build"),
        (vec!["transflow", "catalog", "sy"], "sync"),
        (vec!["transflow", "build", "lo"], "logs"),
        (vec!["transflow", "schedule", "cr"], "create"),
        (vec!["transflow", "completions", "ba"], "bash"),
        (vec!["transflow", "build", "raw/orders", "--mo"], "--mode"),
        (
            vec!["transflow", "build", "raw/orders", "--mode", "sel"],
            "selected",
        ),
        (vec!["transflow", "--color", "ne"], "never"),
        (vec!["transflow", "schedule", "list", "--j"], "--json"),
        (
            vec!["transflow", "--workspace", "/tmp/example", "catalog", "sy"],
            "sync",
        ),
        (
            vec!["transflow", "external", "add", "--work"],
            "--workspace",
        ),
    ] {
        assert_eq!(
            candidates(&temp.0, &script, &words)?,
            [expected],
            "{words:?}"
        );
    }
    assert!(candidates(&temp.0, &script, &["transflow", "build", "--work"])?.is_empty());
    Ok(())
}

#[test]
fn file_values_preserve_spaces_and_do_not_execute_shell_text() -> Result<(), Box<dyn Error>> {
    let temp = Temp::new()?;
    let script = script(&temp.0)?;
    let filename = "schedule $(touch PWNED).json";
    fs::write(temp.0.join(filename), "{}")?;
    assert_eq!(
        candidates(
            &temp.0,
            &script,
            &["transflow", "schedule", "create", "--file", "schedule "]
        )?,
        [filename]
    );
    assert!(!temp.0.join("PWNED").exists());
    Ok(())
}

#[test]
fn unsupported_shell_missing_shell_and_json_script_mix_are_usage_errors()
-> Result<(), Box<dyn Error>> {
    let temp = Temp::new()?;
    for args in [
        vec!["completions"],
        vec!["completions", "zsh"],
        vec!["completions", "bash", "--json"],
    ] {
        let output = invoke(&temp.0, &args)?;
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        if args.contains(&"--json") {
            assert!(output.stderr.is_empty());
            let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            tf_protocol::validate_document("CliEnvelopeV1", &value)?;
            assert_eq!(value["exit_status"], 2);
        } else {
            assert!(output.stdout.is_empty());
        }
    }
    let help = invoke(&temp.0, &["completions", "--help"])?;
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)?.contains("source <(transflow completions bash)"));
    Ok(())
}

#[test]
fn output_failure_is_returned_without_panicking() {
    struct Fails;
    impl io::Write for Fails {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let result = transflow::run(
        ["transflow", "completions", "bash"].map(Into::into),
        &mut Fails,
        &mut Vec::new(),
    );
    assert!(matches!(result, Err(transflow::CliError::Stdout(_))));
}
