//! A job's width on a node: how many cores its test runners and compilers
//! may use, set once when the job starts.
//!
//! A job synced from another host (a remote job) runs on a node. It gets
//! the node's free cores at its start, divided by the jobs that started in
//! the last [`WINDOW_MS`], this one counted: the busy-core figure is a mean
//! of the last seconds and does not show yet what those jobs will use, so
//! they share what it shows as free. The width is at least 1 and at most
//! the node's CPUs. A local job gets no width and keeps the workstation
//! rule (`slot serve --test-threads`).

use std::collections::VecDeque;

/// Set in a remote job's environment: its width, for a test runner that
/// reads its worker count from the environment (a Playwright or Vitest
/// config).
pub const ENV: &str = "SLOT_WIDTH";

/// How far back a start counts as recent: jobs started this long ago or
/// less share the free cores.
pub const WINDOW_MS: i64 = 5_000;

/// The width of a job: `free` cores split over `recent` jobs, at least 1
/// and at most `cpus`.
pub fn width(cpus: u32, free: f64, recent: usize) -> usize {
    let cpus = (cpus as usize).max(1);
    let share = free / recent.max(1) as f64;
    if !share.is_finite() || share < 1.0 {
        return 1;
    }
    (share.floor() as usize).clamp(1, cpus)
}

/// Start times of the jobs that got a width, Unix ms, the oldest first.
#[derive(Debug, Default)]
pub struct Starts(VecDeque<i64>);

impl Starts {
    /// Records a start at `now_ms` and returns the starts within
    /// [`WINDOW_MS`] before it, this one counted.
    pub fn record(&mut self, now_ms: i64) -> usize {
        while self.0.front().is_some_and(|&t| now_ms - t > WINDOW_MS) {
            self.0.pop_front();
        }
        self.0.push_back(now_ms);
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_on_an_idle_64_cpu_node_gets_more_than_16_threads() {
        assert!(width(64, 64.0, 1) > 16, "{}", width(64, 64.0, 1));
        assert_eq!(width(64, 64.0, 1), 64);
    }

    #[test]
    fn jobs_started_at_once_split_the_free_cores() {
        assert_eq!(width(64, 64.0, 2), 32);
        assert_eq!(width(64, 30.5, 3), 10);
        assert_eq!(width(64, 40.0, 1), 40, "busy cores are not free");
    }

    #[test]
    fn a_width_is_between_one_and_the_cpus() {
        assert_eq!(width(64, 0.0, 1), 1, "a full node");
        assert_eq!(width(64, -3.0, 1), 1, "a figure above the CPUs");
        assert_eq!(width(64, 1.0, 9), 1);
        assert_eq!(width(64, 80.0, 1), 64);
        assert_eq!(width(64, 64.0, 0), 64, "no count is one job");
        assert_eq!(width(0, 0.0, 1), 1);
    }

    #[test]
    fn starts_within_the_window_are_counted() {
        let mut s = Starts::default();
        assert_eq!(s.record(1_000), 1);
        assert_eq!(s.record(1_000), 2, "two jobs at once");
        assert_eq!(s.record(1_000 + WINDOW_MS), 3);
        assert_eq!(s.record(1_001 + WINDOW_MS), 2, "the first two left");
    }
}
