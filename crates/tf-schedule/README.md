# Cron and clock planning — T093

This crate resolves typed clock leaves into intended UTC ticks and read-only
next-five previews. It activates no daemon, creates no executing job and does not
read source files or providers. Operational source resolution/dispatch is T095;
schedule lifecycle and editor integration are T094/T098.

## Resolution

`preview(leaf, after_us)` returns exactly five future `Fire` values, with intended
UTC microseconds, ISO UTC/local timestamps, explicit local offsets, the authored
zone and bundled database version. It excludes the `after_us` instant. Unknown
IANA names, invalid expressions, impossible dates and exhausted bounds fail
explicitly. The preview foundation is available to the later editor before save;
T090's definition codec currently performs only structural timezone validation.

Cron accepts exactly five numeric fields: minute, hour, day of month, month and
weekday. Wildcards, lists, inclusive ascending ranges and steps are supported;
a numeric start with a step runs through the field maximum. Weekday 0 and 7 mean
Sunday. If both day-of-month and weekday are restricted, either may match;
otherwise both field predicates apply. Names, macros, seconds and a year field
are unsupported. Calendar validity is resolved using the actual local date.

A nonexistent local minute is skipped. Duplicated minutes use the earliest UTC
instant by default; `duplicate_time=both` returns both, in UTC order. This also
covers non-hour changes and removed local calendar dates. Resolution uses the
already qualified Chrono 0.4.45 and Chrono-TZ 0.10.4 pins with bundled IANA 2025b,
independently of host timezone files. See [Chrono timezone resolution](https://docs.rs/chrono/0.4.45/chrono/trait.TimeZone.html)
and [Chrono-TZ](https://docs.rs/chrono-tz/0.10.4/chrono_tz/).

## Observation and persistence

`Clock` is injectable; `SystemClock` reads wall time once. `prepare(leaves,
evaluation, clock)` resolves a frozen request outside the storage transaction.
Call it on a bounded blocking worker when used by an async coordinator. Persist
its result through [tf-store clock acceptance](../tf-store/README.md#durable-schedule-clocks-t093),
then deliver tokens and invoke T092 evaluation. For `coalesce_latest`, drain
`more` against the same frozen observation before delivering, so day-sized scan
pages do not become separate missed-tick occurrences. These are explicit primitives;
there is no automatic persistent observation loop in this delivery.

The cursor is an exclusive `(UTC microseconds, authored cron-leaf order)` key.
It starts at definition save time and never moves backward. Resolution scans at
most one UTC day per pass and reports `more` when another pass is needed. It uses
at most 500,000 local candidate resolutions and 100,000 due ticks; trees have at
most 64 unique clock leaves. Preview searches at most 32 calendar years and
100,000 matching local candidates. Supported observations are Unix epoch through
year 9999. No limit silently truncates evidence.

Ticks less than one minute behind the frozen observation are on time. Older ones
are missed. `skip` keeps only on-time ticks; `coalesce_latest` keeps the latest
intended tick per leaf; `catch_up` keeps the oldest ticks, up to the saved limit
(1–100) and available pending capacity. It advances only through those accepted
keys, leaving remaining ticks for a later pass. At most 100 intended ticks are
pending per schedule/epoch. Catch-up token delivery waits for earlier evidence
from the same leaf to be consumed; it does not collapse distinct missed ticks into
one occurrence. Compound AND/OR rules still determine whether each token can
satisfy the full trigger.

Paused observation records/ignores all ticks in its advanced interval. Matched,
missed, ignored and coalesced counts persist with the cursor and audit. Token expiry
uses intended tick time, so sufficiently old catch-up ticks can expire and are
then explicitly counted/ignored. Current epoch/ETag/cursor/pause guards prevent
stale preparation from accepting evidence after an edit or competing observation.
No daemon means no scheduled execution; restart/wake behavior becomes automatic
only when the later persistent coordinator integration invokes these services.
