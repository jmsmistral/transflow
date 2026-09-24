# Public planning and inspection commands

T054 exposes the existing shared preparation, freshness and traversal services.
The commands use the installed matched worker and prepared environment. They import
captured trusted Python declarations, but never call a producer function. Execution
is exposed separately by the [build commands](BUILDS.md).

```bash
transflow plan curated/customer_orders
transflow plan --target curated/customer_orders --mode selected --branch review
transflow why curated/customer_orders --boundary-policy require_available
transflow upstream curated/customer_orders
transflow downstream raw/orders --depth 3 --json
transflow upstream curated/customer_orders --git-ref HEAD --branch review --json
```

Use root-level `--workspace <directory>` before the command, or run from the
workspace or a child directory. `--python <executable>` selects the already prepared
interpreter. No command installs dependencies. `--help` shows the selected command's
options and works without a workspace or interpreter.

The data branch follows the shared explicit/Git/workspace-default policy. Explicit
`--git-ref` requires `--branch`; detached HEAD also requires an explicit branch when
follow-Git is enabled. Git-ref capture reads immutable objects without checkout,
reset, stash, hooks or filters. Its dependency inputs/lock must match the prepared
local environment; a different dependency environment fails visibly. The captured
configuration governs source roots, input policies and resource settings. Historical
registry proposals remain subject to current-authoring guards at acceptance.

## Plan and why

Positional targets and repeatable `--target` references are combined and duplicates
normalized. `why` requires one distinct target. Both commands accept:

- `--mode full|selected|between` (default full), repeatable `--boundary`, `--exclude`
  and `--refresh-source`, plus `--force` within that same scope.
- Ordered repeatable `--fallback` or conflicting `--no-fallback`.
- Repeatable `--pin dataset=version-uuid` or `--pin consumer#alias=version-uuid`.
  Exact pins are checked against retained versions, ownership and write conflicts.
- `--boundary-policy require_available|require_current`.
- Repeatable `--param name=JSON` or `--param dataset#name=JSON`.
- `--timeout-seconds` and `--validation-timeout-seconds`; zero disables the
  corresponding deadline. Resolved values and their precedence origins are saved.

Ordinary JSON booleans, strings and numeric values are normalized against their
declared parameter type. Complex or large values use the shared lossless
`ScalarValue` object, for example `{"type":"i64","value":"9223372036854775807"}`.
Unqualified names apply to compatible declarations within the producer scope.
Unknown/out-of-scope names, incompatible declarations, wrong types and overlapping
overrides fail. No parameter expression is evaluated as Python.

`plan` saves a fifteen-minute guarded draft and returns its ID, digest, creation and
expiry times, source/branch context, proposed registrations, prospective writes,
symbolic in-build parents, exact read versions and fallback/pin evidence. Branches,
registered dataset rows, heads and versions are not created. Runtime captures,
drafts and read leases may be retained. A missing input prevents an executable
draft; an invalid declaration anywhere prevents preparation.

`why` returns the target's separate materialization, data/logic/ancestor freshness,
latest attempt and output/input quality dimensions, with the shared causal reasons.
It also attempts a selection preview against the same captured source. If missing
boundaries or another selection refusal prevent a draft, `plan` is null and
`planning_error` explains why; the inspectable target status remains available.
An unknown target or invalid source is still an operational error. Foreign target
status is null and explicitly unknown. Removed local producer definitions retain
their inspectable registered identity/history.

Freshness is labelled `selected_branch_heads`; exact pins in the selection preview
remain separate historical evidence and do not redefine those branch-head facts.
The CLI does not invent future writer, clock or secret-version policies. Where
those semantics or legacy comparison evidence are absent, logic/currentness stays
unknown. `require_available` reports that limitation with warnings on boundary
reads. `require_current` rejects stale or unassessable boundaries without executing
them. Selected foreign inputs resolve through provider ownership and verified local replicas;
missing providers fail fresh planning, while retained exact pins can use local bytes. Unrelated foreign declarations
do not prevent local planning. Graph and why still expose the boundary.

## Graph output

`upstream` and `downstream` accept one reference plus `--depth`, branch/source and
interpreter context. Depth zero includes the start; omitted depth drains every
service page. Unique nodes use minimum hop distances and stable identity ties.
Adjacency entries retain each input alias, data/validation role, declared branch,
fallback suppression and normalized checks. Foreign upstream nodes are explicit
read boundaries; known local consumers remain traversable downstream.

JSON contains the full source/registry/certificate/branch context, unique nodes and
edges, requested depth, exact omitted counts, and distinct scope/delivery completeness.
All requested pages are delivered even when depth is supplied. No default semantic
node or byte truncation is added. The CLI builds one complete result in memory,
linear in the admitted graph; existing source/discovery admission bounds still
apply. Explicit `upstream --expand-external` adds bounded, version-labelled read-only
provider provenance with unavailable source/ancestor labels. It never imports provider
source or copies ancestor data; see [external data](EXTERNAL.md) for limits.

## Streams and contracts

Human output escapes control characters and uses headings and adjacency listings.
`--json` emits one complete envelope on stdout with no discovery import noise.
Visual control characters are escaped in encoded JSON without changing parsed
values. `GraphResultV1`, `PlanResultV1` and `WhyResultV1` are closed public shapes
in the shared generated contracts; CLI envelope version remains 1 and worker
protocol remains 1.0. Inspection results are exempt from the older 1 MiB informational
result cap so it cannot truncate complete lineage. Worker control-frame limits
are unchanged. Numeric counts/times and exact integers use lossless text carriers.

Unknown flags, malformed depths/pins/JSON, conflicting fallback switches and wait
switches fail as usage errors. Operational failures carry a stable diagnostic and
workspace/source context. `why` successfully explaining a blocked selection has
exit status zero and an explicit `planning_error`; `plan` refusing that selection
has exit status one. Neither command accepts `--wait` or `--no-wait`.

See [verification evidence](../../docs/development/verification.md) for actual checks
and CI status. The synthetic publication example is contributor-only test support;
it is not an installed command. See the
[customer-orders walkthrough](../../examples/customer-orders/README.md) for public builds.

T056 plans include resolved job/CPU/thread capacity, optional estimated memory
budget, independent validation/interactive/discovery timers and winning origins.
Omitted memory is JSON null with enforcement `none`. Per-producer
`wall_timeout_seconds` is honored before explicit CLI overrides. Discovery uses
the workspace execution timeout before declarations exist; compute CLI flags do
not override discovery. Counts remain exact decimal strings. These fields extend
the unreleased initial draft contract; prepare old development drafts again.
