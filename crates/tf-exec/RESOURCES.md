# Resource admission and phase budgets (T056)

`Admission` is one shared pool per coordinator. Clone the pool for jobs; creating
one pool per job defeats workspace limits. `request(Demand)` reserves one job slot,
CPU tokens and optional estimated memory/disk atomically. The FIFO queue is bounded
(default 1024); impossible requests fail before enqueueing. A large head request
can block smaller followers. Dropping a queued future removes it and wakes others.
No mutex or database transaction spans worker execution.

Keep the resulting `Reservation` for the whole job. Check out `worker()` and move
that exclusive `WorkerPermit` into `supervisor::Launch`. After transform cleanup,
validation helpers check out the same reservation: they do not acquire another
job slot. Two simultaneous workers cannot spend one reservation. Dropping the
caller does not release running capacity: the supervisor retains the permit until
TERM/grace/KILL/reap and log draining finish. Queue cancellation and capacity
release wake pending futures. T064 now composes runtime dispatch through the [accepted-plan service](../transflow/DISPATCH.md); restart recovery remains T066.

Default capacities come from resolved workspace max_jobs (2) and CPU tokens
(auto uses available logical CPUs). Default worker threads divide tokens across
available job slots, at least one thread each. Explicit memory budgets require
job estimates, and absent budgets impose no Transflow memory gate or ceiling.
Disk reservations are an optional service API, not a filesystem quota or new
workspace key. Estimates never enforce RSS. Hard memory requests fail explicitly;
engine/OS defaults remain separate. Checked MiB conversion refuses overflow.

`Budget` is an attempt-wide monotonic ledger, created unstarted before admission.
Clones share it. Transform setup, invocation and materialization accumulate against
one 3600-second default, pausing while input/output checks use their own independent
3600-second budgets. Every query/helper in a validation phase shares its original
budget; a new query never renews it. Interactive work has a separate 30-second
budget. Discovery is labelled separately and derives its default from workspace
execution policy before producer declarations are available. There is no hidden
300-second discovery timeout or overall deadline layered over phase limits.
Zero disables just that timer; explicit and dropped-future cancellation still work.
Queue wait and worker cleanup/grace do not consume the phase budget. Time between
queries in the same active phase does count. Finish the ledger when the attempt
ends; it then rejects reuse. Callers must serialize work using the reservation.

The supervisor enters the coordinator-selected phase and follows validated worker
phase messages. Evaluate-checks helpers cannot switch from their assigned input
budget to the output budget. It checks expiry between control/log ticks and before
accepting phase transitions or success evidence. A timeout fails the operation,
retains both bounded logs and reports phase, subject/check, elapsed seconds,
effective limit, winning origin, configuration key and override guidance. A WARN
check cannot turn this supervisor failure into a passing result. The coordinator
updates check context without resetting time; adapters/evaluator composition remain
T058/T061/T064. Startup authentication retains its separate ten-second transport
liveness bound, and terminal-process shutdown retains its bounded grace.

Before Python starts, the supervisor sets POLARS_MAX_THREADS, OMP_NUM_THREADS,
OPENBLAS_NUM_THREADS, MKL_NUM_THREADS, NUMEXPR_MAX_THREADS and
VECLIB_MAXIMUM_THREADS from the reservation. Standalone discovery receives the
same resolved default thread count. SQL connection thread settings and engine
buffer/spill policies must be applied by future adapters; no engine hard memory
limit is claimed here.

`transflow::resources::Resolved` maps configuration to these services and the
public plan's exact resource carriers/provenance. Producer declarations override
the workspace compute timeout, then build policy and explicit CLI/API settings
win. Validation uses workspace/build/explicit policy; interactive and discovery
remain separately resolved. Public discovery runs before a producer/build exists,
so its timer comes from workspace execution settings, including explicit zero.
Read-only preparation does not acquire the runtime writer or producer pool.

Tests combine injected hour-scale clocks with real managed subprocesses: a check
survives 301 seconds of simulated time, successive helpers reach 3599 seconds,
and the shared budget fails at 3600 with retained logs. Other tests cover
phase separation, optional-memory semantics, atomic FIFO reservations, queue
cancellation/wakeup, CPU limits, disabled-deadline cancellation, cleanup exclusion,
and reservation ownership after async caller abandonment. Full persisted attempt
identity, publication races and recovery remain T057/T066.
