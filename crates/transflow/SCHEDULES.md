# Schedules

Keep `transflow --workspace /path/to/workspace serve` running for automatic
observation and dispatch. No background service is installed. The lineage
**Schedules** inspector is the simplest way to create and edit definitions.
It saves one current snapshot; Save replaces it under an edit guard. Accepted
occurrences retain their source, branches, input policy, pins and execution options.

A build contains one or more dataset jobs. An occurrence is an accepted scheduling
request and may instead be queued, held, coalesced, skipped or canceled without a
build. History keeps those distinctions.

## Create and edit in lineage

Choose **New schedule**, enter a name and optional description, then select the
datasets to build. **When to build** uses expandable condition cards. Add a
condition and choose **All (AND)** or **Any (OR)** to combine triggers; conditions
can also contain nested groups. Switching AND/OR keeps the existing conditions.
AND waits for evidence from every condition. Separate OR matches can start
separate builds; OR does not combine them into one event.

The data branch and ordered fallbacks are separate from trigger event branches.
The first row shows the data branch, followed by fallbacks in search order.
**Connecting build** attempts to build datasets between trigger and target nodes.

**Advanced options** groups build scope, failures/retries, overlapping runs,
timing/limits, input datasets and custom input/parameter overrides. Each group
expands separately. Retry classes appear when more than one attempt is allowed;
catch-up limits appear with catch-up enabled. Time values accept seconds, minutes
or hours. Keeping events indefinitely requires explicitly enabling that option.
Upcoming clock times are available under **Next intended times**.

Save and Cancel stay together at the top while scrolling. New names start empty;
placeholders are hints rather than saved values. The editor omits source selectors,
snapshot identifiers, condition IDs and input-binding controls. New definitions
retain the coordinator's source defaults and exact pins for new dataset triggers;
editing preserves existing source selectors, trigger IDs and payload modes.
Inspect those settings in **View schedule**; use the CLI definition workflow below
to change source or payload policy. Accepted runs retain their original settings.

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
Read-only time cards use plain-language descriptions with their IANA timezone;
the editor retains numeric cron and previews with offsets and bundled timezone
rules. Full details explain repeated local times. Publication conditions
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
update while active. Use Run now on the saved schedule to execute its definition.
The sidebar omits a scope-preview button and loads the shared informational plan
automatically when a saved schedule is selected. Datasets to build shows the complete
planned list and count, including connecting jobs outside the visible canvas. Connecting
nodes say Attempt to build. Exact occurrence pins and cache/refresh decisions may reuse
inputs; the preview does not accept or execute a build. Unavailable scope is explicit.

## Dedicated Schedule view

Select a schedule in the lineage inspector, then choose **View schedule**. The large
modal shows its current saved definition, compound conditions, datasets, source/input
policies, branch/fallback, execution options, summary and recent builds. **View build**
opens the corresponding retained Build report above it; closing that report returns
to the Schedule view. The lineage branch, selection and graph geometry are preserved.

**Code source** means the code used by the schedule. A fixed snapshot uses retained
captured code; a Git ref selects its committed code; working-tree selection captures
the current files for each occurrence. This choice is independent of the output data
branch. Snapshot identifiers remain available in the full view. Editor Save and Cancel
stay at the top; saving still overwrites the single current definition under its edit guard.

## Graph-assisted editing

Choose **Edit**, then select datasets on lineage. **Add selected nodes** or **Target**
adds eligible producer targets to a fixed-height, target-only list. The selection
also offers **Trigger** and **Clear**. Trigger adds a publication condition on the
draft data branch, with an exact event pin. Existing AND/OR groups retain their operator;
an existing single condition combines with the new condition using AND. Clear removes
that dataset's target/trigger/boundary/exclusion/refresh membership and collapses empty
or single-child groups. Unrelated condition IDs and policies are retained. Backend
validation still rejects invalid targets, cycles and conflicting edits.

New UI schedules start with selected targets only. Manual/time conditions can
trigger those targets without any dataset trigger. The optional **Connecting
build** switch requires at least one dataset trigger and a distinct target. It
reuses published trigger inputs and builds eligible datasets on their paths to
the targets; other inputs remain read boundaries. Removing a required endpoint
blocks Save until repaired or the switch is turned off. The shared planner uses
the complete captured graph and saved input policies. Existing saved advanced
scopes remain available; this UI option uses Between scope with trigger boundaries
without changing explicit targets, API defaults or accepted execution settings.

Target is green, Trigger amber and Attempt to build blue, matching the legend.
Hover a role label for simultaneous flags. Extra-info icons do not repeat roles.

These are draft changes. **Exit schedule**, **Schedules** and **Cancel** offer Save and
exit, Discard changes or Keep editing when there are unsaved changes. Save overwrites
one definition under its original ETag. The data branch picker searches all live
workspace branches, with complete API pagination and bounded result pages. The mini
fallback editor supports add, drag, up/down and remove without switching the lineage
branch. No source/input-default reset button is shown. Info icons beside detail labels
explain their settings; expiry is displayed in hours, minutes or seconds. Exact pin
wording stays in the full view, while condition cards omit it.


## Live schedule status

Paused uses a pause glyph, Active a green tick, and Active and currently running
blue rotating arrows. The running flag reads all linked RUNNING builds, including
manual work and overlapping occurrences; it does not depend on a history page.
Queued requests are not running. Pause stops automatic scheduling but does not cancel
ongoing or manual work, so the running label separately states when automatic
scheduling is paused. Rotation respects reduced-motion preferences. Code and build
options have their own section below Branches in the sidebar and dedicated view.
