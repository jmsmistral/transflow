# Declared lineage traversal

T053 supplies `tf-catalog::traversal` and `transflow::graph_query::inspect` for shared
CLI/API adapters. T054 exposes public upstream/downstream commands and closed JSON
results; T053 itself introduced no public wire contract, database schema or dependency.

`Graph::validated` accepts only a complete graph with a matching validation
certificate. It freezes active registered datasets, pending outputs, registered
foreign identities, and all declared inputs. Local aliases resolve to the same
stable identity; multiple foreign registrations retain their visible paths while
sharing one owner-qualified node. Removed definitions can still have registered
nodes; tombstoned identities cannot be resolved as active starting nodes.

The graph describes declarations, independently of execution scope and canvas
visibility. Data and validation-only inputs both contribute edges. Named branches,
stop-fallback flags and normalized check declarations are retained per input alias.
Repeated inputs remain separate edges even when they share endpoints. Traversal
does not resolve branch heads, select fallback data or infer freshness.

A request names a starting identity, direction and optional depth. The start is at
zero. Breadth-first search visits each identity once and computes its minimum hop
distance; alternate diamond paths never create repeated nodes or path lists.
Stable identities break ties at equal depths, with canonical paths for pending
outputs. Edges always retain parent-to-consumer orientation and are sorted by
consumer identity and alias. The result contains every declared edge whose two
endpoints are in the selected neighbourhood, including alternate/shared links.

Omitted depth includes all reachable local nodes and registered foreign boundaries.
There is no added semantic node/depth cap. Supplied depth must be a nonnegative
integer; signed, fractional and overflowing values fail explicitly. The evaluator
computes the full reachable distances even for limited depth so omitted-node and
omitted-edge counts are exact. Traversal uses linear graph/result storage with
ordered-map/set lookup costs; it does not recurse or enumerate full paths.

Foreign nodes have an explicit boundary marker. Upstream traversal ends there;
downstream traversal starting at a registered foreign node can show its known local
consumers. No provider workspace is opened, no provider graph is inferred, and no
foreign producer runs. Optional explicit foreign provenance expansion is later work.

`Traversal::page` delivers up to 100 nodes and 100 edges per page. Both streams have
independent offsets in the same cursor, so a high-alias node cannot cause unbounded
edge delivery or dropped edges after node delivery ends. Edges may reference nodes
on another page; adapters aggregate by identity. A page contains:

- The exact source UUID/digest, registry revision, validation certificate and branch.
- The resolved start, direction and requested depth.
- Total and remaining in-scope node/edge counts.
- Exact counts omitted by depth and a separate `scope_complete` flag.
- A next cursor until both in-scope delivery streams are exhausted.

A final page can still have `scope_complete=false` when depth deliberately excludes
continuation. A foreign boundary does not claim that provider lineage was expanded.
Cursors bind the complete graph certificate (including environment/configuration
and checks) and query. Changing source, registry, branch, start, direction or depth
requires restarting pagination. Changing transport page size is safe. Cursors are
read-position tokens, not authorization credentials. Pages use the frozen result;
no filesystem, catalogue, provider or head lookup occurs between pages.

`transflow::graph_query::inspect` captures source and discovers through the matched
installed worker on an owner-held blocking task. It validates the whole graph even
for depth zero and never uses a previous graph as an execution fallback. Module
imports still run ordinary trusted local Python. Producer calls, dataset scans,
builds and authoring registration do not occur. Pending outputs retain symbolic
identities and the authored catalogue bytes remain unchanged.

Qualification includes diamonds and shortcuts, 1,101-node wide/deep graphs,
250 independent aliases between two nodes, seeded DAGs against a topological
shortest-distance reference, foreign identity collisions, malformed/context-stale
cursors, and installed-worker depth/pagination journeys. See the [verification
report](../../docs/development/verification.md) for actual local results and owner-monitored CI status.
