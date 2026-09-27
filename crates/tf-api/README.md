# Local HTTP API

`transflow serve` runs the existing CLI coordinator and an authenticated HTTP
listener on `127.0.0.1`. `--port N` selects a port; the default is an available port.
`--open` launches the system browser with a one-use, 60-second fragment code.
The initial page verifies the connection; the workspace UI remains later work.

The private, mode-0600 `.transflow/runtime/runtime.json` records `http_endpoint`
and the session bearer credential in `nonce`. Keep that credential private. CLI
integrations send `Authorization: Bearer …`, never a query parameter. Authenticate
`GET /api/v1/capabilities` and `/api/v1/openapi.json` to discover the delivered API.
The generated [OpenAPI document](../../schemas/generated/openapi-v1.json) and
[shared contracts](../../schemas/contracts-v1.schema.json) define the wire shapes.

Browser launch exchanges the fragment for an HttpOnly, SameSite=Strict cookie
and a CSRF value held only in page memory. Host and Origin must match exactly;
requests without Origin require bearer authentication. Since ordinary browser
GETs often omit Origin, browser clients use `POST /api/v1/read` with the session
CSRF header and `{ "path": "/api/v1/context", "query": { "branch": "main" } }`.
This facade invokes only a registered GET operation. It cannot execute a mutation.
Reloading the bootstrap page requires a fresh launch link. There is no wildcard
CORS, token query support, remote bind, web-storage credential or background daemon.

## Context and reads

Start with `GET /api/v1/context?branch=main`. Every contextual domain request explicitly names
a data branch. Supply the returned `context` fingerprint on subsequent requests;
a changed registry, configuration or runtime revision returns HTTP 409. Cursors
bind the context, route and filters. Refresh the context and restart pagination
on conflict. Pages default to 50 and cap at 200; graph pages default to 100 and
cap at 500, with `expand=true` required when the selected graph exceeds 500 nodes.

Delivered reads include datasets, exact dataset details, retained versions,
branches, branch-filtered build history, exact builds, lineage, saved plans and
captured source. Catalogue filters are `filter` (path substring), `origin`, `id`,
`source_path`, `tag` and `column`. Unknown producer/tags/schema/freshness remain
unknown. A tag/column filter matches only known retained metadata. Head metadata
uses the shared ordered branch policy and exposes integrity and resolution facts;
it does not verify bytes or assert currentness. Failed integrity cannot trigger
fallback to another branch.
Catalogue enrichment caps at 16 MiB per request; narrow the path or identity filter
if that bound is reached. Pagination does not authorize unbounded metadata loading.

Current reads use `retained_current` definition metadata, which can predate edits
in the working copy. `plan=UUID` selects a saved plan's source and proposed registry.
For historical data, use `dataset=UUID&version=UUID`, adding `origin_workspace=UUID`
for a foreign dataset. Local versions select their producing capture and retained
graph when available. Imported versions without a retained graph explicitly lack
lineage. Foreign versions retain provider metadata and never masquerade as local
source captures. Use `origin_workspace` when inspecting foreign dataset IDs.

`GET /api/v1/source/{source}` accepts a captured relative `path`, line `offset`
(default zero), and `limit` (default 200, maximum 1,000). Excerpts cap at 64 KiB.
Working-copy paths, traversal and a snapshot different from the selected context
are refused. GETs never import Python, acquire a writer, contact a provider or copy
external data. Validation/diff POSTs explicitly run trusted local discovery.

## Commands and retries

`POST /validations` and `/catalog/diff` below `/api/v1` share the CLI preparation
services and accept an optional `python` executable. Mutations additionally need
`Idempotency-Key: UUID` and `If-Match: "context-fingerprint"`:

- `/plans` accepts a closed selection with required `branch` and `targets`, plus
  mode, boundaries, exclusions, source refreshes, pins, fallbacks, force, parameter,
  Git-ref and independent timeout options.
- `/builds` accepts either `{ "kind": "request", "request": { … } }` or
  `{ "kind": "plan", "plan_id": "…" }`. The saved variant requires the matching
  `plan` context and permits no overrides. Both use full shared validation/acceptance.
- `/builds/{id}/cancel` accepts `{}` and works while workers run, requiring the
  selected branch to match the build. The cancellation
  and publication race is decided by the existing durable coordinator transaction.
- `/catalog/sync`, `/catalog`, `/branches` and `/externals` call the existing guarded
  services. Lifecycle bodies explicitly name `operation` and its fields; destructive
  application still requires `yes: true`. External `as` uses the CLI's dotted alias
  form, such as `provider.items` (canonical input path `external/provider/items`).

Accepted work continues after HTTP disconnection. One coordinator owns mutations;
busy/full queues return a retryable 503. Successful retries with the same key and
identical request return the retained result, including across restart. Reusing a
key for different input conflicts. Schema 11 adds immutable operation receipts.
An incomplete receipt after a crash returns 409 and **never repeats its effect**.
Inspect build history and retained evidence before deliberately issuing a new key.
There is no claim of an atomic transaction spanning authoring files and the HTTP
receipt. Existing registry/publication recovery remains authoritative.

Only the context lookup establishes a new view; a mutation never silently refreshes
its submitted revision. Historical contexts are inspection-only except explicit
saved-plan acceptance and identified cancellation. Read responses can conflict
while publication is in progress; retry reads with a newly selected context.

Errors contain safe codes/messages, retryability and server-generated request IDs.
Bodies cap at 2 MiB, application concurrency at 16, the mutation queue at one and
metadata responses at 32 MiB. Preview and event bounds are stricter, as described below.
Logs, scheduling, UI panels and SQL query helpers remain subsequent tasks. Read metadata is not a substitute for leased,
verified execution reads.


## Committed events

`GET /api/v1/events/page?after=SEQUENCE` returns at most 100 committed facts and
1 MiB of stored payload. `GET /api/v1/events` delivers the same facts as SSE;
resume with `Last-Event-ID` or `after`. Browser clients use the authenticated
`POST /api/v1/read` facade with path `/api/v1/events` and consume its streaming
response with fetch. Native EventSource cannot supply this facade's CSRF header.

Facts carry decimal-string sequence, UUID, workspace, type, safe payload,
correlation/causation, available source/branch context and UTC microseconds.
Publication/head facts retain their existing transaction. Schema 12 adds build,
job and attempt state notifications in the same transaction as each state change.
A rolled-back transition has no visible event. Heartbeats and raw logs are excluded.
The stream covers the workspace; it does not authorize painting another branch's
state into a selected inspector. Treat facts as invalidations and refetch the
appropriate contextual read models. The frontend `EventCursor` helper deduplicates
sequence/UUID delivery without converting large sequence numbers to JS numbers.

No `after` establishes a current checkpoint. Reconnect can replay the last 10,000
sequences; older or future cursors explicitly return `resync_required`. This is a
delivery window, not deletion of immutable audit evidence. To resynchronize, take
the returned checkpoint, refetch context/build/dataset read models, then resume
from that checkpoint. Facts racing the refetch can repeat; never guess missing
transitions. Persisted build/job terminal state survives the replay window.

Four streams have separate capacity from the 16 ordinary HTTP operations. Each
holds one bounded page and a one-message channel. A blocked send closes after five
seconds, idle polls run every 500 ms, and streams reconnect after 60 seconds.
Disconnect/slow readers hold no writer transaction and cannot backpressure builds.
Session validity is checked throughout delivery; reconnect uses normal auth.

## Exact-version previews

`POST /api/v1/previews?branch=main` accepts, for example:

```json
{
  "dataset": "11111111-1111-4111-8111-111111111111",
  "origin_workspace": "22222222-2222-4222-8222-222222222222",
  "version": "33333333-3333-4333-8333-333333333333",
  "columns": ["id", "label"],
  "rows": 100
}
```

Use IDs from retained dataset/version metadata. Pass `next_cursor` as `cursor`
with the same branch, origin, dataset, exact version and projection. Cursors are
unguessable server-side handles to physical file/row-group/row positions, valid
for five minutes in a 256-entry cache. They hold no data or lease between requests.
Wrong context, expiry, eviction or coordinator restart returns 409; restart paging
at the chosen version. New head publications do not rebind that exact selection.
Positions describe immutable physical order, not a semantic sort.

The workspace's `interactive.preview_rows`, `max_result_rows`, `max_result_bytes`
and independent `query_timeout_seconds` apply. Transport ceilings are 1,000 rows,
128 projected columns and a 2 MiB complete JSON envelope; encoded rows occupy at
most 1.5 MiB to leave schema/context space. A page decodes projected columns in
batches of 128 from one row group at a time. Footer metadata is capped at 8 MiB
and selected row-group uncompressed bytes at 64 MiB. An oversized projection fails
explicitly; reduce columns or use smaller writer row groups. The interactive timer
is cooperative between decoded batches/rows, not a hard interrupt of a filesystem
read. Zero disables that timer; the other bounds remain enforced.

Cells use shared lossless wire values: null, NaN/infinity, integer/decimal strings,
timestamp precision/timezone and nested values stay distinct. The default encoded
cell cap is 4 KiB. A larger cell is `{ "value": null, "truncated": true }`, distinct
from a real null `{ "value": { "type": "null" }, "truncated": false }`. Explicit
bounded inspection can repeat a one-row page with `cell_bytes` up to 65,536.

The header reports exact version, requested branch, recorded publication branch,
publication time and producing source. There is no latest-head fallback in this
endpoint. Local and external reads acquire and renew provider query leases through
the existing owner channel, then release them when the request finishes or fails.
An external preview opens the registered provider's immutable files directly;
it never copies inputs or executes provider code. Between pages, retention remains
the provider's responsibility; an unavailable version fails visibly.

`integrity: "projected_read"` means contained regular files, canonical manifest,
lengths, schema and selected Parquet decoding were checked. Preview intentionally
does not hash/decode every file or assert freshness/quality PASS. Full execution
integrity checks remain unchanged. Preview is a CSRF-protected read operation;
it does not require a mutation idempotency key.

## Interactive queries

T080 adds four authenticated, context-bound routes. Creation and cancellation
require the existing If-Match/Idempotency-Key guards (and CSRF for browser sessions):

| Route | Behaviour |
| --- | --- |
| `POST /api/v1/queries?branch=main` | Accept SQL, separate parameters and explicit version bindings; return a query ID and QUEUED status. |
| `GET /api/v1/queries/{id}?branch=main` | Read frozen SQL/bindings/parameters, state, limits, elapsed time and truncation. |
| `GET /api/v1/queries/{id}/results?branch=main&offset=0&limit=200` | Page typed scalar/nested wire cells under the original schema/context; at most 1,000 rows per page. |
| `POST /api/v1/queries/{id}/cancel?branch=main` | Request cancellation with `{}`; observe CANCELED after cleanup. |

Creation body (UUID values must identify existing registered datasets/versions):

```json
{
  "sql": "SELECT id FROM dataset WHERE id > CAST(? AS BIGINT)",
  "parameters": ["10"],
  "bindings": [
    {
      "alias": "dataset",
      "dataset": "11111111-1111-4111-8111-111111111111",
      "origin_workspace": "22222222-2222-4222-8222-222222222222",
      "version": "33333333-3333-4333-8333-333333333333"
    }
  ],
  "rows": 1000,
  "bytes": 2097152,
  "spill_bytes": "268435456"
}
```

Bindings are required (1–32); paths/interpreters are not request fields. Local and
foreign datasets must be visible in the selected registry. Foreign immutable
files are read directly under renewable provider leases, without producer execution
or byte replication. An unavailable version fails explicitly; a newer head never
rebinds an accepted query. SQL still obeys the [reviewed helper subset](../../python/worker/SQL.md).

Optional rows/bytes may lower the workspace `interactive.max_result_rows` and
`max_result_bytes`; service ceilings are 1,000 and 2 MiB. Both Arrow IPC and JSON
are bounded; oversized rows can produce an empty truncated result. The entire
HTTP result envelope is checked too; a 413 asks for a smaller page. Spill defaults
to 256 MiB, accepts a smaller decimal-string byte allocation including zero, and
is confined to a fresh private directory. These output caps do not bound SQL
computation. The independent `interactive.query_timeout_seconds` defaults to 30;
zero disables that deadline only. Elapsed status time includes queue/cleanup time;
the execution budget excludes queue wait and supervised termination grace.

State proceeds QUEUED → RUNNING → SUCCEEDED/FAILED/CANCELED/TIMED_OUT. Four queries
may be active/queued; completed statuses/results occupy at most 64 cache slots.
A full cache returns busy rather than evicting unexpired results. Workers share
the coordinator's job/CPU/memory admission gate with builds; no query bypasses
build capacity. Persistent capacity is frozen at serve startup; CPU/memory changes
require a restart. Per-build job ceilings remain bounded by coordinator capacity.
Absent explicit memory configuration, no Transflow memory limit is installed.

Completed query IDs/results expire after five minutes (410 while the expired
receipt remains, otherwise 404) and are unavailable after restart. Creation
retries with the same key and request replay the same live ID; a changed request
conflicts. Always use a new key for a new query, including after expiry. Retained
results release input leases as soon as helper/result acceptance finishes; result
pagination does not keep provider data pinned. Cancellation is idempotent and a
terminal query remains terminal. Shutdown cancels active queries, drains provider
releases, and reaps workers. Crash recovery removes orphan helper directories;
finite provider lease expiry covers unreachable-provider/crash cases. Nothing is
published into the dataset catalog. No query UI is delivered in T080.

Query capabilities require the matched installed worker and pinned DuckDB/PyArrow
packages. Prepare them explicitly with `env lock`/`env sync`, then restart `serve`.
Missing query dependencies leave ordinary metadata/build endpoints available.


## Execution history and metrics

All these GET routes require bearer authentication and `branch`; browser clients
use the existing authenticated read facade. Responses carry the frozen context.

- `datasets/{dataset}/history`: keyset pages (`limit` 1–200, `cursor`) of jobs,
  with produced/reused version, original attempt, attempt count and accepted plan.
  Failed jobs remain visible without a version. `/versions` remains a separate
  retained-version projection and includes committed row, byte and file counts.
- `builds/{build}/timeline?plan={plan}`: actual phase intervals per attempt,
  state counts, wall duration, initial queue wait and recorded critical path.
- `attempts/{attempt}?plan={plan}`: inputs, frozen parameters, checks, retained
  process evidence, exact source ID/availability and the retained CLI log command.
  Use `/source/{source}?plan={plan}&path=...` for captured code. Missing source
  never falls back to current files. Build detail retains requester/trigger/targets.
- `metrics?from_us=...&to_us=...`: half-open UTC **build acceptance** window;
  optional local `dataset`, `materialized_any=true`, `window=10` (1–1000).
  Returns build/job state counts, executed jobs, attempts, committed materializations,
  measured/missing duration counts, median, trailing mean and failure rate.

Ratios are exact `{numerator, denominator}` decimal-string objects; durations are
nanoseconds. Empty samples give null statistics. Failure rate is failed divided
by succeeded+failed, including cached successes and excluding canceled/interrupted
builds. Successful phase sums measure start through commit; queue and retry waits
are excluded. Median uses measured successes. The trailing mean uses the last N
successful materializations by finish time/attempt UUID, and is null if any selected
timing is missing. `trailing_samples` reports the actual measured count.

Critical paths sum recorded attempt durations along accepted dependencies, including
failed retries. Cached jobs have no execution duration/attempt, while contributing
zero path weight. Active builds or incomplete evidence have no critical path. ETA
and resource wait are null; initial queue wait is labelled wall time, not compute.
No percentage progress or fabricated Gantt intervals are returned.

Cross-version history/metrics reject `plan`, `version` and foreign-origin selection;
exact-version metadata/source and plan-bound attempt/timeline routes are separate.
Cursors bind the context and filters, so branch/authoring/publication changes require
a fresh page. Metrics reject over 1,000 builds or 10,000 jobs/attempts: narrow the
range or dataset. Full build evidence is limited to 2,000 jobs, 10,000 attempts,
50,000 intervals and 32 MiB. No oversized read silently truncates statistics.
Manual request and scheduled-build counts are available; `schedule_occurrences`
is null until scheduling supplies accepted/ignored/coalesced occurrence evidence.
Charts remain later frontend tasks. No provider data is copied or scanned.
