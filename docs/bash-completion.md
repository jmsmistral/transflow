# Bash completion

Transflow can complete commands, nested subcommands, option names and fixed option
values when you press **Tab**. File and directory arguments use local filename
completion. It works with macOS's built-in Bash 3.2 and Bash 4/5; no separate
`bash-completion` package is required.

The script is generated from the same argument definitions as `transflow --help`,
using [Clap's completion generator](https://docs.rs/clap_complete/4.6.11/clap_complete/).
Generating or using it does not open the catalogue, import transforms, start a
coordinator or run a build. Dataset names, branch names and retained build/schedule
IDs are not queried or suggested.

## Enable in your current terminal

First make sure you are running Bash and the current Transflow CLI is on PATH:

```bash
echo "$BASH_VERSION"
command -v transflow
source <(transflow completions bash)
```

For a source checkout, build the CLI and add its directory to PATH first:

```bash
(cd "$HOME/dev/transflow" && cargo build --locked --offline -p transflow)
export PATH="$HOME/dev/transflow/target/debug:$PATH"
source <(transflow completions bash)
```

Use the dependency setup in [Getting started](getting-started.md) if the offline
build reports missing dependencies. No workspace is needed to enable completion.

Try typing these prefixes, pressing Tab where indicated (do not type `<Tab>`):

| Type | Completion |
|---|---|
| `transflow bui<Tab>` | `build` |
| `transflow catalog sy<Tab>` | `sync` |
| `transflow build raw/orders --mo<Tab>` | `--mode` |
| `transflow build raw/orders --mode sel<Tab>` | `selected` |
| `transflow --color ne<Tab>` | `never` |
| `transflow schedule create --file ./<Tab>` | Local files |

Press Tab twice to list multiple candidates. Prefer a space between an option
and its value when completing, for example `--mode sel`.

## Keep it enabled in new terminals

Save the script in your own configuration directory:

```bash
mkdir -p "$HOME/.config/transflow"
transflow completions bash > "$HOME/.config/transflow/completion.bash"
```

Add this block once to `~/.bashrc`, after any PATH setup:

```bash
if [[ $- == *i* && -r "$HOME/.config/transflow/completion.bash" ]]; then
    source "$HOME/.config/transflow/completion.bash"
fi
```

macOS Terminal commonly starts a login shell, which reads `~/.bash_profile`
instead of `~/.bashrc`. If your login startup file does not already load
`~/.bashrc`, add this block to it:

```bash
if [[ -r "$HOME/.bashrc" ]]; then
    source "$HOME/.bashrc"
fi
```

Keep your existing startup configuration. If you already use `~/.bash_login` or
`~/.profile` instead of `~/.bash_profile`, add the block to that existing file.
Bash checks `~/.bash_profile`, `~/.bash_login`, then `~/.profile`, reading only
the first one it finds. See the
[Bash startup-file reference](https://www.gnu.org/software/bash/manual/html_node/Bash-Startup-Files.html).

Open a new Bash terminal, or load the saved script now:

```bash
source "$HOME/.config/transflow/completion.bash"
complete -p transflow
```

Regenerate and source the saved file after upgrading or rebuilding Transflow so
it includes the new commands and options. The generated file contains shell code;
do not combine generation with `--json`.

## Use the sample without `--workspace`

Completion works with normal nearest-workspace discovery:

```bash
export PATH="$HOME/dev/transflow/target/debug:$PATH"
source <(transflow completions bash)
cd "$HOME/dev/transflow-workspace/preview-scroll-review"
transflow catalog list
```

Continue with [Sample workspace CLI checks](development/sample-workspace-cli.md).
The current directory selects the workspace. Completion does not change it or
require you to add `--workspace`. When needed, the root `--workspace DIRECTORY`
still goes before the command name.

## Troubleshooting and removal

- **Unknown `completions` command:** rebuild or upgrade the CLI, then check
  `command -v transflow` for an older executable earlier on PATH.
- **Nothing happens on Tab:** check `echo "$BASH_VERSION"` and
  `complete -p transflow`. In Bash, `set -o emacs` or `set -o vi` enables readline;
  `bind '"\C-i": complete'` restores Tab completion if you changed that binding.
- **Your shell is Zsh:** start `bash` before sourcing this script. This guide and
  command currently support Bash only.
- **A shell alias has another name:** after sourcing, register that name with
  `complete -F _transflow -o filenames -o bashdefault -o default tf` for an alias named `tf`.
  This assumes the alias invokes `transflow` without extra arguments.
- **Disable:** run `complete -r transflow` in the current shell and remove the
  Transflow source block from your startup file to disable future sessions.
