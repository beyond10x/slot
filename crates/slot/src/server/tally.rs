//! The jobs this node ended over the last 25 hours, by result, read once
//! from the job log at the start and kept up as jobs end: what the node's
//! row reports as `ended_hour` and `ended_hours`, so the page counts every
//! node over the same windows whatever number of finished jobs it lists.

use std::collections::VecDeque;

use crate::types::{HourOutcomes, Job, LOCAL_NODE, Outcome, Outcomes};

const HOUR_MS: i64 = 3_600_000;

/// How far back the tally keeps jobs: 24 hours and the hour that runs.
const KEEP_MS: i64 = 25 * HOUR_MS;

/// Ended jobs of this node: end time and result, in the order they ended.
#[derive(Debug, Default)]
pub(super) struct Tally {
    ended: VecDeque<(i64, Outcome)>,
}

impl Tally {
    /// Counts job `j` if it ended on this node (not on a node this daemon
    /// handed it to) within the kept time before `now`.
    pub(super) fn record(&mut self, j: &Job, now: i64) {
        if !(j.node.is_empty() || j.node == LOCAL_NODE) || j.ended_ms <= 0 {
            return;
        }
        let Some(o) = Outcome::of(j) else {
            return;
        };
        if j.ended_ms < now - KEEP_MS {
            return;
        }
        self.ended.push_back((j.ended_ms, o));
        while self.ended.front().is_some_and(|(t, _)| *t < now - KEEP_MS) {
            self.ended.pop_front();
        }
    }

    /// The jobs that ended in the 60 minutes before `now`.
    pub(super) fn last_hour(&self, now: i64) -> Outcomes {
        let mut n = Outcomes::default();
        for (t, o) in &self.ended {
            if *t > now - HOUR_MS && *t <= now {
                n.add(*o);
            }
        }
        n
    }

    /// The jobs that ended per clock hour of the 25 hours up to `now`,
    /// oldest first, hours without a job left out.
    pub(super) fn hours(&self, now: i64) -> Vec<HourOutcomes> {
        let first = now - now.rem_euclid(HOUR_MS) - 24 * HOUR_MS;
        let mut out: Vec<HourOutcomes> = Vec::new();
        for (t, o) in &self.ended {
            if *t < first || *t > now {
                continue;
            }
            let h = t - t.rem_euclid(HOUR_MS);
            let at = match out.binary_search_by_key(&h, |x| x.t) {
                Ok(i) => i,
                Err(i) => {
                    out.insert(
                        i,
                        HourOutcomes {
                            t: h,
                            n: Outcomes::default(),
                        },
                    );
                    i
                }
            };
            out[at].n.add(*o);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JobState;

    fn job(ended_ms: i64, state: JobState, exit: i32, node: &str) -> Job {
        let mut j: Job = serde_json::from_str(
            r#"{"id":1,"argv":["true"],"cwd":"/w","repo":"r","toolchain":"other",
            "remote":false,"tokens":1,"state":"ok","enqueued_ms":1,"started_ms":1,
            "ended_ms":1,"exit":0,"cpu_s":0.0,"compile_server_cpu_s":0.0,"peak_rss":0}"#,
        )
        .unwrap();
        j.ended_ms = ended_ms;
        j.state = state;
        j.exit = exit;
        j.node = node.to_string();
        j
    }

    #[test]
    fn hours_and_the_last_hour_split_failures_by_cause() {
        let now = 1_800_000_000_000 + 30 * 60_000;
        let mut t = Tally::default();
        t.record(&job(now - 60_000, JobState::Ok, 0, ""), now);
        t.record(&job(now - 120_000, JobState::Failed, 124, "local"), now);
        let mut timed = job(now - 130_000, JobState::Failed, 9, "local");
        timed.timed_out = true;
        t.record(&timed, now);
        t.record(&job(now - 2 * HOUR_MS, JobState::Failed, 101, "local"), now);
        t.record(
            &job(now - 2 * HOUR_MS, JobState::Failed, 127, "node-a"),
            now,
        );
        t.record(&job(now - 26 * HOUR_MS, JobState::Ok, 0, "local"), now);
        t.record(&job(0, JobState::Running, 0, "local"), now);
        let h = t.last_hour(now);
        assert_eq!((h.ok, h.timeout, h.test, h.infra), (1, 2, 0, 0));
        let hours = t.hours(now);
        assert_eq!(hours.len(), 2);
        assert_eq!(hours[0].t, now - now % HOUR_MS - 2 * HOUR_MS);
        assert_eq!(hours[0].n.test, 1);
        assert_eq!(hours[1].n.ok + hours[1].n.timeout, 3);
    }

    #[test]
    fn jobs_older_than_25_hours_are_dropped() {
        let now = 1_800_000_000_000;
        let mut t = Tally::default();
        t.record(
            &job(now - 30 * HOUR_MS, JobState::Ok, 0, "local"),
            now - 6 * HOUR_MS,
        );
        t.record(&job(now, JobState::Ok, 0, "local"), now);
        assert_eq!(t.ended.len(), 1);
    }
}
