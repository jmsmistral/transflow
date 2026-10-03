//! Exact virtual-clock cron, DST and bounded misfire expectations.
#![allow(
    clippy::unwrap_used,
    reason = "Literal synthetic times and successful fixtures"
)]
use chrono::DateTime;
use tf_domain::schedule_clock::{Cursor, Evaluation, Leaf, Misfire};
use tf_schedule::{Clock, Error, prepare, preview};
struct Virtual(i64);
impl Clock for Virtual {
    fn now_us(&self) -> Result<i64, Error> {
        Ok(self.0)
    }
}
fn at(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s).unwrap().timestamp_micros()
}
fn leaf(e: &str, z: &str, both: bool) -> Leaf {
    Leaf {
        id: "clock".into(),
        expression: e.into(),
        timezone: z.into(),
        duplicate_time: if both { "both" } else { "earliest" }.into(),
    }
}
fn request(t: i64, p: Misfire) -> Evaluation {
    Evaluation {
        cursor: Cursor { at_us: t, leaf: 63 },
        paused: false,
        ignored_intervals: vec![],
        misfire: p,
        max_catch_up: 100,
        pending: 0,
    }
}
#[test]
fn five_fields_steps_lists_ranges_and_exact_five_preview() {
    let result = preview(
        &leaf("5,15-25/5 8-9 * * 1-5", "Asia/Kathmandu", false),
        at("2026-10-02T02:00:00Z"),
    )
    .unwrap();
    assert_eq!(result.len(), 5);
    assert_eq!(
        result.iter().map(|f| f.utc.as_str()).collect::<Vec<_>>(),
        vec![
            "2026-10-02T02:20:00+00:00",
            "2026-10-02T02:30:00+00:00",
            "2026-10-02T02:35:00+00:00",
            "2026-10-02T02:40:00+00:00",
            "2026-10-02T03:20:00+00:00"
        ]
    );
    assert_eq!(result[0].local, "2026-10-02T08:05:00+05:45");
    assert!(!result[0].tzdb_version.is_empty());
}
#[test]
fn spring_gap_is_skipped_and_fall_fold_defaults_to_earliest() {
    let gap = preview(
        &leaf("30 2 * * *", "America/New_York", false),
        at("2026-03-07T08:00:00Z"),
    )
    .unwrap();
    assert_eq!(gap[0].utc, "2026-03-09T06:30:00+00:00");
    let fold = preview(
        &leaf("30 1 * * *", "America/New_York", false),
        at("2026-11-01T00:00:00Z"),
    )
    .unwrap();
    assert_eq!(fold[0].utc, "2026-11-01T05:30:00+00:00");
    assert_eq!(fold[1].utc, "2026-11-02T06:30:00+00:00");
    // Starting between the two fold instants must not emit the later instant by default.
    let after = preview(
        &leaf("30 1 * * *", "America/New_York", false),
        at("2026-11-01T05:45:00Z"),
    )
    .unwrap();
    assert_eq!(after[0], fold[1]);
}
#[test]
fn both_fold_instants_have_distinct_utc_keys_and_offset_labels() {
    let fold = preview(
        &leaf("30 1 * * *", "America/New_York", true),
        at("2026-11-01T00:00:00Z"),
    )
    .unwrap();
    assert_eq!(fold[0].local, "2026-11-01T01:30:00-04:00");
    assert_eq!(fold[1].local, "2026-11-01T01:30:00-05:00");
    assert_eq!(fold[1].at_us - fold[0].at_us, 3_600_000_000);
}
#[test]
fn half_hour_dst_and_missing_calendar_day_are_resolved_by_iana() {
    let fold = preview(
        &leaf("45 1 * * *", "Australia/Lord_Howe", true),
        at("2026-04-04T00:00:00Z"),
    )
    .unwrap();
    assert_eq!(fold[1].at_us - fold[0].at_us, 1_800_000_000);
    let skip = preview(
        &leaf("0 12 * * *", "Pacific/Apia", false),
        at("2011-12-29T23:00:00Z"),
    )
    .unwrap();
    assert!(skip.iter().all(|f| !f.local.starts_with("2011-12-30")));
    assert!(skip[0].local.starts_with("2011-12-31"));
}
#[test]
fn restricted_month_day_and_weekday_use_or_and_sunday_accepts_zero_or_seven() {
    let fires = preview(
        &leaf("0 0 31 2 1", "UTC", false),
        at("2026-02-01T00:00:00Z"),
    )
    .unwrap();
    assert_eq!(fires[0].utc, "2026-02-02T00:00:00+00:00");
    assert_eq!(
        preview(&leaf("0 0 * * 0", "UTC", false), at("2026-10-03T00:00:00Z")).unwrap(),
        preview(&leaf("0 0 * * 7", "UTC", false), at("2026-10-03T00:00:00Z")).unwrap()
    );
    let leap = preview(
        &leaf("0 0 29 2 *", "UTC", false),
        at("2025-01-01T00:00:00Z"),
    )
    .unwrap();
    assert_eq!(leap[0].utc, "2028-02-29T00:00:00+00:00");
    assert_eq!(leap[4].utc, "2044-02-29T00:00:00+00:00");
}
#[test]
fn skip_coalesce_catchup_and_paused_counts_are_explicit() {
    let start = at("2026-10-03T00:00:00Z");
    let end = start + 5 * 60_000_000 + 30_000_000;
    let leaves = [leaf("* * * * *", "UTC", false)];
    let skip = prepare(&leaves, &request(start, Misfire::Skip), &Virtual(end)).unwrap();
    assert_eq!(
        (skip.matched, skip.missed, skip.ignored, skip.ticks.len()),
        (5, 4, 4, 1)
    );
    let latest = prepare(
        &leaves,
        &request(start, Misfire::CoalesceLatest),
        &Virtual(end),
    )
    .unwrap();
    assert_eq!((latest.coalesced, latest.ticks.len()), (4, 1));
    assert_eq!(latest.ticks, skip.ticks);
    let mut req = request(start, Misfire::CatchUp);
    req.max_catch_up = 2;
    let catch = prepare(&leaves, &req, &Virtual(end)).unwrap();
    assert_eq!((catch.matched, catch.missed, catch.ticks.len()), (2, 2, 2));
    assert!(catch.more);
    req.paused = true;
    let pause = prepare(&leaves, &req, &Virtual(end)).unwrap();
    assert_eq!((pause.matched, pause.ignored, pause.ticks.len()), (5, 5, 0));
    assert_eq!(pause.cursor.at_us, end);
}
#[test]
fn compound_cursor_never_loses_simultaneous_ticks_under_catchup_limit() {
    let start = at("2026-10-03T00:00:00Z");
    let mut leaves = vec![leaf("* * * * *", "UTC", false); 2];
    leaves[1].id = "second".into();
    let mut req = request(start, Misfire::CatchUp);
    req.max_catch_up = 1;
    let first = prepare(&leaves, &req, &Virtual(start + 60_000_000)).unwrap();
    assert_eq!(
        first.ticks[0].key,
        Cursor {
            at_us: start + 60_000_000,
            leaf: 0
        }
    );
    assert!(first.more);
    req.cursor = first.cursor;
    let second = prepare(&leaves, &req, &Virtual(start + 60_000_000)).unwrap();
    assert_eq!(
        second.ticks[0].key,
        Cursor {
            at_us: start + 60_000_000,
            leaf: 1
        }
    );
    assert!(!second.more);
}
#[test]
fn backward_jumps_and_full_pending_capacity_preserve_the_cursor() {
    let t = at("2026-10-03T00:00:00Z");
    let leaves = [leaf("* * * * *", "UTC", false)];
    let mut req = request(t, Misfire::CatchUp);
    let back = prepare(&leaves, &req, &Virtual(t - 60_000_000)).unwrap();
    assert_eq!(back.cursor, req.cursor);
    assert!(back.ticks.is_empty());
    req.pending = 100;
    let full = prepare(&leaves, &req, &Virtual(t + 60_000_000)).unwrap();
    assert_eq!(full.cursor, req.cursor);
    assert!(full.more);
    assert_eq!(full.matched, 0);
}
#[test]
fn long_sleep_is_paged_without_hidden_loss_and_dense_catchup_caps_at_one_hundred() {
    let start = at("2026-10-01T00:00:00Z");
    let end = start + 3 * 86_400_000_000;
    let leaves = [leaf("* * * * *", "UTC", false)];
    let req = request(start, Misfire::Skip);
    let page = prepare(&leaves, &req, &Virtual(end)).unwrap();
    assert!(page.more);
    assert_eq!(page.matched, 1440);
    assert_eq!(page.ignored, 1440);
    let dense = prepare(&leaves, &request(start, Misfire::CatchUp), &Virtual(end)).unwrap();
    assert_eq!(dense.ticks.len(), 100);
    assert!(dense.more);
}
#[test]
fn invalid_syntax_zones_policies_and_impossible_previews_fail_closed() {
    for (expr, zone) in [
        ("0 0 0 * *", "UTC"),
        ("0 24 * * *", "UTC"),
        ("*/0 * * * *", "UTC"),
        ("0 0 * * * *", "UTC"),
        ("@daily", "UTC"),
        ("0 0 * * *", "Mars/Olympus"),
    ] {
        assert_eq!(preview(&leaf(expr, zone, false), 0), Err(Error::Definition));
    }
    assert_eq!(
        preview(&leaf("0 0 31 2 *", "UTC", false), 0),
        Err(Error::Limit)
    );
    assert_eq!(
        preview(&leaf("* * * * *", "UTC", false), -1),
        Err(Error::Limit)
    );
    let mut bad = leaf("* * * * *", "UTC", false);
    bad.duplicate_time = "latest".into();
    assert_eq!(preview(&bad, 0), Err(Error::Definition));
    let mut req = request(0, Misfire::Skip);
    req.max_catch_up = 101;
    assert!(prepare(&[], &req, &Virtual(0)).is_err());
}
#[test]
fn paused_windows_do_not_replay_after_resume_and_catchup_prefix_preserves_counts() {
    let start = at("2026-10-03T00:00:00Z");
    let mut req = request(start, Misfire::CatchUp);
    req.ignored_intervals = vec![(start, start + 2 * 60_000_000)];
    req.max_catch_up = 1;
    let first = prepare(
        &[leaf("* * * * *", "UTC", false)],
        &req,
        &Virtual(start + 5 * 60_000_000),
    )
    .unwrap();
    assert_eq!((first.matched, first.ignored, first.ticks.len()), (3, 2, 1));
    assert_eq!(first.cursor.at_us, start + 3 * 60_000_000);
    assert!(first.more);
    req.cursor = first.cursor;
    let second = prepare(
        &[leaf("* * * * *", "UTC", false)],
        &req,
        &Virtual(start + 5 * 60_000_000),
    )
    .unwrap();
    assert_eq!(
        (second.matched, second.ignored, second.ticks.len()),
        (1, 0, 1)
    );
    assert_eq!(second.cursor.at_us, start + 4 * 60_000_000);
}
