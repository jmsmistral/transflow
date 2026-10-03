//! Clock planning carriers with no timezone, persistence or system-clock dependency.
/// Chronological high-water key. Leaf order handles simultaneous compound ticks.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Cursor {
    /// Original UTC microsecond timestamp.
    pub at_us: i64,
    /// Authored cron-leaf order; 63 means the whole instant has been observed.
    pub leaf: u8,
}
/// Authored clock leaf, resolved by the scheduling crate.
#[derive(Clone, Debug)]
pub struct Leaf {
    /// Stable trigger leaf identity.
    pub id: String,
    /// Exactly five numeric cron fields.
    pub expression: String,
    /// Recorded IANA timezone.
    pub timezone: String,
    /// Earliest (default) or both duplicated local times.
    pub duplicate_time: String,
}
/// Missed-tick behavior, independent of build overlap policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Misfire {
    /// Discard older ticks; an intended tick less than a minute old is on time.
    Skip,
    /// Keep the latest intended tick for each clock leaf.
    CoalesceLatest,
    /// Keep the oldest bounded ticks and leave the remaining cursor for another pass.
    CatchUp,
}
/// Frozen observation request; resolution happens outside the SQLite transaction.
#[derive(Clone, Debug)]
pub struct Evaluation {
    /// Exclusive previously observed key.
    pub cursor: Cursor,
    /// Current operational pause state.
    pub paused: bool,
    /// Missed-tick policy.
    pub misfire: Misfire,
    /// Explicit catch-up limit, from 1 through 100.
    pub max_catch_up: u32,
    /// Already pending intended ticks, from 0 through 100.
    pub pending: u32,
}
/// An intended tick identified by UTC time and authored leaf order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Tick {
    /// Chronological identity within the current trigger epoch.
    pub key: Cursor,
}
/// Bounded prepared work committed only if definition and cursor guards still match.
#[derive(Clone, Debug)]
pub struct Plan {
    /// New exclusive high-water key.
    pub cursor: Cursor,
    /// One frozen wall-clock observation; backward jumps never lower the cursor.
    pub observed_at_us: i64,
    /// At most 100 intended ticks, in chronological order.
    pub ticks: Vec<Tick>,
    /// Intended ticks accounted for by this cursor advance.
    pub matched: u64,
    /// Accounted ticks at least one minute behind the observation.
    pub missed: u64,
    /// Ticks ignored due to pause or skip policy.
    pub ignored: u64,
    /// Older intended ticks replaced by latest-per-leaf evidence.
    pub coalesced: u64,
    /// More observation or catch-up work remains, without silent truncation.
    pub more: bool,
    /// Version of bundled IANA data used to prepare this plan.
    pub tzdb_version: String,
}
