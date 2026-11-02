use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Bounds the failure response.
#[derive(Debug, Clone, Copy)]
pub(super) struct BreakerConfig {
    /// consecutive failures before opening
    pub threshold: u32,
    /// first cooldown
    pub base_cool: Duration,
    /// cooldown ceiling
    pub max_cool: Duration,
}

const MINUTE: Duration = Duration::from_secs(60);

pub(super) const DEFAULT_BREAKER: BreakerConfig = BreakerConfig {
    threshold: 3,
    base_cool: MINUTE.saturating_mul(3),
    max_cool: MINUTE.saturating_mul(30),
};

/// The admission decision for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Admit {
    Pass,
    /// the single half-open probe
    Probe,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Closed,
    Open {
        until: Instant,
    },
    /// claimed: the probe is in flight
    HalfOpen {
        claimed: bool,
    },
}

/// Guards one upstream domain: closed passes everything, open rejects
/// instantly, half-open admits exactly one probe.
#[derive(Debug)]
pub(super) struct Breaker {
    cfg: BreakerConfig,
    state: State,
    fails: u32,
    cool: Duration,
}

impl Breaker {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self {
            cfg,
            state: State::Closed,
            fails: 0,
            cool: Duration::ZERO,
        }
    }

    /// Decides whether a request may pass now. force (playback) always
    /// passes and does not claim the probe slot.
    pub fn allow(&mut self, now: Instant, force: bool) -> Admit {
        if force {
            return Admit::Pass;
        }
        match self.state {
            State::Closed => Admit::Pass,
            State::Open { until } if now < until => Admit::Reject,
            State::HalfOpen { claimed: true } => Admit::Reject,
            State::Open { .. } | State::HalfOpen { claimed: false } => {
                self.state = State::HalfOpen { claimed: true };
                Admit::Probe
            }
        }
    }

    /// Hands back a probe slot whose request was refused before reaching
    /// the upstream, so the next request can probe instead.
    pub fn release_probe(&mut self) {
        if self.state == (State::HalfOpen { claimed: true }) {
            self.state = State::HalfOpen { claimed: false };
        }
    }

    /// Closes the breaker and resets the backoff sequence.
    pub fn record_success(&mut self) {
        self.state = State::Closed;
        self.fails = 0;
        self.cool = Duration::ZERO;
    }

    /// Counts a failure and opens the breaker once the threshold is hit,
    /// doubling the cooldown each consecutive open.
    pub fn record_failure(&mut self, now: Instant) {
        self.fails += 1;
        let half_open = matches!(self.state, State::HalfOpen { .. });
        if !half_open && self.fails < self.cfg.threshold {
            return;
        }
        self.cool = if self.cool < self.cfg.base_cool {
            self.cfg.base_cool
        } else {
            (self.cool * 2).min(self.cfg.max_cool)
        };
        self.state = State::Open {
            until: now + self.cool,
        };
    }

    /// Names the state for /state.
    pub fn state_name(&self) -> &'static str {
        match self.state {
            State::Closed => "closed",
            State::Open { .. } => "open",
            State::HalfOpen { .. } => "half-open",
        }
    }
}

/// Keeps one breaker per upstream domain.
#[derive(Debug)]
pub(super) struct Breakers {
    cfg: BreakerConfig,
    by_domain: Mutex<HashMap<String, Breaker>>,
}

impl Default for Breakers {
    fn default() -> Self {
        Self::new(DEFAULT_BREAKER)
    }
}

impl Breakers {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self {
            cfg,
            by_domain: Mutex::default(),
        }
    }

    fn with<T>(&self, domain: &str, f: impl FnOnce(&mut Breaker) -> T) -> T {
        let mut map = self
            .by_domain
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let breaker = map
            .entry(domain.to_owned())
            .or_insert_with(|| Breaker::new(self.cfg));
        f(breaker)
    }

    pub fn allow(&self, domain: &str, now: Instant, force: bool) -> Admit {
        self.with(domain, |b| b.allow(now, force))
    }

    pub fn release_probe(&self, domain: &str) {
        self.with(domain, Breaker::release_probe);
    }

    pub fn record_success(&self, domain: &str) {
        self.with(domain, Breaker::record_success);
    }

    pub fn record_failure(&self, domain: &str, now: Instant) {
        self.with(domain, |b| b.record_failure(now));
    }

    /// Breaker states keyed by domain, sorted like Go's JSON maps.
    pub fn snapshot(&self) -> BTreeMap<String, &'static str> {
        let map = self
            .by_domain
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        map.iter()
            .map(|(domain, b)| (domain.clone(), b.state_name()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);
    const BASE: Duration = DEFAULT_BREAKER.base_cool;

    fn opened(now: Instant) -> Breaker {
        let mut b = Breaker::new(DEFAULT_BREAKER);
        for _ in 0..3 {
            b.record_failure(now);
        }
        b
    }

    #[test]
    fn open_and_recover() {
        let mut b = Breaker::new(DEFAULT_BREAKER);
        let now = Instant::now();

        // three consecutive failures open it
        for _ in 0..3 {
            assert_eq!(b.allow(now, false), Admit::Pass, "must pass while closed");
            b.record_failure(now);
        }
        assert_eq!(b.allow(now + SECOND, false), Admit::Reject);
        assert_eq!(b.state_name(), "open");

        // after the cooldown exactly one probe passes
        let after = now + BASE + SECOND;
        assert_eq!(b.allow(after, false), Admit::Probe);
        assert_eq!(b.state_name(), "half-open");
        assert_eq!(b.allow(after, false), Admit::Reject, "probe in flight");

        // probe success closes and resets
        b.record_success();
        assert_eq!(b.allow(after, false), Admit::Pass);
        assert_eq!(b.state_name(), "closed");
    }

    #[test]
    fn cooldown_doubles() {
        let now = Instant::now();
        let mut b = opened(now);

        // probe at cooldown boundary fails -> reopen with doubled cooldown
        let probe_at = now + BASE + SECOND;
        assert_eq!(b.allow(probe_at, false), Admit::Probe);
        b.record_failure(probe_at);
        assert_eq!(b.state_name(), "open");
        // the doubled cooldown runs from the failed probe: 3m + 6m
        assert_eq!(b.allow(probe_at, false), Admit::Reject);
        assert_eq!(b.allow(now + 3 * BASE + 2 * SECOND, false), Admit::Probe);
    }

    #[test]
    fn cooldown_caps() {
        let mut now = Instant::now();
        let mut b = opened(now);
        for _ in 0..10 {
            now += DEFAULT_BREAKER.max_cool + SECOND;
            assert_eq!(b.allow(now, false), Admit::Probe);
            b.record_failure(now);
        }
        assert_eq!(b.cool, DEFAULT_BREAKER.max_cool);
        assert_eq!(
            b.allow(now + DEFAULT_BREAKER.max_cool - SECOND, false),
            Admit::Reject
        );
    }

    #[test]
    fn released_probe_can_be_reclaimed() {
        let now = Instant::now();
        let mut b = opened(now);
        let after = now + BASE + SECOND;
        assert_eq!(b.allow(after, false), Admit::Probe);
        b.release_probe();
        assert_eq!(b.state_name(), "half-open");
        assert_eq!(b.allow(after, false), Admit::Probe, "slot handed back");
        assert_eq!(b.allow(after, false), Admit::Reject);
    }

    #[test]
    fn release_outside_half_open_is_a_no_op() {
        let now = Instant::now();
        let mut b = opened(now);
        b.release_probe();
        assert_eq!(b.allow(now, false), Admit::Reject, "still open");
        let mut closed = Breaker::new(DEFAULT_BREAKER);
        closed.release_probe();
        assert_eq!(closed.state_name(), "closed");
    }

    #[test]
    fn force_bypasses() {
        let now = Instant::now();
        let mut b = opened(now);
        assert_eq!(
            b.allow(now, true),
            Admit::Pass,
            "force must bypass open state"
        );
        // a forced success resets the breaker
        b.record_success();
        assert_eq!(b.state_name(), "closed");
    }

    #[test]
    fn set_is_per_domain() {
        let set = Breakers::default();
        let now = Instant::now();
        for _ in 0..3 {
            set.record_failure("a.example", now);
        }
        assert_eq!(set.allow("a.example", now, false), Admit::Reject);
        assert_eq!(set.allow("b.example", now, false), Admit::Pass);
        let snapshot = set.snapshot();
        assert_eq!(
            snapshot.into_iter().collect::<Vec<_>>(),
            [("a.example".into(), "open"), ("b.example".into(), "closed")]
        );
    }
}
