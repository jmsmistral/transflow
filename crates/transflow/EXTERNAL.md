# External registrations

The CLI can explicitly register datasets belonging to another local workspace:

```bash
transflow --workspace ./analysis external add \
  --workspace ../market-data --dataset curated/security_master \
  --as market_data.security_master
transflow external list --limit 50
transflow external show market_data.security_master
transflow external remove market_data.security_master
transflow external remove market_data.security_master --yes
```

The root `--workspace` selects the consumer; the option inside `external add`
selects the provider. Add prints `external/market_data/security_master` and
`C.external.market_data.security_master`. Inspection/removal also accept the full
slash-separated reference. `--as` takes dot-separated identifiers with at least
one provider and one dataset segment. The `external` root cannot be an output.

Add reads provider configuration and its existing registry without importing any
provider Python, opening its runtime database or activating schedules. It works
before consumer environment setup. The provider dataset must already be registered;
its human path is display metadata, while workspace/dataset UUIDs are authoritative.
Two workspaces with identical dataset paths remain different providers and need
distinct provider aliases. A provider alias stays reserved after removal.

Omitted `--branch` captures the provider's configured default data branch, independently
of its current Git branch. Omitted fallback override preserves inheritance from the
provider at future planning time. Repeated `--fallback <branch>` freezes an explicit
ordered override; `--no-fallback` records an explicitly empty override. These flags
are mutually exclusive. Registration does not yet resolve versions or validate branch
availability against provider runtime state.

The tracked `.transflow/catalog.toml` contains only identities, aliases and selector
policy. `transflow.local.toml`, ignored by workspace initialization, holds absolute
provider directories keyed by provider UUID. Existing Python/secret references are
preserved when this file is rewritten. No machine path is emitted in registration
results. Add is idempotent for the same alias, identity and selector and can repair a
locator after moving a provider. It refuses to replace another identity or selector.

List/show read metadata without Python or a runtime writer. They include removed
registrations and distinguish available, unconfigured, unavailable, identity-mismatched
and missing-dataset locators. They never search for a replacement. Lists have bounded,
registry-revision-bound cursors; changes require restarting pagination. Replica status
is `not_inspected`, never an invented claim of up-to-date local data.

Remove previews current consumer input counts and runtime blockers. `--yes` is required
to write a tombstone. Consumer source inspection uses its prepared environment and
supports `--python`; it never calls producer bodies. Active builds/leases and saved
views/schedules conservatively block removal. Remove retains the registration UUID,
origin, selector and reserved alias, excluding it from new runtime C/input lookup.
Historical execution manifests and data are not deleted; local locator entries remain.

Both human output and `--json` use stdout. JSON adds a closed `ExternalResultV1` result
inside the existing envelope and advertises `external.add/list/show/remove`.

Registry changes use existing durable reconciliation and recovery. The local locator
is atomically installed first: interruption may leave an unused locator, but it cannot
implicitly create a registration or change identity. Retry with the same explicit
arguments. Filesystem/source guards reject stale authoring files and symlinked local
settings. As elsewhere, the final compare and POSIX rename are not a transaction with
non-cooperating editors. Local settings are normalized, so comments/formatting may change.

## Consuming registered data

Ordinary `plan` resolves selected external inputs and retains safe version metadata
and an expiring provider read pin, without copying or scanning whole data files.
`build` reads the exact provider files directly. Omitted input branches use the
registered default; `Branch.CURRENT` maps the consumer branch into the provider,
and named branches use provider policy. Registration fallback overrides win;
`stop_branch_fallback=True` suppresses all tails. Full/force execute local jobs only.

The running provider coordinator handles authenticated metadata/lease requests;
an idle provider uses transient metadata-only ownership. Neither imports provider
Python nor starts producers or schedules. Busy providers without a metadata endpoint
fail explicitly. Both coordinators must support the direct-read service version 2.

Before execution or cache reuse, SHA-256/manifest/schema/row-count verification scans
the exact selected bytes. The engine then reads provider paths, with provider leases
renewing every 30 seconds and expiring after fifteen minutes. Protection lasts through
input checks, lazy streaming materialisation, output checks and worker cleanup.
Renewal loss, identity mismatch and unavailable/corrupt files fail closed. Normal
completion/cancellation releases the leases; crashes leave bounded expiring pins.

No external input artifacts are copied, hard-linked or otherwise replicated into the
consumer object store. Only origin identities, publication time, schema/counts,
source digests, exact input provenance and safe output certificate metadata remain
locally. Consumer outputs are materialised normally, even if their bytes happen to
match an input. Provider source contents, samples and logs are not copied.

Fresh planning contacts the provider. Exact `--pin` and `build replay` also require
the provider and the original version; they never substitute newer data or an old
local replica. Historical provenance remains inspectable independently of provider
availability. Provider retention controls long-term replay availability. Zero rows
are valid; a nonempty requirement belongs in an explicit input expectation.

`upstream curated/result --expand-external` adds read-only, version-labelled ancestor
metadata, with workspace/dataset/version visited keys. Missing providers, ancestor
metadata and source contents are explicitly labelled. No ancestor artifacts are copied.
Use `--depth` to narrow expansion; explicit limits are 10,000 visited foreign versions,
100,000 edges/frontier entries and 16 MiB of foreign metadata. The local declaration
graph and executable build scope are unchanged. JSON includes `foreign_provenance`;
human output lists qualified versions and availability. Plan read records include
`origin_workspace` and the local external alias.

Runtime schema 10 retains qualified foreign job bindings against metadata, preserving
local-version foreign keys and migrating old history. Legacy replica records/files
are left untouched for explicit retention/cleanup but never used for consumption.
The legacy JSON `replica_status: not_inspected` field remains for wire compatibility;
human list/show identifies provider direct storage and does not claim inspected data.
Native editor setup and automatic overlay refresh remain deferred.
