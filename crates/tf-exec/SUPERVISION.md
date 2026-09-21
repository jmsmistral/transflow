# Worker supervision (T055–T058)

`tf_exec::supervisor` owns one fresh process group per attempt. `Launch` takes an
absolute, already verified interpreter, an operation enum, a schema-validated
request, resolved supervision policy and optional known secret values. It passes
an argument array with `-I -B -m transflow_worker`; no shell or ambient PYTHONPATH
is involved. Environment checking remains the caller's responsibility.

Each launch creates a short private 0700 directory, a create-new 0600 request and
a private Unix socket. Requests and frames are limited to 1 MiB. Before imports,
the worker proves possession of the request's random 32-byte nonce, then verifies
the coordinator's acknowledgement. Neither nonce nor request contents enter argv.
This strengthens the unreleased bootstrap handshake; Rust and the matched Python
worker must be updated together. Framed protocol 1.0 and authored schemas do not
change. This is isolation of trusted code, not a hostile-code sandbox.

The owner incrementally reads framed control data and independently drains both
nonblocking pipes. A tick admits at most eight frames and 64 KiB from each log
stream. It checks identities, sequence, protocol/capabilities, operation-specific
phase progression, result phase/uniqueness and terminal ordering. Unexpected EOF,
partial/oversized/invalid frames, success without a required result, or a nonzero
exit cannot become success. Only one result reference and terminal are retained;
metric and heartbeat floods cannot grow an event queue. Data remains in files.

`Policy` defaults to a ten-second connect/authentication/hello deadline, a
30-second heartbeat-silence diagnostic threshold, five-second termination grace,
and 10 MiB retained per stream. Heartbeat silence is evidence, not a failure
inference. There is no default whole-operation timeout or memory cap. The caller
can supply an explicit operation deadline only when no phase budget is attached.
[Independent phase timers and admission](RESOURCES.md) now compose with this owner.
Discovery uses the workspace execution-derived timer, defaulting to 3600 seconds. After a terminal message, the
process must exit within the termination grace (at least one second); this bounds
shutdown only, not user work.

Logs retain redacted byte prefixes and raw observed-byte counts. The configurable
retention limit must fit the supplied workspace cap; both default to 10 MiB.
Marker space is reserved inside the cap. Excess output is drained and discarded,
with `[transflow: log truncated]` retained explicitly. Raw bytes remain bytes,
including invalid UTF-8. Exact known secrets and nonce forms are removed across
read boundaries; authorization, proxy-authorization, cookie, set-cookie and
x-api-key header values are suppressed through line end. This cannot detect
arbitrary secrets or transformations of them. Debug formatting excludes log text.

A caller may provide a canonical private attempt directory. Log files are opened
create-new relative to its directory descriptor before starting Python, with
0600 permissions and no symlink following. They are flushed on normal cleanup,
including crash/protocol-error/cancellation. Retention belongs to the caller.
`Report` preserves both streams, typed outcome, terminal/result evidence, PID/start identity, native
exit status and whether both streams reached EOF. Process success is not authority
to publish a dataset or update a head.

The blocking `run` owner keeps its child alive as an owned resource until cleanup.
`run_async` uses a blocking executor and requires persistent log paths. Dropping
or aborting its future requests cancellation; the owner continues to TERM, wait
its grace, KILL, reap and flush logs. An attached worker permit remains owned until
cleanup finishes, bounding admitted concurrency; no detached child handle is returned. Explicit cancellation
is idempotent. Synchronous discovery shares this owner, retains logs in failure
evidence and keeps import output separate from CLI stdout/stderr. Public log
inspection and durable attempt/event composition remain later tasks.

Cleanup rechecks the captured OS start record before each group signal, refuses reaped
handles and missing/mismatched identities, and holds the direct child waitable with `waitid(WNOWAIT)` until the final
process-group signal. Its PID therefore cannot be reused during cleanup. The
child is reaped only afterwards; no saved PID is signalled later. Darwin can
return EPERM for a zombie-only group: the macOS path checks the pinned group via
bounded `/bin/ps` output and only accepts that error when every member is a zombie.
Other permission errors remain failures. Early grace completion additionally
requires bounded OS evidence that the group has no live members; closed log
pipes alone never authorize an early KILL. If that inspection is unavailable,
the full grace is retained. See Apple's
[killpg1 implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c).
The direct child also has a kill fallback. Descendants in the group receive
termination; orphan descendant reaping belongs to the OS. Trusted code can escape
the group, and inherited descriptors then have a bounded final drain with
`logs_complete=false`. No hostile containment or restart PID recovery is claimed;
[build cancellation and publication races](CANCELLATION.md) are now composed by T057.
Persisted restart process reconciliation remains T066. A waiting persistent client
must observe coordinator-owned work independently; dropping the supervisor future
is coordinator abandonment, not a normal client disconnect.

Native tests cover simultaneous multi-megabyte streams, crash/startup errors,
wrong nonce/session/version/phase, malformed/oversized/partial frames, missing or
repeated results, post-terminal traffic, deadlines, heartbeat silence, split
secret/header redaction, private paths, TERM-resistant descendants and dropped
async callers. Installed-worker tests cover mutual authentication before imports
and coordinator disconnect during a slow import. CI runs the same suite on the
supported macOS/Linux matrix; local evidence records only platforms actually run.

T056 reports typed phase timeout evidence, the actual pre-import thread limit and
cleanup duration. Phase timers exclude cleanup/grace. POLARS, OpenMP and common
BLAS/NumExpr/Accelerate environment limits are set before Python starts. DuckDB
connection thread/buffer/spill configuration remains the adapter responsibility.

T058 adds the `polars.execute.v1` capability and [Polars execution service](../../python/POLARS.md).
Execute controls carry artifact-ready evidence; successful worker exit is followed
by independent Rust candidate validation and never directly advances a head.
