# Immutable filesystem captures (T023)

`SourceSnapshot::capture` copies the T022 source allowlist, workspace configuration,
durable registry, dependency input and lock into a unique private staging directory
under `.transflow/runtime/source-snapshots`. Both dependency files are required.
The application caller supplies coordinator authority; this library never starts
Git, Python, a database or an installer.

Before copying it records file identity, size and modification/change timestamps.
It copies in bounded chunks, hashes the copied bytes, checks guards, reloads the
workspace and re-enumerates sources. Observable changes abort with a retry error.
A progress/cancellation observer makes failures at copy boundaries deterministic
in tests. Failed attempts remove only their own staging directory, preserving
source and existing captures. This is not an atomic multi-file snapshot of a live
filesystem; unobservable races remain possible.

Files and a strict format-1 JSON manifest are synced before installing the unique
capture by rename and syncing its parent. The manifest records the independent
snapshot UUID, workspace ID, source roots, every relative path/size/SHA-256, the
source-purpose content fingerprint and explicit `git: null`. Captures with equal
bytes have equal fingerprints and different snapshot IDs. Computation policy,
installed-environment identity and future registry overlays remain separate work.

Retained reopening verifies manifest compatibility, exact file membership, file
hashes and configuration identity. Added executable files and symlinks are rejected.
Bounded historical reads use retained bytes and verify their digest again; live
files are not reread. These are application-immutable files, not protection against
a hostile process running as the same OS user.

Copy limits are explicit: the default permits 256 MiB per source/resource file and
1 GiB total, with at most 100,000 enumerated source entries and a 16 MiB manifest.
These source-copy bounds do not impose a worker or dataframe memory ceiling.
Runtime cleanup/reachability, complete plan acceptance and crash recovery are later
tasks; an interrupted process can leave an unreferenced staging directory.
