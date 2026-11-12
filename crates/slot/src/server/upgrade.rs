//! When to re-execute after `systemctl --user reload slot`.
//!
//! The daemon re-executes only while no job runs. Holding admission until
//! then lets the pool drain, but one long job then holds every build on the
//! machine (2026-11-12: 25 jobs queued behind one 30-minute test run). So
//! the hold is bounded: admission stops for at most `hold`; if a job still
//! runs, admission opens again, and the next hold starts only when every
//! running job is predicted to end within `hold` (from earlier runs of the
//! same command), or right away when the pool is idle.

use std::time::{Duration, Instant};

/// Time admission stays open after a hold ended before the next may start,
/// so queued jobs get to run.
pub const MIN_OPEN: Duration = Duration::from_secs(10);

/// What the daemon does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing changes.
    Wait,
    /// Stop admitting jobs.
    Hold,
    /// Admit jobs again: the hold ran out with jobs still running.
    Release,
    /// No job runs: re-execute now.
    Reexec,
}

/// A pending upgrade.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    hold: Duration,
    requested: Option<Instant>,
    holding_since: Option<Instant>,
    released_at: Option<Instant>,
    holds: u32,
}

impl Plan {
    /// A plan that holds admission for at most `hold` at a time; zero never
    /// holds and re-executes at the first idle moment.
    pub fn new(hold: Duration) -> Self {
        Self {
            hold,
            ..Self::default()
        }
    }

    /// An upgrade was requested. Returns the step to take now.
    pub fn request(&mut self, now: Instant, idle: bool) -> Step {
        if self.requested.is_none() {
            self.requested = Some(now);
        }
        if idle {
            return Step::Reexec;
        }
        if self.holding_since.is_none() && !self.hold.is_zero() {
            self.holding_since = Some(now);
            self.holds += 1;
            return Step::Hold;
        }
        Step::Wait
    }

    /// Whether an upgrade is pending.
    pub fn pending(&self) -> bool {
        self.requested.is_some()
    }

    /// Whether admission is held now.
    #[cfg(test)]
    pub fn holding(&self) -> bool {
        self.holding_since.is_some()
    }

    /// Holds started so far.
    #[cfg(test)]
    pub fn holds(&self) -> u32 {
        self.holds
    }

    /// Time left in the current hold.
    pub fn hold_left(&self, now: Instant) -> Option<Duration> {
        self.holding_since
            .map(|t| self.hold.saturating_sub(now.duration_since(t)))
    }

    /// One look. `idle`: no job runs or holds slots. `remaining`: the
    /// longest predicted time left of the running jobs, `None` when a job has
    /// no prediction or has run past it.
    pub fn step(&mut self, now: Instant, idle: bool, remaining: Option<Duration>) -> Step {
        if self.requested.is_none() {
            return Step::Wait;
        }
        if idle {
            return Step::Reexec;
        }
        if let Some(t) = self.holding_since {
            if now.duration_since(t) >= self.hold {
                self.holding_since = None;
                self.released_at = Some(now);
                return Step::Release;
            }
            return Step::Wait;
        }
        let open_long_enough = self
            .released_at
            .is_none_or(|t| now.duration_since(t) >= MIN_OPEN);
        if !self.hold.is_zero() && open_long_enough && remaining.is_some_and(|r| r <= self.hold) {
            self.holding_since = Some(now);
            self.holds += 1;
            return Step::Hold;
        }
        Step::Wait
    }

    /// One line for `slot status`.
    pub fn describe(&self, now: Instant) -> String {
        if !self.pending() {
            return String::new();
        }
        match self.hold_left(now) {
            Some(left) => format!(
                "upgrade pending: no new job starts for {}s, re-executing once none runs",
                left.as_secs()
            ),
            None if self.hold.is_zero() => {
                "upgrade pending: re-executing at the first moment no job runs".into()
            }
            None => format!(
                "upgrade pending: jobs start as usual; admission stops again when the running \
                 jobs are expected to end within {}s, or none runs (hold {} so far)",
                self.hold.as_secs(),
                self.holds
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    #[test]
    fn an_idle_pool_re_executes_at_once() {
        let t0 = Instant::now();
        let mut p = Plan::new(60 * S);
        assert_eq!(p.step(t0, true, None), Step::Wait, "nothing requested");
        assert_eq!(p.request(t0, true), Step::Reexec);
    }

    #[test]
    fn a_long_job_gets_a_bounded_hold_then_admission_opens() {
        let t0 = Instant::now();
        let mut p = Plan::new(60 * S);
        assert_eq!(p.request(t0, false), Step::Hold);
        assert!(p.holding());
        assert_eq!(p.step(t0 + 30 * S, false, None), Step::Wait);
        assert_eq!(p.hold_left(t0 + 30 * S), Some(30 * S));
        assert_eq!(p.step(t0 + 60 * S, false, None), Step::Release);
        assert!(!p.holding() && p.pending());
        assert_eq!(
            p.step(t0 + 120 * S, false, None),
            Step::Wait,
            "no prediction: wait for idle"
        );
        assert_eq!(
            p.step(t0 + 130 * S, false, Some(300 * S)),
            Step::Wait,
            "predicted end too far"
        );
        assert_eq!(p.step(t0 + 140 * S, false, Some(40 * S)), Step::Hold);
        assert_eq!(p.holds(), 2);
        assert_eq!(p.step(t0 + 170 * S, true, None), Step::Reexec);
    }

    #[test]
    fn admission_stays_open_a_moment_after_a_hold() {
        let t0 = Instant::now();
        let mut p = Plan::new(60 * S);
        p.request(t0, false);
        assert_eq!(p.step(t0 + 60 * S, false, Some(5 * S)), Step::Release);
        assert_eq!(p.step(t0 + 65 * S, false, Some(5 * S)), Step::Wait);
        assert_eq!(p.step(t0 + 70 * S, false, Some(5 * S)), Step::Hold);
    }

    #[test]
    fn a_zero_hold_never_stops_admission() {
        let t0 = Instant::now();
        let mut p = Plan::new(Duration::ZERO);
        assert_eq!(p.request(t0, false), Step::Wait);
        assert_eq!(p.step(t0 + 5 * S, false, Some(S)), Step::Wait);
        assert_eq!(p.step(t0 + 6 * S, true, None), Step::Reexec);
        assert!(p.describe(t0).contains("first moment"));
    }
}
