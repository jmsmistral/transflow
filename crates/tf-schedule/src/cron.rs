//! Five-field numeric cron over bundled IANA rules, with explicit gap/fold behavior.
use chrono::{DateTime, Datelike, Days, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use std::time::{SystemTime, UNIX_EPOCH};
use tf_domain::schedule_clock::{Cursor, Evaluation, Leaf, Misfire, Plan, Tick};
const MINUTE: i64 = 60_000_000;
const DAY: i64 = 86_400_000_000;
const LAST: i64 = 253_402_300_799_999_999; // End of UTC year 9999.
/// Explicit failure, never an invented next time or silently truncated observation.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum Error {
    /// Unsupported cron field, timezone or duplicate-time policy.
    #[error("The schedule needs five valid numeric cron fields and a known IANA timezone")]
    Definition,
    /// Invalid time, exhausted preview horizon or work budget.
    #[error("The clock request exceeds its supported time range or resolution budget")]
    Limit,
}
/// Injectable wall clock. Every pass freezes one observation.
pub trait Clock {
    /// UTC microseconds since Unix epoch.
    fn now_us(&self) -> Result<i64, Error>;
}
/// Production wall clock; observing it does not start persistent scheduling.
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_us(&self) -> Result<i64, Error> {
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| Error::Limit)?
                .as_micros(),
        )
        .map_err(|_| Error::Limit)
    }
}
/// Resolved preview includes explicit offsets to distinguish duplicated local times.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fire {
    /// Intended UTC microseconds, suitable for stable tick identity.
    pub at_us: i64,
    /// ISO timestamp in UTC.
    pub utc: String,
    /// ISO timestamp in the authored timezone, including its resolved offset.
    pub local: String,
    /// Recorded IANA zone name.
    pub timezone: String,
    /// Bundled timezone database version.
    pub tzdb_version: String,
}
struct Field {
    bits: u64,
    wildcard: bool,
}
impl Field {
    fn parse(s: &str, low: u32, high: u32, sunday: bool) -> Result<Self, Error> {
        let mut bits = 0;
        for part in s.split(',') {
            let (range, step) = part
                .split_once('/')
                .map_or((part, 1), |(r, n)| (r, n.parse().unwrap_or(0)));
            if step == 0 || step > high - low + 1 {
                return Err(Error::Definition);
            }
            let (a, b) = if range == "*" {
                (low, high)
            } else if let Some((a, b)) = range.split_once('-') {
                (
                    a.parse().map_err(|_| Error::Definition)?,
                    b.parse().map_err(|_| Error::Definition)?,
                )
            } else {
                let a = range.parse().map_err(|_| Error::Definition)?;
                (a, if part.contains('/') { high } else { a })
            };
            if a < low || b > high || a > b {
                return Err(Error::Definition);
            }
            for n in (a..=b).step_by(step as usize) {
                bits |= 1 << if sunday && n == 7 { 0 } else { n };
            }
        }
        if bits == 0 {
            return Err(Error::Definition);
        }
        Ok(Self {
            bits,
            wildcard: s == "*",
        })
    }
    fn has(&self, n: u32) -> bool {
        self.bits & (1 << n) != 0
    }
}
struct Cron {
    fields: [Field; 5],
    tz: Tz,
    both: bool,
}
impl Cron {
    fn parse(leaf: &Leaf) -> Result<Self, Error> {
        if leaf.id.is_empty()
            || leaf.id.len() > 256
            || leaf.id.chars().any(char::is_control)
            || leaf.expression.len() > 256
            || !matches!(leaf.duplicate_time.as_str(), "earliest" | "both")
        {
            return Err(Error::Definition);
        }
        let f: Vec<_> = leaf.expression.split_whitespace().collect();
        if f.len() != 5 {
            return Err(Error::Definition);
        }
        Ok(Self {
            fields: [
                Field::parse(f[0], 0, 59, false)?,
                Field::parse(f[1], 0, 23, false)?,
                Field::parse(f[2], 1, 31, false)?,
                Field::parse(f[3], 1, 12, false)?,
                Field::parse(f[4], 0, 7, true)?,
            ],
            tz: leaf.timezone.parse().map_err(|_| Error::Definition)?,
            both: leaf.duplicate_time == "both",
        })
    }
    fn date(&self, d: NaiveDate) -> bool {
        let dom = self.fields[2].has(d.day());
        let dow = self.fields[4].has(d.weekday().num_days_from_sunday());
        self.fields[3].has(d.month())
            && if !self.fields[2].wildcard && !self.fields[4].wildcard {
                dom || dow
            } else {
                dom && dow
            }
    }
    fn day(&self, date: NaiveDate, budget: &mut usize) -> Result<Vec<i64>, Error> {
        let mut result = vec![];
        if !self.date(date) {
            return Ok(result);
        }
        for hour in 0..24 {
            if !self.fields[1].has(hour) {
                continue;
            }
            for minute in 0..60 {
                if !self.fields[0].has(minute) {
                    continue;
                }
                *budget = budget.checked_sub(1).ok_or(Error::Limit)?;
                let local = date.and_hms_opt(hour, minute, 0).ok_or(Error::Limit)?;
                match self.tz.from_local_datetime(&local) {
                    LocalResult::None => {} // Nonexistent spring-forward minute.
                    LocalResult::Single(t) => result.push(t.timestamp_micros()),
                    LocalResult::Ambiguous(a, b) => {
                        result.push(a.timestamp_micros().min(b.timestamp_micros()));
                        if self.both {
                            result.push(a.timestamp_micros().max(b.timestamp_micros()));
                        }
                    }
                }
            }
        }
        result.sort_unstable();
        Ok(result)
    }
}
fn instant(n: i64) -> Result<DateTime<Utc>, Error> {
    if !(0..=LAST).contains(&n) {
        return Err(Error::Limit);
    }
    DateTime::from_timestamp_micros(n).ok_or(Error::Limit)
}
/// Validate and preview exactly the next five UTC/local fire times, strictly after `after_us`.
/// Search is bounded to 32 calendar years; impossible dates fail explicitly.
pub fn preview(leaf: &Leaf, after_us: i64) -> Result<Vec<Fire>, Error> {
    let c = Cron::parse(leaf)?;
    let after = instant(after_us)?;
    let mut date = after
        .with_timezone(&c.tz)
        .date_naive()
        .checked_sub_days(Days::new(1))
        .ok_or(Error::Limit)?;
    let mut fires = vec![];
    let mut budget = 100_000;
    for _ in 0..11_713 {
        for at in c.day(date, &mut budget)? {
            if at > after_us && at <= LAST {
                fires.push(at);
            }
        }
        fires.sort_unstable();
        fires.truncate(5);
        // Finish the following local date before stopping: large backward transitions
        // can map the next local date before the last candidate's UTC instant.
        if fires.len() == 5 && date > instant(fires[4])?.with_timezone(&c.tz).date_naive() {
            break;
        }
        date = date.succ_opt().ok_or(Error::Limit)?;
    }
    if fires.len() != 5 {
        return Err(Error::Limit);
    }
    fires
        .into_iter()
        .map(|at| {
            let t = instant(at)?;
            Ok(Fire {
                at_us: at,
                utc: t.to_rfc3339(),
                local: t.with_timezone(&c.tz).to_rfc3339(),
                timezone: leaf.timezone.clone(),
                tzdb_version: chrono_tz::IANA_TZDB_VERSION.into(),
            })
        })
        .collect()
}
/// Resolve a frozen observation outside storage ownership. At most one UTC day is
/// scanned per pass; `more` explicitly requests another pass. Catch-up preserves
/// a compound (UTC time, authored cron leaf order) cursor and at most 100 pending ticks.
pub fn prepare(leaves: &[Leaf], request: &Evaluation, clock: &dyn Clock) -> Result<Plan, Error> {
    let now = clock.now_us()?;
    instant(now)?;
    instant(request.cursor.at_us)?;
    if leaves.len() > 64
        || request.cursor.leaf > 63
        || !(1..=100).contains(&request.max_catch_up)
        || request.pending > 100
        || request.ignored_intervals.len() > 100
        || request
            .ignored_intervals
            .iter()
            .any(|(a, b)| *a < 0 || b < a)
    {
        return Err(Error::Limit);
    }
    let mut ids = std::collections::BTreeSet::new();
    let crons = leaves
        .iter()
        .map(|leaf| {
            if !ids.insert(&leaf.id) {
                return Err(Error::Definition);
            }
            Cron::parse(leaf)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut plan = Plan {
        cursor: request.cursor,
        observed_at_us: now,
        ticks: vec![],
        matched: 0,
        missed: 0,
        ignored: 0,
        coalesced: 0,
        more: false,
        tzdb_version: chrono_tz::IANA_TZDB_VERSION.into(),
    };
    if now < request.cursor.at_us {
        return Ok(plan);
    }
    let through = now.min(request.cursor.at_us.saturating_add(DAY));
    let end = Cursor {
        at_us: through,
        leaf: 63,
    };
    let mut due = vec![];
    let mut budget = 500_000;
    for (order, c) in crons.iter().enumerate() {
        let mut date = instant(request.cursor.at_us)?
            .with_timezone(&c.tz)
            .date_naive()
            .checked_sub_days(Days::new(1))
            .ok_or(Error::Limit)?;
        let last = instant(through)?
            .with_timezone(&c.tz)
            .date_naive()
            .checked_add_days(Days::new(1))
            .ok_or(Error::Limit)?;
        while date <= last {
            for at in c.day(date, &mut budget)? {
                let key = Cursor {
                    at_us: at,
                    leaf: order as u8,
                };
                if key > request.cursor && key <= end {
                    due.push(Tick { key });
                }
            }
            date = date.succ_opt().ok_or(Error::Limit)?;
        }
    }
    due.sort_unstable();
    if due.len() > 100_000 {
        return Err(Error::Limit);
    }
    let ignored = |tick: &&Tick| {
        request
            .ignored_intervals
            .iter()
            .any(|(a, b)| tick.key.at_us > *a && tick.key.at_us <= *b)
    };
    let eligible: Vec<_> = due.iter().filter(|t| !ignored(t)).copied().collect();
    let on_time = |tick: &&Tick| tick.key.at_us > now.saturating_sub(MINUTE);
    if request.paused {
        plan.ignored = due.len() as u64;
    } else {
        plan.ignored = (due.len() - eligible.len()) as u64;
        match request.misfire {
            Misfire::Skip => {
                plan.ticks = eligible.iter().filter(on_time).copied().collect();
                plan.ignored = (due.len() - plan.ticks.len()) as u64;
            }
            Misfire::CoalesceLatest => {
                let mut latest = std::collections::BTreeMap::new();
                for t in &eligible {
                    latest.insert(t.key.leaf, *t);
                }
                plan.ticks = latest.into_values().collect();
                plan.ticks.sort_unstable();
                plan.coalesced = (eligible.len() - plan.ticks.len()) as u64;
            }
            Misfire::CatchUp => {
                let limit = request.max_catch_up.min(100 - request.pending) as usize;
                plan.ticks = eligible.iter().take(limit).copied().collect();
                if eligible.len() > limit {
                    let first_unaccepted = eligible[limit].key;
                    let accounted: Vec<_> = due
                        .iter()
                        .take_while(|t| t.key < first_unaccepted)
                        .collect();
                    plan.more = true;
                    if let Some(last) = accounted.last() {
                        plan.cursor = last.key;
                    }
                    plan.matched = accounted.len() as u64;
                    plan.missed = accounted
                        .iter()
                        .filter(|t| t.key.at_us <= now.saturating_sub(MINUTE))
                        .count() as u64;
                    plan.ignored = (accounted.len() - plan.ticks.len()) as u64;
                    return Ok(plan);
                }
            }
        }
    }
    if plan.ticks.len() > 100 {
        return Err(Error::Limit);
    }
    plan.matched = due.len() as u64;
    plan.missed = due
        .iter()
        .filter(|t| t.key.at_us <= now.saturating_sub(MINUTE))
        .count() as u64;
    plan.cursor = end;
    plan.more = through < now;
    Ok(plan)
}
