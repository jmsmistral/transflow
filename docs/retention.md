# Retention and garbage collection

Transflow retains immutable publication history separately from physical data.
Garbage collection explains which local payloads are retained or collectible,
and removes bytes only after explicit approval and a fresh reachability check.

```bash
transflow --workspace /absolute/workspace gc
transflow --workspace /absolute/workspace --json gc --dry-run
# Review first. Stop the workspace coordinator before standalone apply:
transflow --workspace /absolute/workspace gc --apply --yes
```

A dry run opens an existing compatible runtime database read-only. It does not
write observations, retention clocks, deletion intents or payloads. SQLite may
maintain its transient WAL index while reading. Existing older databases must
first be migrated by the matched coordinator; dry run never performs migrations.

## What stays

Default version retention is the **union** of the latest 20 publications per
dataset/data branch and versions published within 30 days. It also protects:

- Live heads and exact explicit pins, including plan, schedule and backup pins.
- Active query/preview/provider read leases and accepted build inputs/candidates.
- Pending schedule tokens/occurrences and fixed schedule source snapshots.
- Local historical inputs followed transitively from every retained version.
- Source snapshots needed by those roots and the last valid descriptor graph.

Pins and dependencies can retain more than 20 versions or 30 days. A foreign
registration is metadata, not a request to copy or delete provider data. An
active external read protects its original provider bytes with a provider lease.
GC visits only the selected workspace's validated local runtime namespaces.

## Independent payload lifetimes

Failed candidates remain quarantined for seven days after a recorded terminal
attempt; unknown/incomplete terminal evidence keeps them protected. They never
become input versions. Completed attempt logs and their temporary output tree
have independent 30-day retention; active or unrecognized attempts are retained.
Worker log caps remain in force separately from collection.

Unknown object bytes must pass full artifact/Parquet verification and receive a
durable first-seen observation before a seven-day orphan grace period can begin.
Unrooted source captures similarly require strict manifest/file verification,
workspace identity agreement and seven days of observed grace. Directory mtime
is never deletion authority. Unknown staging/namespace entries are retained for
inspection, and unsupported legacy root/claim state blocks collection.

## Apply and interrupted collection

Standalone apply acquires the real workspace runtime owner. A running coordinator
causes a refusal; the command does not kill it. Each candidate gets a fresh root
check and durable deletion intent. Artifact claims fence new reads, pins and
publication. Source claims fence new source pins and plan acceptance.

Only the verified directory identity is moved into an intent-specific private
location. Descriptor-relative operations reject symlinks, special files, hardlinks,
substituted directories and device crossings. After deletion is durable, SQLite
records completion and marks collected artifacts `EXPIRED`. Immutable versions,
checks and provenance remain available as metadata.

A subsequent explicit apply resumes the exact interrupted intent, including a
partially removed private directory. Claims stay fenced on failure. Earlier
items may already have completed; use another dry run to inspect the recorded
state. GC never promises a rollback of completed physical deletion.

Reports are bounded to 2,000 filesystem/metadata entries and 512 KiB before
mutations. Database/root inventories cap at 100,000 rows; each physical tree caps
at 100,000 entries, 64 levels and 16 MiB of names. A limit failure is explicit;
there is no silent partial report or unreported sweep.

## Expired data

Version inspection returns `availability: "expired"` for collected local bytes.
The lineage preview says **Data expired**, keeps schema/history visible and does
not issue a preview request. Exact reads and replay fail when required bytes or
captured sources are unavailable; they never substitute today's head silently.
A new verified execution/import can recreate identical bytes and publish a new
version. That does not fabricate a past run or a successful replay.

Backups and restore are separate operations. Their existing pin interface is
protected by GC; this command does not implement a backup format or restore tool.
