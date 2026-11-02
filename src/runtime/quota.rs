use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Limits scan-driven resolutions per upstream domain in a sliding
/// window. Playback (force) requests are never counted.
#[derive(Debug)]
pub(super) struct Quota {
    limit: usize,
    window: Duration,
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl Quota {
    pub fn new(limit: usize, window: Duration) -> Self {
        Self {
            limit,
            window,
            hits: Mutex::default(),
        }
    }

    /// Whether a hit at t still counts at now. Written as a distance
    /// rather than `now - window`, which would underflow on a host that
    /// booted less than one window ago.
    fn in_window(&self, t: Instant, now: Instant) -> bool {
        now.saturating_duration_since(t) < self.window
    }

    /// Records and reports whether a scan request may pass.
    pub fn allow(&self, domain: &str, now: Instant) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(PoisonError::into_inner);
        let recent = hits.entry(domain.to_owned()).or_default();
        recent.retain(|&t| self.in_window(t, now));
        if recent.len() >= self.limit {
            return false;
        }
        recent.push(now);
        true
    }

    /// Current window usage per domain for /state, sorted like Go's JSON
    /// maps; idle domains are left out.
    pub fn usage(&self, now: Instant) -> BTreeMap<String, usize> {
        let hits = self.hits.lock().unwrap_or_else(PoisonError::into_inner);
        hits.iter()
            .map(|(domain, ts)| {
                let n = ts.iter().filter(|&&t| self.in_window(t, now)).count();
                (domain.clone(), n)
            })
            .filter(|&(_, n)| n > 0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);
    const HOUR: Duration = Duration::from_secs(60 * 60);

    #[test]
    fn window() {
        let q = Quota::new(3, HOUR);
        let now = Instant::now();

        for i in 0..3 {
            assert!(q.allow("a.example", now + i * MINUTE), "inside limit");
        }
        assert!(
            !q.allow("a.example", now + 3 * MINUTE),
            "must block over limit"
        );
        // other domains unaffected
        assert!(
            q.allow("b.example", now + 3 * MINUTE),
            "quota is per domain"
        );
        // entries age out of the window
        assert!(
            q.allow("a.example", now + 61 * MINUTE),
            "old hits must expire"
        );
    }

    #[test]
    fn rejected_requests_are_not_counted() {
        let q = Quota::new(1, HOUR);
        let now = Instant::now();
        assert!(q.allow("a", now));
        for _ in 0..5 {
            assert!(!q.allow("a", now));
        }
        assert_eq!(q.usage(now).get("a"), Some(&1));
    }

    #[test]
    fn usage() {
        let q = Quota::new(10, HOUR);
        let now = Instant::now();
        q.allow("b.example", now);
        q.allow("a.example", now);
        q.allow("a.example", now + MINUTE);
        assert_eq!(
            q.usage(now + MINUTE).into_iter().collect::<Vec<_>>(),
            [("a.example".into(), 2), ("b.example".into(), 1)]
        );
        // idle domains drop out of the report
        assert!(q.usage(now + 2 * HOUR).is_empty());
    }
}
