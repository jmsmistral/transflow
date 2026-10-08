# Schedules

Keep `transflow --workspace /path/to/workspace serve` running for automatic
observation and dispatch. No background service is installed. The lineage
**Schedules** inspector is the simplest way to create and edit definitions.
It saves one current snapshot; Save replaces it under an edit guard. Accepted
occurrences retain their source, branches, input policy, pins and execution options.

A build contains one or more dataset jobs. An occurrence is an accepted scheduling
request and may instead be queued, held, coalesced, skipped or canceled without a
build. History keeps those distinctions.

## CLI

```bash
transflow --workspace /path/to/workspace schedule list
transflow --workspace /path/to/workspace schedule show <schedule-id>
transflow --workspace /path/to/workspace schedule export <schedule-id> --path schedule.json
transflow --workspace /path/to/workspace schedule create --file schedule.json --paused
transflow --workspace /path/to/workspace schedule update <schedule-id> --file schedule.json --if-match <etag>
transflow --workspace /path/to/workspace schedule run <schedule-id>
transflow --workspace /path/to/workspace schedule pause <schedule-id>
transflow --workspace /path/to/workspace schedule resume <schedule-id>
transflow --workspace /path/to/workspace schedule history <schedule-id> --limit 50
transflow --workspace /path/to/workspace schedule metrics <schedule-id> --from-us 0 --to-us <end-microseconds>
transflow --workspace /path/to/workspace schedule delete <schedule-id>
transflow --workspace /path/to/workspace schedule delete <schedule-id> --yes
```

All commands support the root `--json` envelope. Read commands work without a
coordinator. Mutations use the same guarded services as the API; manual Run requires
`serve` and leaves a paused schedule paused. Delete previews by default; `--yes`
tombstones the definition and cancels unstarted automatic work. Manual requests,
active builds and retained history remain. Export writes a new private file and
refuses to overwrite an existing path. Reimport is explicit and validates the complete
[definition schema](../../schemas/contracts-v1.schema.json); files are not watched.
Pause/resume/run/delete accept an optional `--if-match`; otherwise the CLI reads
and uses the current guard. Update requires it. Resume/update can request an explicit
bounded replay using both `--replay-after-event <event-id>` and `--replay-limit <1..100>`.

## Source, branches and automatic conditions

Source selection is independent of the output data branch. Git defaults to a named
clean ref; non-Git defaults to a retained validated source snapshot. Mutable working
trees and additive registration are explicit choices. The selected output branch,
ordered fallback branches, named input policies and provider policies are saved;
they do not follow later foreground branch or policy changes.

Conditions support nested AND/OR, numeric five-field cron with IANA timezones,
dataset publication/head changes, successful builds and successful schedules.
Time previews disclose offsets and bundled timezone rules. Publication conditions
can pin the exact event version or act only as signals. Event expiry, missed tick
policy, bounded catch-up and automatic burst limits are explicit.

Same-schedule overlap is disabled by default. Compatible pending occurrences can
coalesce to the latest evidence; distinct frozen templates are not combined. Bounded
queues and persisted skip reasons are alternatives. Disjoint builds run concurrently
under coordinator admission limits; conflicting dataset/branch writes wait for
reservations. Static cycles are rejected where authored target/trigger membership
proves them; runtime causation ancestry, hop and burst limits protect remaining cases.
Force never bypasses write reservations or input/quality checks. Retries are opt-in
infrastructure classes; authentication, protocol, integrity and quality refusals
remain nonretryable.

## History and metrics

History is scoped to one schedule, newest first, with explicit pagination. It includes
accepted, queued, held, skipped, coalesced, failed and canceled dispositions, optional
build links, job/attempt counts and missing timing evidence. Tombstones keep it readable.
Metrics use an explicitly half-open occurrence-ready time cohort, at most 1,000
occurrences and 10,000 jobs/attempts. Successful build **operation** durations include
cache reuse; they are distinct from non-cached job execution measurements. Missing
values are not zero. Medians and trailing means retain exact rational values; the
failure rate denominator is succeeded plus failed accepted builds. Ignored event
matches use observation time; clock dispositions use intended tick time. Those
separate cohorts are labelled.

The UI shows inclusive From/To calendar dates, two decimal duration display, and
separate counts for occurrences, builds, jobs, attempts and timings. Charts display
only the selected history page and disclose that basis. Build reports automatically
update while active. Informational scope previews use the shared planner and cannot
be accepted as ad-hoc builds; use Run now on the saved schedule.
