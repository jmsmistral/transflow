# Catalogue editor generations

`tf_catalog::editor` renders a verified `CatalogSnapshotV1` into typing-only files,
then installs `.transflow/runtime/generated/<catalog-fingerprint>/type-stubs/`.
The root SDK exports and public `transflow.catalog` exports are explicitly
forwarded to the installed SDK. Namespace properties include registered aliases
and children of paths that are also datasets. Unknown names remain type errors.

`Overlay::render` has no filesystem or Python execution. `EditorCache::refresh`
runs on a blocking caller while that caller owns the workspace mutation lock.
It writes exclusive private staging files, syncs the files/directories, installs
the complete generation, then atomically replaces a relative `current` symlink.
`EditorCache::check` verifies that pointer against an expected snapshot and checks
all expected bytes, names and the manifest. Changed fingerprints, extra runtime
files, symlinks inside a generation and replaced parent directories fail visibly.
Concurrent callers must share workspace ownership; the old-pointer check also
rejects an observed competing refresh. Files are mode 0600, new directories 0700.

The stable editor path is `.transflow/runtime/generated/current`. It points only
to a checked fingerprint generation in this workspace. An interrupted generation
is never activated; private `.overlay-*` staging can remain after interruption.
After an activation's rename, an I/O failure during directory sync means durability
is unconfirmed; inspect with `check` before retrying. Cache cleanup is an explicit
future runtime maintenance operation. A changed generator format must invalidate
or explicitly rebuild its cache; existing modified bytes are never overwritten.

The catalogue fingerprint excludes the capture's source-snapshot ID, so captures
with identical catalogue semantics reuse one generation. The manifest retains
the workspace ID, fingerprint, generator format and file digests. This cache is
not runtime authority: discovery binds its supplied immutable catalogue and checks
its fingerprint independently. Never add the cache to `PYTHONPATH`, mutate
site-packages or put generated `.py` files alongside these stubs.

## Checker and editor setup

The installed-wheel tests qualify **mypy 2.3.1** with this workspace-local setting
(use the interpreter containing the installed `transflow` wheel):

```ini
[mypy]
strict = True
mypy_path = $MYPY_CONFIG_FILE_DIR/.transflow/runtime/generated/current
python_executable = /absolute/path/to/workspace/environment/bin/python
```

For Pyright-backed VS Code and Emacs integrations, the candidate project recipe
is a `pyrightconfig.json` in each workspace:

```json
{
  "stubPath": ".transflow/runtime/generated/current",
  "typeCheckingMode": "strict"
}
```

Select the workspace's matched Python interpreter in the editor/server. Restart
its language server after catalogue refresh if it retains old completion data.
Do not configure a shared environment-wide stub path. Pyright's documented
[stub path](https://github.com/microsoft/pyright/blob/main/docs/type-stubs.md) and
Python's [typing distribution rules](https://typing.python.org/en/latest/spec/distributing.html)
explain the intended resolution. **This Pyright/editor recipe is not yet qualified
in native VS Code or Emacs.** Those observations remain on the
[manual checklist](../../docs/development/manual-acceptance.md). No installation
or native-editor pass is claimed by the mypy tests.

Production generation is a Rust service for the upcoming preparation/catalogue
command; this task does not expose a public `catalog sync` command. The developer
fixture renderer `cargo run -p tf-catalog --example render-editor --locked --offline`
reads a snapshot on stdin and prints the exact typing files as JSON. Shared fixtures
are checked byte-for-byte against this renderer and exercised by real isolated
mypy processes using one installed SDK. The tests cover two simultaneous workspace
catalogues, a refresh in only one, missing names, preserved SDK diagnostics and
unchanged installed bytes. Filesystem tests separately exercise the real Rust
writer, stale pointers, tampering, permissions and failure at all three boundaries.
