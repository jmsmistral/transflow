//! Provenance of resolved resource settings, independent of configuration transport.
/// Winning layer for an effective resource setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    /// Built-in default.
    Default,
    /// Workspace authoring policy.
    Workspace,
    /// Producer declaration.
    Definition,
    /// Accepted build or schedule.
    Build,
    /// Explicit CLI or API override.
    Explicit,
}
/// Resolved integer with provenance. Zero timeout explicitly disables that deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Setting {
    /// Effective integer value.
    pub value: u64,
    /// Winning source.
    pub origin: Origin,
}
