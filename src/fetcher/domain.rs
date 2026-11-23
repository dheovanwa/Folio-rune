//! One clock per host.
//!
//! A reader that prefetches chapters will hit the same host in bursts, and the fastest way to be
//! blocked is to look like a scraper. So every request asks this module first: it answers
//! "now", "in 800ms", or "no, the host already has as many requests in flight as it allows".
//!
//! Three properties matter, and each one is why the type is shaped the way it is:
//!
//! 1. **The gap adapts.** A host that starts answering 429 pushes its own gap up; a run of
//!    successes brings it back down towards the floor. One bad host never slows the others.
//! 2. **Waiting is the caller's business.** `try_acquire` never sleeps. A prefetch thread can
//!    park for the returned duration and stay cancellable; the UI thread can give up instead.
//! 3. **Time is a parameter.** Every method takes `now`. That is what makes the whole thing
//!    testable without a sleeping test, and what keeps `Instant::now()` out of the lock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How hard to be on a host. Defaults are deliberately gentle: a reader prefetching two chapters
/// ahead is not in a hurry, and the cost of being wrong in the fast direction is a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// The steady-state gap between two requests to the same host.
    pub base_gap: Duration,
    /// The floor the gap may narrow back to after successes.
    pub min_gap: Duration,
    /// The ceiling the gap may widen to after repeated 429s.
    pub max_gap: Duration,
    /// Concurrent requests allowed per host. Above one only pays off for hosts that allow it.
    pub max_in_flight: usize,
    /// Multiplier applied to the gap on a 429, as a percentage so the policy stays `Eq`.
    pub widen_percent: u32,
    /// Successes in a row before the gap is narrowed by `widen_percent`.
    pub narrow_after: u32,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            base_gap: Duration::from_millis(750),
            min_gap: Duration::from_millis(250),
            max_gap: Duration::from_secs(30),
            max_in_flight: 1,
            widen_percent: 200,
            narrow_after: 4,
        }
    }
}

/// What a host is currently allowed to do.
#[derive(Debug)]
pub enum Acquired {
    /// Go now. The [`Slot`] must be reported or dropped, or the host leaks a permit.
    Ready(Slot),
    /// Too early, or the host is at its concurrency cap. Come back after this long.
    Wait(Duration),
}

impl Acquired {
    pub fn ready(self) -> Option<Slot> {
        match self {
            Acquired::Ready(slot) => Some(slot),
            Acquired::Wait(_) => None,
        }
    }

    /// The duration to park for, zero when the slot is ready.
    pub fn wait_for(&self) -> Duration {
        match self {
            Acquired::Ready(_) => Duration::ZERO,
            Acquired::Wait(d) => *d,
        }
    }
}

/// How a request ended, as far as the rate limit is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A page came back. Counts towards narrowing the gap.
    Success,
    /// The server said 429. Widens this host's gap; `retry_after` is its own advice, if any.
    RateLimited { retry_after: Option<Duration> },
    /// A 5xx or a transport failure: not the host's fault, so do not punish its rate.
    Error,
}

struct HostState {
    /// The current adaptive gap.
    gap: Duration,
    /// Earliest instant the next request may start.
    not_before: Option<Instant>,
    /// Requests currently on the wire to this host.
    in_flight: usize,
    /// Consecutive successes, for narrowing.
    streak: u32,
}

impl HostState {
    fn new(gap: Duration) -> HostState {
        HostState { gap, not_before: None, in_flight: 0, streak: 0 }
    }
}

/// Per-host rate limiting and concurrency control.
///
/// Shared behind an `Arc` by the fetcher. A [`Slot`] holds a clone of that `Arc`, so a permit
/// cannot outlive its limiter and no lifetime plumbing reaches the worker threads.
pub struct DomainLimiter {
    policy: Policy,
    hosts: Mutex<HashMap<String, HostState>>,
}

impl std::fmt::Debug for DomainLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DomainLimiter").field("policy", &self.policy).finish_non_exhaustive()
    }
}

impl DomainLimiter {
    pub fn new(policy: Policy) -> Arc<DomainLimiter> {
        Arc::new(DomainLimiter { policy, hosts: Mutex::new(HashMap::new()) })
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    fn hosts(&self) -> MutexGuard<'_, HashMap<String, HostState>> {
        // A poisoned lock means a worker panicked mid-update. The map holds only counters and
        // instants, so the worst case is a stale gap; taking the data is better than panicking
        // the whole fetch path.
        self.hosts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ask to start a request to `host` at `now`.
    ///
    /// A `Wait` carries the duration until the host is free; the caller chooses whether to
    /// sleep, retry, or drop the prefetch. Nothing is reserved unless `Ready` comes back.
    pub fn try_acquire(self: &Arc<Self>, host: &str, now: Instant) -> Acquired {
        let mut hosts = self.hosts();
        let state = hosts
            .entry(host.to_string())
            .or_insert_with(|| HostState::new(self.policy.base_gap));

        if state.in_flight >= self.policy.max_in_flight {
            // Concurrency is a hard wall. The host frees up when a slot is reported, not on a
            // timer, so the caller polls rather than parking for a made-up duration.
            return Acquired::Wait(Duration::from_millis(0));
        }
        if let Some(not_before) = state.not_before
            && now < not_before
        {
            return Acquired::Wait(not_before.duration_since(now));
        }
        state.in_flight += 1;
        Acquired::Ready(Slot { limiter: Arc::clone(self), host: host.to_string(), reported: false })
    }

    /// The current gap for a host, for diagnostics and tests.
    pub fn gap(&self, host: &str) -> Duration {
        self.hosts().get(host).map_or(self.policy.base_gap, |s| s.gap)
    }

    /// How many requests are in flight to a host.
    pub fn in_flight(&self, host: &str) -> usize {
        self.hosts().get(host).map_or(0, |s| s.in_flight)
    }

    /// Record how a request ended and free its permit. Called by [`Slot::report`], and by
    /// `Slot::drop` as `Outcome::Error` when a caller forgot — so a panic cannot wedge a host
    /// at its concurrency cap forever.
    fn release(&self, host: &str, outcome: Outcome, now: Instant) {
        let mut hosts = self.hosts();
        let Some(state) = hosts.get_mut(host) else { return };
        state.in_flight = state.in_flight.saturating_sub(1);
        state.not_before = Some(now.checked_add(state.gap).unwrap_or(now));

        match outcome {
            Outcome::Success => {
                state.streak = state.streak.saturating_add(1);
                if state.streak >= self.policy.narrow_after && state.gap > self.policy.min_gap {
                    let narrowed = state.gap.mul_f64(100.0 / f64::from(self.policy.widen_percent));
                    state.gap = narrowed.max(self.policy.min_gap);
                    state.streak = 0;
                }
            }
            Outcome::RateLimited { retry_after } => {
                state.streak = 0;
                let widened = state.gap.mul_f64(f64::from(self.policy.widen_percent) / 100.0);
                state.gap = widened.clamp(self.policy.min_gap, self.policy.max_gap);
                // The server's own advice wins over our guess, but is clamped to the policy
                // band: a `Retry-After: 0` is not an invitation to hammer.
                let wait = retry_after.unwrap_or(state.gap).clamp(self.policy.min_gap, self.policy.max_gap);
                state.not_before = Some(now.checked_add(wait).unwrap_or(now));
            }
            Outcome::Error => {
                // A 500 says nothing about politeness. Keep the gap, reset the streak so the
                // narrowing counter reflects *successful* requests only.
                state.streak = 0;
            }
        }
    }
}

/// A permit to make one request to one host. Dropping it without reporting counts as an error,
/// which frees the permit without changing the host's rate.
#[derive(Debug)]
pub struct Slot {
    limiter: Arc<DomainLimiter>,
    host: String,
    reported: bool,
}

impl Slot {
    /// Report the outcome. Frees the permit and updates the host's gap.
    pub fn report(mut self, outcome: Outcome, now: Instant) {
        self.limiter.release(&self.host, outcome, now);
        self.reported = true;
    }

    /// The host this permit belongs to.
    pub fn host(&self) -> &str {
        &self.host
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if !self.reported {
            // An abandoned request: free the permit but do not touch the host's rate.
            self.limiter.release(&self.host, Outcome::Error, Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> Arc<DomainLimiter> {
        DomainLimiter::new(Policy {
            base_gap: Duration::from_millis(100),
            min_gap: Duration::from_millis(50),
            max_gap: Duration::from_secs(2),
            max_in_flight: 1,
            widen_percent: 200,
            narrow_after: 2,
        })
    }

    #[test]
    fn the_first_request_to_a_host_is_immediate() {
        let l = limiter();
        let now = Instant::now();
        let slot = l.try_acquire("example.com", now).ready().expect("first request is free");
        assert_eq!(l.in_flight("example.com"), 1);
        slot.report(Outcome::Success, now);
        assert_eq!(l.in_flight("example.com"), 0);
    }

    #[test]
    fn a_second_request_waits_out_the_gap() {
        let l = limiter();
        let now = Instant::now();
        l.try_acquire("example.com", now).ready().unwrap().report(Outcome::Success, now);
        // Immediately after, the host is still cooling down.
        let wait = l.try_acquire("example.com", now).wait_for();
        assert_eq!(wait, Duration::from_millis(100));
        // Once the gap has elapsed, it is free again.
        assert!(l.try_acquire("example.com", now + Duration::from_millis(100)).ready().is_some());
    }

    #[test]
    fn a_429_widens_and_a_streak_narrows_again() {
        let l = limiter();
        let mut now = Instant::now();
        l.try_acquire("h", now).ready().unwrap().report(Outcome::RateLimited { retry_after: None }, now);
        assert_eq!(l.gap("h"), Duration::from_millis(200), "doubled on the 429");

        // Sit out the widened gap, then succeed twice: the gap halves back.
        now += Duration::from_millis(200);
        l.try_acquire("h", now).ready().unwrap().report(Outcome::Success, now);
        now += Duration::from_millis(200);
        l.try_acquire("h", now).ready().unwrap().report(Outcome::Success, now);
        assert_eq!(l.gap("h"), Duration::from_millis(100), "narrowed after the streak");
    }

    #[test]
    fn retry_after_is_honoured_but_clamped_to_the_policy_band() {
        let l = limiter();
        let now = Instant::now();
        l.try_acquire("h", now)
            .ready()
            .unwrap()
            .report(Outcome::RateLimited { retry_after: Some(Duration::from_secs(5)) }, now);
        // 5s is above the 2s ceiling, so the ceiling is what the caller is told.
        let wait = l.try_acquire("h", now).wait_for();
        assert!(wait <= Duration::from_secs(2) && wait >= Duration::from_millis(50), "got {wait:?}");
    }

    #[test]
    fn one_host_never_holds_up_another() {
        let l = limiter();
        let now = Instant::now();
        l.try_acquire("slow.example", now).ready().unwrap().report(
            Outcome::RateLimited { retry_after: Some(Duration::from_secs(10)) },
            now,
        );
        assert!(l.try_acquire("fast.example", now).ready().is_some());
    }

    #[test]
    fn the_concurrency_cap_is_a_hard_wall() {
        let l = limiter();
        let now = Instant::now();
        let first = l.try_acquire("h", now).ready().unwrap();
        assert!(l.try_acquire("h", now).ready().is_none(), "second is refused while one is in flight");
        first.report(Outcome::Success, now);
        assert!(l.try_acquire("h", now + Duration::from_secs(1)).ready().is_some());
    }

    #[test]
    fn a_dropped_slot_frees_the_permit() {
        let l = limiter();
        let now = Instant::now();
        {
            let _forgotten = l.try_acquire("h", now).ready().unwrap();
        }
        assert_eq!(l.in_flight("h"), 0, "dropping an unreported slot must not leak the permit");
    }
}
