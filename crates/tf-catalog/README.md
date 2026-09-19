# Durable catalogue parsing (T020)

`RegistrySnapshot::parse(workspace_id, text)` reads one format-1 durable registry into
immutable typed records and exact indexes. It performs no filesystem access, Python
imports, DB reads, UUID allocation or registration. The caller explicitly supplies
workspace identity and file bytes; moving the file or deleting runtime state cannot
change an ID. Mutating reconciliation and rename commands remain later tasks.

The parser uses pinned `toml` 1.1.6 with unknown fields denied at every record level.
The registry owns identities and names. Producer modules, functions, engines, inputs,
checks and machine-local provider paths are rejected rather than duplicated here.

```toml
format_version = 1

[[datasets]]
id = "00000000-0000-4000-8000-000000000001"
path = "curated/orders"
kind = "transform"

[[aliases]]
path = "legacy/orders"
target_id = "00000000-0000-4000-8000-000000000001"

[[tombstones]]
id = "00000000-0000-4000-8000-000000000002"
path = "retired/orders"
kind = "imported"

[[external_registrations]]
id = "00000000-0000-4000-8000-000000000003"
alias = "external/reference/currencies"
provider_workspace_id = "00000000-0000-4000-8000-000000000004"
provider_dataset_id = "00000000-0000-4000-8000-000000000005"
provider_display_path = "reference/currencies"
default_branch = "master"
fallback_override = []
```

Each top-level record array may be omitted. Local kinds are `transform`, `source` and
`imported`. Tombstones retain both ID and canonical name, and cannot overlap active
records. Aliases point directly to active local IDs, never to another alias or a
foreign registration. Duplicate IDs, names, aliases or registration IDs fail with
record locations and the conflicting earlier location. Dataset and registration IDs
are distinct roles; foreign dataset IDs remain qualified by their provider workspace.

External aliases require `external/<provider-alias>/<dataset-path>`. Default branch
is explicit; omitted fallback_override means provider policy must be captured later,
while an empty array explicitly selects no fallbacks. Order is preserved. Multiple
registrations may reference the same foreign key under distinct local aliases and
policies. They remain separate registration identities. Provider display paths are
non-authoritative; a provider path rename does not change its key. Locators belong
in ignored local configuration and are not accepted by this parser.

## Lookup and fingerprints

`resolve` accepts an exact path or `dataset:<uuid>` for an active local identity.
There is no fuzzy match, current-directory namespace, unknown-input allocation or
unregistered foreign UUID escape hatch. Namespace-only references and tombstones
produce distinct errors. A dataset may also have child datasets at longer paths.
`resolve_output` rejects foreign read boundaries. Public getters expose immutable
references; lookups cannot mutate the registry or its fingerprints.

`raw_digest` is SHA-256 of original bytes, for later expected-old/new replacement
checks. `fingerprint` hashes canonical JSON with the T015 catalogue prefix and an
explicit `registry_semantics_version: 1` envelope. It includes the workspace and all
four record arrays; arrays sort by stable ID or alias path while ordered fallback
policies remain ordered. Comments, whitespace and record ordering change only the
raw digest. Names, kinds, tombstones, aliases and provider policy affect semantics.
This complete registry fingerprint is distinct from the narrower SDK
`CatalogSnapshotV1` wire projection. No shared wire schema changed in this task.

Input is bounded to 16 MiB and 100,000 records. Paths are at most 4096 bytes/64 segments;
branches are at most 4096 bytes, and fallback arrays at most 1000 entries. Canonical
metadata retains T015's independent 16 MiB limit. Syntax errors expose parser byte
ranges; semantic errors expose safe record pointers. Raw TOML and parser error text
are not retained in diagnostics, preventing accidental secret/source echo.

Ten tests cover exact references, collisions, unknown fields, aliases/tombstones,
foreign policies, semantic hashing, file moves/runtime deletion, bounded input and
5,000-dataset lookup/order invariance. These establish A45/A50/A53 parser prerequisites.
Code discovery, graph validation, SDK snapshot integration and registry mutation
journaling remain later tasks; this parser does not implement authoring CLI commands.
