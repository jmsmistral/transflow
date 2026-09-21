# Resource, cancellation and resolver feasibility

T011 adds ten capability tests using the existing pinned qualification environment.
They pass locally on macOS arm64 with Python 3.14.7. The native
qualification workflow passed all six Python/platform jobs at `a46aca6`,
covering macOS arm64 and Linux x86_64/arm64 with Python 3.14.7.
[CI evidence](../../../docs/development/evidence/t011-ci.json) records the successful runs.
The [local evidence snapshot](../../../docs/development/evidence/t011-macos-arm64.json)
retains the actual engine settings, RSS observations and resolver results.

From the implementation root, after the explicit dependency setup in the
[compatibility guide](../../../docs/development/compatibility.md):

```bash
target/qualification/py314/bin/python tools/qualification/resources/probe.py --output target/qualification/t011-py314.json
```

The full `tools/qualification/check.sh` runner also writes
`resource-capabilities.json` alongside its other native reports. Normal checks
never fetch dependencies. The contributor aggregate lints/formats the probes;
engine tests use the separate qualification environments. No wheel/runtime
module or application CLI was added by this spike.

## Observed behavior

| Capability | Evidence |
|---|---|
| No default Transflow memory gate | Omitted memory budget admits even a synthetic estimate above machine capacity; no process ceiling is set. This tests admission policy, not allocating that amount. |
| Optional memory reservation | An explicit 100-byte synthetic budget admits a 60-byte request with 40 reserved and rejects 61. Reservations are not RSS enforcement. |
| Engine defaults | Unmodified DuckDB memory limit reports 102.3 GiB on this host. Polars has no Transflow cap; the probe reports measured peak RSS, without claiming the engine or OS is unlimited. |
| Optional engine limit | DuckDB with an explicit 1 MB buffer limit and zero spill allowance raises OutOfMemoryException for the sort, then still accepts a small query. This is not a process RSS ceiling. |
| Unsupported hard limits | An explicit process hard-limit request fails on every platform in this prototype. No cgroup backend is implemented or advertised. |
| Independent phase clocks | Default compute/validation deadlines are 3600 seconds; interactive is 30. With injected elapsed time, the interactive child stops at 31 while a real DuckDB validation child remains alive at 3599 and stops at 3600. |
| Phase budget boundaries | Queries share the validation phase's start; output validation starts a separate budget; a prior compute deadline does not become an overall job deadline. |
| Disabled deadlines | With phase timeout 0, real Polars/DuckDB children remain active past injected elapsed time and still terminate/reap on explicit cancellation. |
| Process group cleanup | SIGTERM reaches a managed parent and descendant; the parent reaps its child. A separate SIGTERM-ignoring child requires SIGKILL after a 50 ms test grace period. |
| Failure behavior | Timeout produces ERROR with phase, check name, elapsed/limit, config key/source, logs and override advice. Caller failure reaps the fixture process and preserves unrelated candidate/last-good files. |
| Hash-lock resolver | pip 26.2.1/pip-tools 7.6.1 resolve synthetic wheels offline, select the allowed transitive version, reproduce the lock, install both modules in a clean environment, and reject a tampered wheel without changing the installed environment. |

Polars and DuckDB each complete a real query before signalling readiness and then
continue a bounded-data compute loop. Thread limits are set before imports or
connection creation. Measured RSS is peak process resident memory, with platform
units normalized to bytes. It is evidence about this fixture, not a workload
budget or a claim that cancellation always has the measured fixture latency.

## Qualification boundaries

The [ADR-019 decision](../../../../transflow-spec/docs/adr/ADR-019.md) records the
future runtime requirements. This is a Python feasibility helper, not the Rust
coordinator, reservation service, version publication protocol or worker transport.
Injected clock values avoid waiting an hour; no test claims an hour-long wall run.
Signal delivery, grace expiry and reaping are real POSIX operations. Production
cancellation must additionally fence session/start identity, manage retained logs
and leases, persist state, and handle every publication boundary.

Fixture output is fixed and bounded. These helpers are not suitable for arbitrary
user subprocess output or hostile processes that escape their process group.
The default one-second fixture cleanup grace and the 50 ms escalation test do not
change the production five-second grace. A file preserved beside a terminated
fixture is not proof of database/publication recovery.

Optional Linux cgroup support remains separately qualified work. Using RLIMIT_AS
as a portable RSS ceiling or replacing actual cancellation with dropping a handle
would not satisfy that requirement. No Transflow runtime memory ceiling, new
package manager, hidden dependency download or application command is introduced.

## Primary references

Python documents [session creation and explicit subprocess cleanup](https://docs.python.org/3/library/subprocess.html).
Polars documents [thread-pool setup before process start](https://docs.pola.rs/docs/python/dev/reference/api/polars.thread_pool_size.html)
and [streaming execution](https://docs.pola.rs/user-guide/concepts/streaming/).
The resolver uses [pip-tools hash generation](https://pip-tools.readthedocs.io/en/stable/#using-hashes)
and [pip hash-checking installs](https://pip.pypa.io/en/stable/topics/secure-installs/).
These references explain mechanisms; the executable reports establish behavior
for the pinned versions actually tested.
