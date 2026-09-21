//! Single application mapping from effective workspace settings to execution services.
use serde_json::{Value, json};
use tf_catalog::workspace::EffectivePolicy;
use tf_exec::{admission::Capacity, timing::Limits};
/// Unrepresentable configuration; never silently wraps or disables a configured gate.
#[derive(Debug, thiserror::Error)]
#[error("Resource capacity cannot be represented; reduce the configured resource value")]
pub struct Error;
/// Frozen resource values shared by planning, worker launch and coordinator admission.
#[derive(Clone, Debug)]
pub struct Resolved {
    /// Coordinator-wide atomic reservation capacities.
    pub capacity: Capacity,
    /// Independent phase policies with exact winning origins.
    pub timers: Limits,
    /// Default worker threads derived from reserved CPU tokens and active slots.
    pub worker_threads: u32,
}
impl Resolved {
    /// Preserve absent memory and disabled deadlines; resolve MiB to bytes exactly.
    pub fn new(policy: &EffectivePolicy) -> Result<Self, Error> {
        let jobs = u32::try_from(policy.max_jobs.value).map_err(|_| Error)?;
        if jobs == 0 || policy.cpu_tokens == 0 {
            return Err(Error);
        }
        let memory_bytes = policy
            .memory_budget_mib
            .map(|s| s.value.checked_mul(1024 * 1024).ok_or(Error))
            .transpose()?;
        if memory_bytes == Some(0) {
            return Err(Error);
        }
        Ok(Self {
            capacity: Capacity {
                jobs,
                cpu: policy.cpu_tokens,
                memory_bytes,
                ..Capacity::default()
            },
            timers: Limits {
                transform: policy.transform_seconds,
                validation: policy.validation_seconds,
                interactive: policy.interactive_seconds,
                discovery: policy.discovery_seconds,
            },
            worker_threads: (policy.cpu_tokens / jobs.min(policy.cpu_tokens)).max(1),
        })
    }
    /// Complete public/accepted-plan resource projection. No machine-local override applies.
    pub fn json(&self, policy: &EffectivePolicy) -> Value {
        json!({
            "timeout_seconds":policy.transform_seconds.value.to_string(),"timeout_origin":format!("{:?}",policy.transform_seconds.origin),
            "validation_timeout_seconds":policy.validation_seconds.value.to_string(),"validation_timeout_origin":format!("{:?}",policy.validation_seconds.origin),
            "discovery_timeout_seconds":policy.discovery_seconds.value.to_string(),"discovery_timeout_origin":format!("{:?}",policy.discovery_seconds.origin),
            "interactive_timeout_seconds":policy.interactive_seconds.value.to_string(),"interactive_timeout_origin":format!("{:?}",policy.interactive_seconds.origin),
            "max_jobs":policy.max_jobs.value.to_string(),"max_jobs_origin":format!("{:?}",policy.max_jobs.origin),
            "cpu_tokens":policy.cpu_tokens.to_string(),"cpu_origin":format!("{:?}",policy.cpu_origin),"worker_threads":self.worker_threads.to_string(),
            "memory_budget_mib":policy.memory_budget_mib.map(|s|s.value.to_string()),"memory_origin":policy.memory_budget_mib.map(|s|format!("{:?}",s.origin)),
            "memory_enforcement":if self.capacity.memory_bytes.is_some(){"estimated_reservation"}else{"none"}
        })
    }
}
