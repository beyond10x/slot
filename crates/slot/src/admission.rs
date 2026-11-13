//! Admission by memory and disk: a job waits while the filesystem it builds
//! on is short of space, or while the memory it is expected to need is not
//! available.
//!
//! Expected memory is the 90th percentile of the peak resident memory of
//! earlier runs of the same command in the same repository (at least 3
//! runs), else of the same toolchain (at least 5 runs), else
//! [`DEFAULT_EXPECTED`]. A job alone in the pool runs whatever the memory
//! says: waiting would not free any, so the queue cannot stall on memory.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use crate::types::Job;

/// One gigabyte, 10⁹ bytes.
pub const GB: u64 = 1_000_000_000;

/// Expected peak memory of a job with no history.
pub const DEFAULT_EXPECTED: u64 = GB;

/// Runs kept per command and per toolchain.
const KEEP_PER_COMMAND: usize = 20;
const KEEP_PER_TOOLCHAIN: usize = 200;

/// Limits; 0 turns one off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// Memory that must stay available besides the job's expected peak, bytes.
    pub min_free_memory: u64,
    /// Free space the build's filesystem must keep, bytes.
    pub min_free_disk: u64,
}

/// Peak memory of earlier jobs.
#[derive(Debug, Default)]
pub struct History {
    by_command: HashMap<(String, Vec<String>), VecDeque<u64>>,
    by_toolchain: HashMap<String, VecDeque<u64>>,
    /// Run times of jobs that ended on their own (ok or failed), ms.
    runtime: HashMap<(String, Vec<String>), VecDeque<u64>>,
}

fn p90(v: &VecDeque<u64>) -> u64 {
    let mut s: Vec<u64> = v.iter().copied().collect();
    s.sort_unstable();
    s[(s.len() * 9 / 10).min(s.len() - 1)]
}

impl History {
    /// Adds a finished job; jobs that never ran or have no sample are skipped.
    pub fn record(&mut self, j: &Job) {
        if j.started_ms > 0
            && j.ended_ms >= j.started_ms
            && matches!(
                j.state,
                crate::types::JobState::Ok | crate::types::JobState::Failed
            )
        {
            let r = self
                .runtime
                .entry((j.repo.clone(), j.argv.clone()))
                .or_default();
            r.push_back((j.ended_ms - j.started_ms) as u64);
            if r.len() > KEEP_PER_COMMAND {
                r.pop_front();
            }
        }
        if j.peak_rss == 0 || j.started_ms == 0 {
            return;
        }
        let c = self
            .by_command
            .entry((j.repo.clone(), j.argv.clone()))
            .or_default();
        c.push_back(j.peak_rss);
        if c.len() > KEEP_PER_COMMAND {
            c.pop_front();
        }
        let t = self.by_toolchain.entry(j.toolchain.clone()).or_default();
        t.push_back(j.peak_rss);
        if t.len() > KEEP_PER_TOOLCHAIN {
            t.pop_front();
        }
    }

    /// Expected run time of a command, ms: the 90th percentile of its last
    /// 20 runs that ended on their own, with at least 3; `None` otherwise.
    pub fn expected_runtime(&self, repo: &str, argv: &[String]) -> Option<u64> {
        self.runtime
            .get(&(repo.to_string(), argv.to_vec()))
            .filter(|v| v.len() >= 3)
            .map(p90)
    }

    /// Expected peak memory of a job, bytes.
    pub fn expected(&self, repo: &str, argv: &[String], toolchain: &str) -> u64 {
        if let Some(v) = self.by_command.get(&(repo.to_string(), argv.to_vec()))
            && v.len() >= 3
        {
            return p90(v);
        }
        if let Some(v) = self.by_toolchain.get(toolchain)
            && v.len() >= 5
        {
            return p90(v);
        }
        DEFAULT_EXPECTED
    }
}

/// `MemAvailable` from `/proc/meminfo`, bytes.
pub fn mem_available() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo(&s)
}

/// `MemAvailable` from the text of `/proc/meminfo`, bytes.
pub fn parse_meminfo(s: &str) -> Option<u64> {
    let kb: u64 = s
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kb * 1024)
}

/// Bytes an unprivileged user may still write on the filesystem of `path`
/// or of its nearest existing ancestor.
pub fn disk_free(path: &Path) -> Option<u64> {
    let mut p = Some(path);
    while let Some(q) = p {
        if let Ok(st) = rustix::fs::statvfs(q) {
            return Some(st.f_bavail * st.f_frsize);
        }
        p = q.parent();
    }
    None
}

/// `12.3 GB`.
pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GB as f64)
}

/// Why a job must wait for disk: the first of `free` (path, bytes) below
/// the limit.
pub fn disk_wait(free: &[(String, u64)], limits: Limits) -> Option<String> {
    if limits.min_free_disk == 0 {
        return None;
    }
    free.iter()
        .find(|(_, f)| *f < limits.min_free_disk)
        .map(|(p, f)| {
            format!(
                "{} free on the filesystem of {p}, builds need {}",
                gb(*f),
                gb(limits.min_free_disk)
            )
        })
}

/// Why a job must wait for memory: what is available, less what jobs that
/// started moments ago will still take (`reserved`), is below its expected
/// peak plus the floor. `others` is whether any other job runs; without one
/// the job runs anyway.
pub fn memory_wait(
    available: u64,
    reserved: u64,
    expected: u64,
    limits: Limits,
    others: bool,
) -> Option<String> {
    if limits.min_free_memory == 0 || !others {
        return None;
    }
    let left = available.saturating_sub(reserved);
    (left < expected + limits.min_free_memory).then(|| {
        format!(
            "{} memory available{}, the job needs about {} and {} stays free",
            gb(available),
            if reserved > 0 {
                format!(" ({} promised to jobs that just started)", gb(reserved))
            } else {
                String::new()
            },
            gb(expected),
            gb(limits.min_free_memory)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JobState;

    fn job(repo: &str, cmd: &str, tc: &str, rss: u64) -> Job {
        Job {
            id: 1,
            argv: cmd.split(' ').map(String::from).collect(),
            cwd: "/w".into(),
            repo: repo.into(),
            project: String::new(),
            session: String::new(),
            client: String::new(),
            toolchain: tc.into(),
            remote: false,
            tokens: 1,
            state: JobState::Ok,
            enqueued_ms: 1,
            started_ms: 2,
            ended_ms: 3,
            exit: 0,
            cpu_s: 0.0,
            compile_server_cpu_s: 0.0,
            peak_rss: rss,
            wait_reason: String::new(),
            target_dir: String::new(),
        }
    }

    #[test]
    fn expected_runtime_needs_three_finished_runs() {
        let mut h = History::default();
        let argv = vec!["cargo".to_string(), "test".to_string()];
        let mut j = job("a", "cargo test", "rust", 0);
        assert_eq!(h.expected_runtime("a", &argv), None);
        for ms in [10_000, 20_000] {
            j.ended_ms = j.started_ms + ms;
            h.record(&j);
        }
        assert_eq!(h.expected_runtime("a", &argv), None);
        j.state = JobState::Cancelled;
        j.ended_ms = j.started_ms + 1;
        h.record(&j);
        assert_eq!(
            h.expected_runtime("a", &argv),
            None,
            "a cancelled run says nothing"
        );
        j.state = JobState::Failed;
        j.ended_ms = j.started_ms + 30_000;
        h.record(&j);
        assert_eq!(h.expected_runtime("a", &argv), Some(30_000));
    }

    #[test]
    fn expected_memory_comes_from_the_command_then_the_toolchain() {
        let mut h = History::default();
        let argv = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(
            h.expected("a", &argv("cargo test"), "rust"),
            DEFAULT_EXPECTED
        );
        for gb_ in [1, 2, 3, 4, 5] {
            h.record(&job("b", "cargo build", "rust", gb_ * GB));
        }
        assert_eq!(
            h.expected("a", &argv("cargo test"), "rust"),
            5 * GB,
            "toolchain p90"
        );
        for gb_ in [8, 9, 10] {
            h.record(&job("a", "cargo test", "rust", gb_ * GB));
        }
        assert_eq!(h.expected("a", &argv("cargo test"), "rust"), 10 * GB);
        h.record(&job("a", "cargo test", "rust", 0));
        assert_eq!(
            h.expected("a", &argv("cargo test"), "rust"),
            10 * GB,
            "no sample, not kept"
        );
    }

    #[test]
    fn meminfo() {
        let s = "MemTotal:       65734072 kB\nMemFree:  1 kB\nMemAvailable:   50266548 kB\n";
        assert_eq!(parse_meminfo(s), Some(50_266_548 * 1024));
        assert_eq!(parse_meminfo("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn disk_below_the_limit_waits() {
        let l = Limits {
            min_free_memory: 0,
            min_free_disk: 25 * GB,
        };
        assert_eq!(disk_wait(&[("/w".into(), 30 * GB)], l), None);
        assert_eq!(
            disk_wait(&[("/w".into(), 30 * GB), ("/t".into(), 18 * GB)], l).as_deref(),
            Some("18.0 GB free on the filesystem of /t, builds need 25.0 GB")
        );
        assert_eq!(
            disk_wait(&[("/t".into(), 0)], Limits::default()),
            None,
            "off"
        );
    }

    #[test]
    fn memory_short_waits_unless_the_job_is_alone() {
        let l = Limits {
            min_free_memory: 4 * GB,
            min_free_disk: 0,
        };
        assert_eq!(memory_wait(50 * GB, 0, 4 * GB, l, true), None);
        assert!(memory_wait(7 * GB, 0, 4 * GB, l, true).is_some());
        assert!(
            memory_wait(20 * GB, 14 * GB, 4 * GB, l, true)
                .unwrap()
                .contains("promised to jobs that just started")
        );
        assert_eq!(memory_wait(1, 0, 4 * GB, l, false), None, "alone: runs");
        assert_eq!(
            memory_wait(1, 0, 4 * GB, Limits::default(), true),
            None,
            "off"
        );
    }

    #[test]
    fn disk_free_finds_the_nearest_existing_directory() {
        let d = tempfile::tempdir().unwrap();
        assert!(disk_free(&d.path().join("not/yet/there")).is_some_and(|b| b > 0));
    }
}
