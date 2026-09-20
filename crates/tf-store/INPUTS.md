# Input branch resolution and binding identity (T042–T043)

`tf-domain::input` captures local fallback policy and normalizes each binding.
Omitted/CURRENT use the build output branch and may take the explicit build tail.
Named selectors use their own starting-branch rule, even when named like the
output branch. An authored empty rule and an explicit empty override stay empty.
Strict inputs remove the entire tail. Candidate order preserves first occurrence;
there is no recursive expansion of another branch's policy. Limits are explicit:
1,024 rules/tail entries and 4,096 UTF-8 bytes per branch name.

`tf-catalog::input_bindings::local_bindings` requires a successful graph certificate
matching the supplied validation context and registered dataset identities. It
returns a map keyed by consumer dataset **and alias**, retaining role and qualified
origin. It does not delete off-branch or validation-only declaration edges; complete
graph cycle validation still runs first. Provider registration policy is later
work: the local adapter explicitly rejects foreign bindings instead of applying
consumer defaults to them. A captured policy is independent of later config edits.

`InputBinding::disposition` distinguishes a planned producer result from a read
boundary. Local inputs starting at the output branch bind to an eligible producer
in the write set, including a named selector equal to output. Named off-branch
reads and foreign origins never add writes on those branches/workspaces. The
later planner creates symbolic job dependencies and binds their successful/cached
results; resolving a planned producer as a historical read is refused here.

`Store::resolve_input` walks the normalized candidates within one IMMEDIATE SQLite
transaction and leases the first published head before committing. It records
absent/deleted branches and missing heads, actual branch ID/name, exact version,
policy origin and fallback index. It creates no branches. An independent named
selection is not itself fallback. Failed latest attempts are not head authority;
the last committed head remains usable. Missing versions/artifact metadata,
collection claims and database errors stop resolution rather than advancing.

`tf-exec::input_read::verify` strictly verifies the selected manifest, hashes,
Parquet/schema/row evidence on a blocking task that retains actual runtime ownership.
The selected read and lease ticket return on success **or failure** so the caller
can release it. Callers renew the ticket for long reads/checks and release it at
completion/cancellation. If a caller disappears, the persisted lease expires; the
blocking task retains ownership until it actually finishes. A corrupt/missing or
incompatible artifact never invokes branch resolution again. A lease does not
make a stale plan current or make an input check pass.

`ResolvedRead::provenance` plugs directly into guarded publication and immutable
`version_inputs`; repeated datasets retain distinct aliases, roles, versions and
requested/resolved branches. `semantic_value` supplies deterministic per-binding
material for the later full compute fingerprint, including validation-only aliases.
It is not itself a complete computation cache key.

This is shared service functionality, not public build/plan commands. Boundary
freshness (T052), actual check evaluation (T060), full planning/execution composition
and provider transport remain their own tasks. Freshness or check refusal must fail
the selected binding, never re-enter fallback. Tests use real SQLite, published
Parquet objects and owner guards; they also confirm failed required input evidence
preserves the prior output head and keeps the exact input binding.

Explicit historical overrides and original replay context are described in the
[pin and replay guide](REPLAY.md); they never re-enter branch fallback.
