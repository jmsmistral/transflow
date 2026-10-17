# Dataset references in your editor

Transflow's local language service adds dataset completion, hover, producer
navigation and typo diagnostics to Python files. It complements ordinary Python
tooling. It parses source with the Python 3.14 standard library and reads catalogue
metadata; it does not import producers, build datasets or install packages.

```bash
transflow --workspace /absolute/workspace lsp
# Optional explicit parser and data branch:
transflow --workspace /absolute/workspace lsp \
  --python /absolute/python3.14 --branch develop
```

The transport is standard LSP 3.17 with `Content-Length` frames over stdio.
Do not add `--json`: stdout is reserved for JSON-RPC; process diagnostics use
stderr. Clients supply local workspace folders and full document synchronization.
The configured workspace interpreter is used unless `--python` overrides it.

## What the service understands

```python
from transflow import Input
from transflow.catalog import C

orders = Input(C.raw.orders)
other = Input("raw/orders")
```

- Complete catalogue namespaces after `C.` and paths inside the first string
  argument of `Input(...)`. Explicit import aliases are supported.
- Hover shows stable identity, catalogue fingerprint, selected data branch and
  available retained version/schema metadata. Foreign providers are not contacted.
- Go to definition uses the last validated producer descriptor captured by an
  explicit `catalog sync` or validated operation. Navigation stays within the
  workspace's configured local source roots; symlink escapes are refused.
- Unknown references and syntax errors are shown without executing the draft.
  A broken catalogue keeps the server's last valid metadata, clearly labelled.
  Without captured descriptors, completion still works and navigation is unknown.

Captured producer locations can differ from today's working tree. Hover labels
that distinction and never calls the working draft built or verified. Catalogue
refresh does not automatically run `catalog sync`; use that command deliberately
after changing declarations. Ambiguously rebound import names are conservatively
left to Python tooling. Dynamic aliases, general Python type checking and column
lineage are outside this service's scope.

Limits are explicit: 16 workspace roots, 128 open documents, 512 KiB UTF-8 per
document, 4,096 dataset references, a three-second syntax subprocess deadline,
1 MiB per protocol frame and 200 completions per response (`isIncomplete` asks
clients to refine the prefix). Changes are debounced for 150 ms. Large metadata
responses fail explicitly instead of being silently truncated.

## VS Code

The thin local extension is in [editors/vscode](../editors/vscode/). It has no npm
runtime dependencies or telemetry. It uses additive completion/hover/definition
providers and its own diagnostic collection, so it does not replace Pylance or
another Python language server. Only trusted local workspaces can start the CLI.

Launch a development instance with an isolated profile:

```bash
code --user-data-dir /tmp/transflow-vscode-review \
  --extensions-dir /tmp/transflow-vscode-extensions \
  --extensionDevelopmentPath=/absolute/transflow/editors/vscode \
  /absolute/workspace
```

Set **Transflow: Executable** to the matched native CLI. Optionally set
**Transflow: Python** and **Transflow: Branch** per workspace. Open the actual
workspace root containing `workspace.toml`, then a Python file under its source
roots. Commands are **Transflow: Restart Dataset Service** and
**Transflow: Open Dataset Producer**. Each workspace has its own process and
metadata; catalogue/configuration changes refresh the service.

The bundled Node transport tests use the real service but do not, by themselves,
qualify the native VS Code UI. See the [native checklist](development/manual-acceptance.md).
The extension is a local development package, not a Marketplace publication.

## Emacs

The additive minor mode requires Emacs 29.1 or later and only built-in libraries:

```elisp
(add-to-list 'load-path "/absolute/transflow/editors/emacs")
(require 'transflow)
(setq transflow-executable "/absolute/transflow/target/debug/transflow")
;; Optional when workspace configuration has no interpreter:
(setq transflow-python "/absolute/python3.14")
```

Open a Python source file and run `M-x transflow-mode`. Use completion at point,
Eldoc hover, Flymake diagnostics (enable `flymake-mode` if needed) and
`M-x transflow-open-producer`. `M-x transflow-stop` stops all service processes.
The mode keeps existing Python CAPF, Eldoc and Flymake providers. It does not
claim that Eglot and lsp-mode can both own the same Python buffer simultaneously.
When using lsp-mode or Eglot for Python, keep that client and add this minor mode
for Transflow; do not replace the Python server with Transflow's narrow service.

Emacs 30.1 batch qualification exercises the actual client connection, independent
workspace completion, typo diagnostics, refresh, UTF-16 and producer navigation.
This is separate from interactive editor/Python-server qualification.

For Emacs 30.1 with Python 3.14, start an ordinary Python shell and run
`M-x transflow-python-shell-compat` once in that buffer if completion JSON is
polluted by the editor's deprecated `codecs.open` helper. The explicit compatibility
command updates only the running process's built-in file-evaluation helper to
`open`, retaining encoding/newline handling. It changes no installed editor files
and installs no warning filter. The native check covers installed-SDK root and
submodule completion alongside the Transflow providers. This is unnecessary for
a separate Python LSP. See [Python's deprecation note](https://docs.python.org/3.14/library/codecs.html#codecs.open).

## Stubs and a command fallback

Explicit catalogue synchronization generates `.pyi` files under
`.transflow/runtime/generated/current`. Point your Python tool's **stub search**
configuration at that directory, as described in the
[catalogue overlay guide](development/catalog-overlay.md). Do not add it to
`PYTHONPATH`; runtime imports must keep using the installed SDK and worker.

Without any editor integration, use:

```bash
transflow --workspace /absolute/workspace catalog show raw/orders
transflow --workspace /absolute/workspace --json catalog show raw/orders
```

The `producer` field contains the retained relative path, line and source snapshot
when known. Missing/stale producer metadata remains explicit. Neither command
imports producers to navigate them.

Protocol reference: [Language Server Protocol 3.17](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/).
