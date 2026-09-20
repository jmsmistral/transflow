# Historical pins and retained replay context (T045)

`tf-plan::pins` parses `dataset=version-uuid` and
`consumer#alias=version-uuid` using exact registered paths/IDs. Dataset shorthand
may cover repeated aliases only when they share one effective starting branch,
candidate list and strictness. Otherwise the error lists binding-qualified
alternatives. Repeated identical overrides are idempotent; conflicting versions
and pin/write conflicts fail. Normal structural validation still precedes planning.
The Python `Input` API continues to reject a version keyword.

`Store::resolve_pin` checks the origin workspace/dataset and retained version,
then acquires an exact version lease in the same transaction. It never searches
branch heads, including when the declared selector is strict or its branch is
absent. Immutable head-change history supplies publication origin even after a
branch tombstone. Provenance retains the original selector and labels
`kind: exact_pin`, `selector_override: true`, an empty attempted-branch list and
`fallback_index: null`. The numeric `fallback_index()` convenience getter applies
only to non-exact reads; callers use `is_exact_pin()` to distinguish overrides.

The selected object still goes through the existing owner-held strict manifest,
hash, Parquet and schema verifier. Metadata availability is not byte integrity.
Unavailable, collecting, corrupt or incompatible data fails; no fallback is tried.
Consumer schema/check obligations and complete compute fingerprints are later
planner/evaluator integration, not bypassed by a pin.

Schema 5 adds immutable `replay_manifests`. `Store::retain_replay` is called during
accepted-build preparation with the complete original effective scope, parameters
and all resolved read boundaries. It captures the accepted plan's source snapshot,
code/file manifest, selector/Git and environment evidence from SQLite, and stores
consumer, alias, role, origin dataset/version/digest and selector context for each
boundary. Idempotent retries must have identical canonical evidence. Scope and
boundaries are bounded at 10,000 each and the canonical record at 1 MiB. This local
foundation rejects foreign boundaries; verified replica integration belongs to T072.

`Store::lease_replay` requires an explicit destination and acquires every original
boundary lease atomically. Missing manifests, mismatched original context,
wrong-owner versions or collection claims fail without committing partial leases.
It never reads current workspace configuration or heads, creates a branch, or
changes original selectors to the destination. `tf-exec::input_read::verify_replay`
then checks the original objects while retaining runtime ownership. Leases return
even on verification failure so the owner can release them; long readers must
renew them. Source/environment execution verification remains an explicit later
gate: this API returns retained metadata and verified objects, not a runnable build.

Manifests preserve history, not permanent data-retention promises. Existing retention
policy/pins govern artifacts; expired data makes replay unavailable. T049 wires
capture, environment and acceptance; T067 exposes public build/replay commands.
Internal producers execute/reuse under normal rules in that later composition.
