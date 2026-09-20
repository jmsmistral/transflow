//! Source refresh decisions use a single frozen evaluation clock, never wall-clock reads.
/// Normalized source policy. TTL is positive seconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshPolicy {
    /// Refresh on every eligible build.
    Always,
    /// Require an explicit source target or refresh request.
    Manual,
    /// Refresh after this positive duration.
    Ttl(std::num::NonZeroU64),
}
/// Why an eligible producer must run or remain a retained boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshDecision {
    /// Refresh on every eligible build.
    Always,
    /// Explicit user source request.
    Explicit,
    /// Force bypasses an eligible source cache or TTL.
    Forced,
    /// No retained source publication exists.
    Missing,
    /// The frozen clock reaches or exceeds the TTL.
    Due,
    /// Use the retained current source version.
    Current,
    /// Manual I/O is not authorized by traversal or force.
    ManualBoundary,
}
impl RefreshDecision {
    /// Whether this decision requires a new source execution.
    pub fn executes(self) -> bool {
        !matches!(self, Self::Current | Self::ManualBoundary)
    }
}
/// Negative/future timestamps and overflowing TTLs fail closed.
pub fn decide(
    policy: RefreshPolicy,
    explicit: bool,
    force: bool,
    evaluated_us: i64,
    published_us: Option<i64>,
) -> Result<RefreshDecision, &'static str> {
    if evaluated_us < 0 || published_us.is_some_and(|t| t < 0 || t > evaluated_us) {
        return Err(
            "Source refresh timestamps are invalid; inspect the retained publication clock",
        );
    }
    let ttl = match policy {
        RefreshPolicy::Ttl(seconds) => Some(
            i64::try_from(seconds.get())
                .ok()
                .and_then(|s| s.checked_mul(1_000_000))
                .ok_or("Source TTL exceeds the supported clock range")?,
        ),
        _ => None,
    };
    if explicit {
        return Ok(RefreshDecision::Explicit);
    }
    if policy == RefreshPolicy::Manual {
        return Ok(RefreshDecision::ManualBoundary);
    }
    if force {
        return Ok(RefreshDecision::Forced);
    }
    if policy == RefreshPolicy::Always {
        return Ok(RefreshDecision::Always);
    }
    let Some(published) = published_us else {
        return Ok(RefreshDecision::Missing);
    };
    Ok(if ttl.is_some_and(|ttl| evaluated_us - published >= ttl) {
        RefreshDecision::Due
    } else {
        RefreshDecision::Current
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refresh_clock_and_force_do_not_authorize_manual_io() {
        let ttl = RefreshPolicy::Ttl(std::num::NonZeroU64::MIN);
        assert_eq!(
            decide(ttl, false, false, 1_000_000, Some(1)),
            Ok(RefreshDecision::Current)
        );
        assert_eq!(
            decide(ttl, false, false, 1_000_000, Some(0)),
            Ok(RefreshDecision::Due)
        );
        assert_eq!(
            decide(ttl, false, false, 0, None),
            Ok(RefreshDecision::Missing)
        );
        assert_eq!(
            decide(ttl, false, true, 0, Some(0)),
            Ok(RefreshDecision::Forced)
        );
        assert_eq!(
            decide(RefreshPolicy::Always, false, false, 0, Some(0)),
            Ok(RefreshDecision::Always)
        );
        assert_eq!(
            decide(RefreshPolicy::Manual, false, true, 0, None),
            Ok(RefreshDecision::ManualBoundary)
        );
        assert_eq!(
            decide(RefreshPolicy::Manual, true, false, 0, None),
            Ok(RefreshDecision::Explicit)
        );
        assert!(decide(ttl, false, false, 0, Some(1)).is_err());
        assert!(decide(ttl, true, true, -1, None).is_err());
        assert!(
            decide(
                RefreshPolicy::Ttl(std::num::NonZeroU64::MAX),
                true,
                true,
                0,
                None
            )
            .is_err()
        );
    }
}
