//! Execution measurements exclude cache reuse, queue waits and retry backoff.
use std::collections::BTreeSet;

/// Exact rational, avoiding floating-point rounding even above JavaScript's integer range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ratio {
    /// Sum (or middle pair) in the metric's unit.
    pub numerator: u128,
    /// Divisor, always positive.
    pub denominator: usize,
}
/// Successful materialization samples, ordered oldest to newest by completion and attempt ID.
#[derive(Debug, PartialEq, Eq)]
pub struct Statistics {
    /// Number of measured successes in the entire selected window.
    pub samples: usize,
    /// Exact median, absent for no measured successes.
    pub median: Option<Ratio>,
    /// Mean of the most recent requested number of measured successes.
    pub trailing_mean: Option<Ratio>,
    /// Actual sample count in the trailing mean.
    pub trailing_samples: usize,
}
/// Compute exact statistics. A zero window or arithmetic overflow is invalid.
pub fn statistics(samples: &[u128], window: usize) -> Option<Statistics> {
    if window == 0 {
        return None;
    }
    let n = samples.len();
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let median = if n == 0 {
        None
    } else if n.is_multiple_of(2) {
        Some(Ratio {
            numerator: sorted.get(n / 2 - 1)?.checked_add(*sorted.get(n / 2)?)?,
            denominator: 2,
        })
    } else {
        Some(Ratio {
            numerator: *sorted.get(n / 2)?,
            denominator: 1,
        })
    };
    let trailing_samples = n.min(window);
    let trailing_mean = if trailing_samples == 0 {
        None
    } else {
        Some(Ratio {
            numerator: samples
                .iter()
                .skip(n - trailing_samples)
                .try_fold(0_u128, |a, b| a.checked_add(*b))?,
            denominator: trailing_samples,
        })
    };
    Some(Statistics {
        samples: n,
        median,
        trailing_mean,
        trailing_samples,
    })
}

/// One job's recorded execution duration and dependency indices within a bounded build.
pub struct TimedJob {
    /// None means incomplete evidence, not zero execution time.
    pub duration_ns: Option<u128>,
    /// In-build parents; cache hits may carry zero weight when computing the path only.
    pub parents: Vec<usize>,
}
/// Longest dependency path. Missing timings, cycles and invalid edges return no path.
/// Ties use the first job/parent in the supplied stable order.
pub fn critical_path(jobs: &[TimedJob]) -> Option<(u128, Vec<usize>)> {
    if jobs.is_empty() {
        return Some((0, vec![]));
    }
    let mut pending = vec![0; jobs.len()];
    let mut children = vec![vec![]; jobs.len()];
    let mut ready = BTreeSet::new();
    for (i, job) in jobs.iter().enumerate() {
        job.duration_ns?;
        let parents: BTreeSet<_> = job.parents.iter().copied().collect();
        *pending.get_mut(i)? = parents.len();
        if parents.is_empty() {
            ready.insert(i);
        }
        for p in parents {
            children.get_mut(p)?.push(i);
        }
    }
    let mut totals = vec![0_u128; jobs.len()];
    let mut previous = vec![None; jobs.len()];
    let mut seen = 0;
    while let Some(i) = ready.pop_first() {
        seen += 1;
        let job = jobs.get(i)?;
        let mut best = 0;
        for &p in &job.parents {
            if *totals.get(p)? > best {
                best = *totals.get(p)?;
                *previous.get_mut(i)? = Some(p);
            }
        }
        *totals.get_mut(i)? = best.checked_add(job.duration_ns?)?;
        for &child in children.get(i)? {
            let n = pending.get_mut(child)?;
            *n = n.checked_sub(1)?;
            if *n == 0 {
                ready.insert(child);
            }
        }
    }
    if seen != jobs.len() {
        return None;
    }
    let mut end = 0;
    for i in 1..jobs.len() {
        if totals.get(i)? > totals.get(end)? {
            end = i;
        }
    }
    let duration = *totals.get(end)?;
    let mut path = vec![];
    let mut next = Some(end);
    while let Some(i) = next {
        path.push(i);
        next = *previous.get(i)?;
    }
    path.reverse();
    Some((duration, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_statistics_empty_even_odd_and_trailing_window()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            statistics(&[], 10),
            Some(Statistics {
                samples: 0,
                median: None,
                trailing_mean: None,
                trailing_samples: 0
            })
        );
        let s = statistics(&[10, 90, 30, 50], 2).ok_or("missing statistics")?;
        assert_eq!(s.samples, 4);
        assert_eq!(
            s.median,
            Some(Ratio {
                numerator: 80,
                denominator: 2
            })
        );
        assert_eq!(
            s.trailing_mean,
            Some(Ratio {
                numerator: 80,
                denominator: 2
            })
        );
        assert_eq!(s.trailing_samples, 2);
        let big = 9_007_199_254_740_993;
        assert_eq!(
            statistics(&[big, big + 2, big + 1], 10)
                .ok_or("missing statistics")?
                .median
                .ok_or("missing median")?
                .numerator,
            big + 1
        );
        assert!(statistics(&[1], 0).is_none());
        assert!(statistics(&[u128::MAX, 1], 10).is_none());
        Ok(())
    }
    #[test]
    fn dependency_path_uses_execution_weights_not_sum_or_wall_time() {
        let mut jobs = vec![
            TimedJob {
                duration_ns: Some(10),
                parents: vec![],
            },
            TimedJob {
                duration_ns: Some(50),
                parents: vec![0],
            },
            TimedJob {
                duration_ns: Some(20),
                parents: vec![0],
            },
            TimedJob {
                duration_ns: Some(0),
                parents: vec![1, 2],
            },
            TimedJob {
                duration_ns: Some(7),
                parents: vec![3],
            },
        ];
        assert_eq!(critical_path(&jobs), Some((67, vec![0, 1, 3, 4])));
        jobs[2].duration_ns = None;
        assert!(critical_path(&jobs).is_none());
        jobs[2].duration_ns = Some(20);
        jobs[0].parents.push(4);
        assert!(critical_path(&jobs).is_none());
    }
}
