# Transflow datasets

Local dataset completion, hover, diagnostics and producer navigation for VS Code
and compatible desktop editors. The extension starts the native `transflow lsp`
service and adds its features alongside ordinary Python tooling.

## Local preview setup

1. Install the local Transflow VSIX through **Extensions: Install from VSIX**.
2. Open the workspace folder containing `workspace.toml` and trust that folder.
3. Set **Transflow: Executable** to the matching native CLI. Set **Transflow:
   Python** to Python 3.14 if the workspace does not already configure it.
4. Open a Python file under a configured source root. Complete `C.` catalogue
   references or the dataset path in `Input("...")`, hover a reference, or use
   **Go to Definition** to open its retained producer location.

For example:

```python
from transflow import Input
from transflow.catalog import C

orders = Input(C.raw.orders)
```

Run `transflow catalog sync` explicitly after changing dataset declarations to
refresh catalogue names and retained producer locations. The extension does not
run transforms, initiate builds or upload source/catalogue data.

Use **Transflow: Restart Dataset Service** after changing service configuration.
**Transflow: Open Dataset Producer** navigates from the current reference.
Missing or stale catalogue/producer metadata is labelled explicitly.

The preview requires a separately installed native Transflow CLI and Python
3.14. Its VSIX does not contain either runtime. It is a local development package,
not a Marketplace or Open VSX publication. Installed Python tooling is retained;
Transflow does not supply general Python type checking or debugging.
