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

Start with `GET /api/v1/context?branch=main`. Every domain request explicitly names
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
responses at 32 MiB. Logs, row previews, SSE, scheduling, UI panels and query helpers
remain their subsequent tasks. Read metadata is not a substitute for leased,
verified execution reads.
