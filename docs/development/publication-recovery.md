# Publication failure qualification (T039)

The suite runs the production `tf-exec::publication` service with actual Parquet,
SQLite WAL/FULL and the runtime OS lock. A child first publishes a two-row baseline,
then attempts a distinct empty-table artifact. A bounded stdout readiness pipe
holds it at a specific boundary; the parent sends SIGKILL and reaps it. No sleep
chooses the failure window. The parent then acquires new runtime ownership,
reconciles the database and inspects persisted data.

| Interruption boundary | Head after restart | Candidate visibility |
| --- | --- | --- |
| File synced | Previous valid version | Private staging only |
| Manifest/staging synced | Previous valid version | Private staging only |
| Intent committed | Previous valid version | Journal metadata, no version |
| Before install rename | Previous valid version | Journal metadata, no version |
| After rename/sealing | Previous valid version | Invisible installed orphan |
| Both rename parents synced | Previous valid version | Invisible installed orphan |
| Before visibility commit | Previous valid version | Invisible installed orphan |
| Visibility transaction committed | New valid version | Committed success and outbox |
| Before notification | New valid version | Same durable event available for replay |

Each restart checks head/version/artifact links, full hashes and decoded row counts,
old successful/new interrupted-or-successful attempts, reservation release,
staging/orphan state, SQLite integrity and foreign keys. Source files are deleted
after the kill; replay still returns the original stable event identity and does
not create another version/event or materialize data again. Delivery is at least
once; consumers deduplicate by event ID.

Two additional matrices inject ENOSPC and EACCES at all nine boundaries. These
simulate returned I/O errors without filling a user's disk or changing real
workspace permissions. A separate test corrupts installed bytes just before
commit and confirms strict verification prevents publication. T038 tests cover
late SQL-trigger failure with full rollback, head conflicts, and cancellation
before and after commit. The async-drop test proves the actual OS lock remains
held while detached blocking publication work is still running.

The real subprocess test entrypoint is inert without the test harness environment.
The parent bounds readiness/output and kills/reaps children even when assertions
fail. No user datasets or screenshots are involved.

Run `cargo test -p tf-exec --test publication_recovery --locked --offline`, or
`bash tools/check-rust.sh`. Six test entrypoints cover nine SIGKILL cases,
eighteen injected I/O cases, post-install corruption and async ownership loss.
One entrypoint is the controlled child, not an extra scenario. A further test
verifies an earlier successful job remains published when a later job in the same
build fails and the build finishes FAILED.

These results qualify process interruption and returned errors on the tested
local filesystems. They are not hardware power-loss tests, kernel-crash tests,
adversarial owner-write protection or proof that a storage device honors flushes.
GC/read leases and full worker/process-group restart handling remain later tasks.
