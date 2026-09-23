# Workspace configuration (T021)

`WorkspaceConfig::parse` validates format 1 with strict TOML keys/types and bounded
input. `Workspace::load` selects the nearest ancestor workspace or an explicit
root, validates the registry, and resolves source directories relative to that
root. It rejects overlapping or escaped roots, runtime/environment roots and
nested workspace roots. It does not enumerate producers or execute Python/Git.

Defaults are master, src, Python 3.14, two jobs, detected CPU count, independent
3600-second transform/validation deadlines and a 30-second interactive deadline.
Memory admission is absent unless explicitly configured. Zero disables only the
specified deadline. Diagnostic sample rows do not turn exact checks into sampled
checks. Typed override structures expose definition, build/schedule and explicit
precedence with provenance; branch/pin resolution remains a separate service.

Local settings accept only `[python] executable`, `[external_workspaces]` keyed
by provider UUID with absolute directory values, and `[secret_references]` with
`env:VARIABLE` locators. They cannot change branches, parameters or check policy.
The environment service must report/fingerprint the selected interpreter before
execution. Parsing never resolves a secret value. Invalid TOML diagnostics retain
byte spans without echoing source excerpts or locator/secret contents.

Authoring reads use the already-qualified safe rustix API for nonblocking,
no-follow opens and verify regular-file identity. This adds no dependency version.
As with source capture, concurrent edits by the same user remain possible; this
is not an OS sandbox or atomic multi-file snapshot.

## Execution failure policy

`[execution]` accepts `max_attempts` (1–100, default 1), `retryable_classes`
(default empty; `transient_io` and `worker_unavailable` only), and
`abort_on_failure` (default true). These settings are frozen in accepted source
configuration. See [retry and restart behaviour](../transflow/BUILDS.md).
