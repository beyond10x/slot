//! Adaptive pool size. The pool should be the CPU that builds can use
//! without starving everything else: the host's cores, minus a reserve for
//! interactive work, minus what builds outside slot already use. CPU
//! pressure trims it further.
//!
//! The controller runs in observe mode first: the daemon logs what it
//! would do next to what happened, and enforcement follows once the log
//! shows the recommendation tracks the host.

use crate::types::HostCensus;

/// Controller settings.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// Host cores.
    pub cores: usize,
    /// Pool size configured with `--tokens`; the upper bound.
    pub max: usize,
    /// Never recommend fewer slots than this.
    pub min: usize,
    /// Cores kept free for interactive work.
    pub reserve: usize,
    /// CPU pressure (`some avg10`, percent) above which the pool shrinks.
    pub psi_high: f64,
    /// Smoothing of the unmanaged build load, 0 to 1 (weight of a new sample).
    pub alpha: f64,
}

impl Settings {
    /// Defaults for a pool of `max` slots on `cores` cores.
    pub fn new(cores: usize, max: usize) -> Self {
        Self {
            cores,
            max,
            min: 2.min(max),
            reserve: 2,
            psi_high: 40.0,
            alpha: 0.2,
        }
    }
}

/// Recommends a pool size from successive census samples.
#[derive(Debug, Clone)]
pub struct Controller {
    s: Settings,
    unmanaged: f64,
    trim: usize,
    started: bool,
}

impl Controller {
    /// A controller with `s`.
    pub fn new(s: Settings) -> Self {
        Self {
            s,
            unmanaged: 0.0,
            trim: 0,
            started: false,
        }
    }

    /// Feeds one sample; returns the recommended number of slots.
    pub fn step(&mut self, c: &HostCensus) -> usize {
        if self.started {
            self.unmanaged += self.s.alpha * (c.unmanaged_cores - self.unmanaged);
        } else {
            self.unmanaged = c.unmanaged_cores;
            self.started = true;
        }
        if c.psi_cpu > self.s.psi_high {
            self.trim = (self.trim + 1).min(self.s.max);
        } else if c.psi_cpu < self.s.psi_high / 2.0 {
            self.trim = self.trim.saturating_sub(1);
        }
        self.recommended()
    }

    /// The current recommendation.
    pub fn recommended(&self) -> usize {
        let free = self.s.cores as f64 - self.s.reserve as f64 - self.unmanaged;
        let base = free.round().max(0.0) as usize;
        base.saturating_sub(self.trim)
            .clamp(self.s.min, self.s.max.max(self.s.min))
    }

    /// Smoothed cores of builds slot does not run.
    pub fn unmanaged(&self) -> f64 {
        self.unmanaged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn census(unmanaged: f64, psi: f64) -> HostCensus {
        HostCensus {
            unmanaged_cores: unmanaged,
            psi_cpu: psi,
            ..HostCensus::default()
        }
    }

    #[test]
    fn idle_host_gets_cores_minus_reserve() {
        let mut c = Controller::new(Settings::new(20, 20));
        assert_eq!(c.step(&census(0.0, 0.0)), 18);
    }

    #[test]
    fn unmanaged_builds_shrink_the_pool_smoothly() {
        let mut c = Controller::new(Settings::new(20, 20));
        c.step(&census(0.0, 0.0));
        let r1 = c.step(&census(10.0, 0.0));
        assert_eq!(r1, 16, "first step moves by alpha");
        for _ in 0..40 {
            c.step(&census(10.0, 0.0));
        }
        assert_eq!(c.recommended(), 8);
    }

    #[test]
    fn pressure_trims_and_recovers() {
        let mut c = Controller::new(Settings::new(20, 20));
        for _ in 0..3 {
            c.step(&census(0.0, 80.0));
        }
        assert_eq!(c.recommended(), 15);
        for _ in 0..3 {
            c.step(&census(0.0, 5.0));
        }
        assert_eq!(c.recommended(), 18);
    }

    #[test]
    fn never_below_min_or_above_max() {
        let mut c = Controller::new(Settings::new(20, 12));
        assert_eq!(c.step(&census(0.0, 0.0)), 12);
        let mut c = Controller::new(Settings::new(4, 4));
        for _ in 0..50 {
            c.step(&census(30.0, 99.0));
        }
        assert_eq!(c.recommended(), 2);
    }
}
