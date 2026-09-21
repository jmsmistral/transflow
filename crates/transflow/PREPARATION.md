# Workspace validation and catalogue synchronization

After explicit `env lock` and `env sync` with the matched Transflow wheel:

```bash
transflow --workspace ./analysis validate --python /path/to/prepared/python
transflow --workspace ./analysis catalog sync --check --python /path/to/prepared/python
transflow --workspace ./analysis catalog sync --python /path/to/prepared/python
```

`--python` selects the base tooling interpreter used by environment setup; it is
not the managed worker interpreter. If omitted, the local interpreter setting or
configured `python3.x` executable is used. Environment inspection verifies the
locked dependencies, installed files, interpreter and matched application version.
These commands never resolve or install dependencies.

Validation captures the complete configured source, imports declarations in a
fresh managed worker and checks the entire local graph. Producer functions are
not invoked. Trusted Python imports can have arbitrary side effects; this is not
a Python sandbox. Validation and `sync --check` place their request, capture and
socket in private temporary storage outside the workspace and clean up on return.
They do not acquire a writer, create SQLite, update identities or refresh editor
files. `validate` succeeds with pending symbolic outputs. `sync --check` exits 1
when those additions require registration and preserves its report in JSON.

Sync acquires exclusive runtime ownership, recovers interrupted registry work,
and validates before allocating additive dataset IDs. The existing guarded,
journaled registry service commits those exact identities. A final capture binds
only that registry delta to the original discovery: all other captured inputs
must agree. Existing bound C references retain their IDs; discovery is not rerun.
The final graph is structurally validated again, editor stubs are refreshed, and
structural graph evidence is retained in `.transflow/runtime/graphs` with an
atomic `current` display pointer. Retained evidence is not execution authority.
A later invalid graph leaves the previous display evidence intact and fails.

Results distinguish registrations, unchanged producers, and absent producers.
Absence never deletes or tombstones a dataset. Reports include exact totals and
up to 64 entries per group; a larger group explicitly indicates omitted entries
in human output. JSON uses the closed `PreparationResultV1` contract, source,
catalogue and certificate identities, and deferred schema/data-check counts.
Structural success does not mean data checks have passed or data was published.

Discovery uses a private authenticated Unix socket, bounded frames/metadata,
separate discarded process output, a ten-second connection limit and a five-minute
whole-discovery limit. A partial message cannot restart the deadline. On normal
completion or failure the process group is stopped and the direct child reaped.
The broader execution supervisor, interactive cancellation and persistent
coordinator remain later tasks. Abrupt parent termination cannot promise cleanup
of arbitrary user-created detached processes.

An interruption after registry commit may leave valid new identities while editor
or graph activation is incomplete. The command reports failure; retrying sync
recovers/idempotently reuses the identities. No cross-filesystem/SQLite atomic
transaction is claimed. Non-cooperating source edits cause a conflict; imports
that modify live files are not rolled back. Captures and private staging retained
by interrupted mutating sync are subject to later runtime retention tooling.

`bash tools/check-cli.sh` tests a real native executable against freshly installed
local wheels and offline synthetic dependencies. Rust CI runs this bridge on
macOS arm64, Linux x86-64 and Linux arm64 with Python 3.14. Python's independent
3.14 matrix continues testing SDK/worker contracts and isolated installations.
Native VS Code/Emacs completion qualification remains open under T031/T108.
