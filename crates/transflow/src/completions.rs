//! Static completion output; generating or sourcing it never opens a workspace.

use clap::{Arg, Command};
use clap_complete::aot::{Bash, Generator};
use std::io::{self, Write};

pub(crate) fn command() -> Command {
    Command::new("completions")
        .disable_help_flag(true)
        .about("Print a Bash completion script without opening a workspace")
        .after_help("Enable in Bash: source <(transflow completions bash). Regenerate after upgrading Transflow. Script output cannot be combined with --json.")
        .arg(
            Arg::new("shell")
                .value_parser(["bash"])
                .required_unless_present("help")
                .conflicts_with("json"),
        )
}

pub(crate) fn write_bash(output: &mut dyn Write) -> io::Result<()> {
    let mut command = crate::command();
    command.set_bin_name("transflow");
    command.build();
    // Use the fallible API so a closed pipe remains a normal CLI output error.
    Bash.try_generate(&command, output)?;
    // Bash 3.2 has no compopt: readline needs this registration flag to quote
    // generated filename candidates (otherwise a space becomes two arguments).
    writeln!(
        output,
        "\n# Bash 3.2 cannot set filename quoting dynamically with compopt.\nif [[ ${{BASH_VERSINFO[0]}} -lt 4 ]]; then\n    complete -F _transflow -o filenames -o bashdefault -o default transflow\nfi"
    )
}
