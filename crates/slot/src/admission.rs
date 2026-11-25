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
    /// Free space the filesystem must keep for a job that compiled nothing
    /// in its last [`LIGHT_RUNS`] runs, bytes; at most `min_free_disk`.
    pub min_free_disk_light: u64,
}

/// Runs of a command that must all have compiled nothing before it waits
/// at the light disk floor.
pub const LIGHT_RUNS: usize = 5;

/// CPUs one run used and when it ended, Unix ms.
type CoresRun = (f64, i64);

/// Wall time of one run that ended ok, ms, and its CPU seconds.
type OkRun = (u64, f64);

/// A run with less than this share of the median CPU seconds of its
/// command's ok runs was served from a cache: it is no best run.
const CACHE_HIT_FRACTION: f64 = 0.5;

/// Peak memory of earlier jobs.
#[derive(Debug, Default)]
pub struct History {
    by_command: HashMap<(String, Vec<String>), VecDeque<u64>>,
    by_toolchain: HashMap<String, VecDeque<u64>>,
    /// Run times of jobs that ended on their own (ok or failed), ms.
    runtime: HashMap<(String, Vec<String>), VecDeque<u64>>,
    /// CPUs those jobs used over their run: CPU seconds over run time
    /// (with the compilers a compile cache server ran for them), and the
    /// run's end, Unix ms.
    cores: HashMap<(String, Vec<String>), VecDeque<CoresRun>>,
    /// The runs that ended ok, for the best earlier run of a command.
    ok_runs: HashMap<(String, Vec<String>), VecDeque<OkRun>>,
    /// Whether each of the last [`LIGHT_RUNS`] runs of a command that
    /// started compiled: used a compile server or had a target directory.
    compiled: HashMap<(String, Vec<String>), VecDeque<bool>>,
}

fn p90(v: &VecDeque<u64>) -> u64 {
    let mut s: Vec<u64> = v.iter().copied().collect();
    s.sort_unstable();
    s[(s.len() * 9 / 10).min(s.len() - 1)]
}

impl History {
    /// Adds a finished job; jobs that never ran or have no sample are skipped.
    pub fn record(&mut self, j: &Job) {
        if !j.repo.is_empty() && j.started_ms > 0 && j.ended_ms >= j.started_ms {
            let c = self
                .compiled
                .entry((j.repo.clone(), j.argv.clone()))
                .or_default();
            c.push_back(j.compile_server_cpu_s > 0.0 || !j.target_dir.is_empty());
            if c.len() > LIGHT_RUNS {
                c.pop_front();
            }
        }
        if !j.repo.is_empty()
            && j.started_ms > 0
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
            if j.state == crate::types::JobState::Ok {
                let o = self
                    .ok_runs
                    .entry((j.repo.clone(), j.argv.clone()))
                    .or_default();
                o.push_back(((j.ended_ms - j.started_ms) as u64, j.cpu_s));
                if o.len() > KEEP_PER_COMMAND {
                    o.pop_front();
                }
            }
            // A run under a second says little about the CPUs a job takes.
            let ms = j.ended_ms - j.started_ms;
            if ms >= 1000 {
                let c = self
                    .cores
                    .entry((j.repo.clone(), j.argv.clone()))
                    .or_default();
                c.push_back((j.cpu_s * 1000.0 / ms as f64, j.ended_ms));
                if c.len() > KEEP_PER_COMMAND {
                    c.pop_front();
                }
            }
        }
        if j.peak_rss == 0 || j.started_ms == 0 {
            return;
        }
        if !j.repo.is_empty() {
            let c = self
                .by_command
                .entry((j.repo.clone(), j.argv.clone()))
                .or_default();
            c.push_back(j.peak_rss);
            if c.len() > KEEP_PER_COMMAND {
                c.pop_front();
            }
        }
        let t = self.by_toolchain.entry(j.toolchain.clone()).or_default();
        t.push_back(j.peak_rss);
        if t.len() > KEEP_PER_TOOLCHAIN {
            t.pop_front();
        }
    }

    /// Expected run time of a command, ms: the 90th percentile of its last
    /// 20 runs in this repository that ended on their own, with at least 3.
    /// A command without a repository has no runtime estimate.
    pub fn expected_runtime(&self, repo: &str, argv: &[String]) -> Option<u64> {
        self.runtime
            .get(&(repo.to_string(), argv.to_vec()))
            .filter(|v| v.len() >= 3)
            .map(p90)
    }

    /// CPUs a command is expected to use while it runs: the median of CPU
    /// seconds over run time of its last 20 runs in this repository that
    /// ended on their own and ran a second or longer, with at least 3.
    pub fn expected_cores(&self, repo: &str, argv: &[String]) -> Option<f64> {
        let v = self
            .cores
            .get(&(repo.to_string(), argv.to_vec()))
            .filter(|v| v.len() >= 3)?;
        let mut s: Vec<f64> = v.iter().map(|(c, _)| *c).collect();
        s.sort_by(f64::total_cmp);
        Some(s[s.len() / 2])
    }

    /// The share of the CPUs a command asks for by share at `now_ms`, from
    /// the same runs as [`History::expected_cores`]
    /// ([`crate::shares::predict`]); `None` with fewer than 3.
    pub fn predicted_share(&self, repo: &str, argv: &[String], now_ms: i64) -> Option<f64> {
        let v = self.cores.get(&(repo.to_string(), argv.to_vec()))?;
        let runs: Vec<(f64, i64)> = v.iter().copied().collect();
        crate::shares::predict(&runs, now_ms)
    }

    /// The best earlier run a slowdown is measured against, ms: the
    /// shortest of the kept runs of a command that ended ok and were not
    /// served from a cache (at least [`CACHE_HIT_FRACTION`] of the median
    /// CPU seconds of those runs).
    pub fn best_runtime(&self, repo: &str, argv: &[String]) -> Option<u64> {
        let runs = self.ok_runs.get(&(repo.to_string(), argv.to_vec()))?;
        let mut cpu: Vec<f64> = runs.iter().map(|r| r.1).collect();
        cpu.sort_by(f64::total_cmp);
        let median = *cpu.get(cpu.len() / 2)?;
        runs.iter()
            .filter(|r| r.1 >= CACHE_HIT_FRACTION * median)
            .map(|r| r.0)
            .min()
    }

    /// Whether a command compiled nothing in its last [`LIGHT_RUNS`] runs in
    /// this repository: none used a compile server or had a target
    /// directory. False with fewer runs and without a repository.
    pub fn compiles_nothing(&self, repo: &str, argv: &[String]) -> bool {
        self.compiled
            .get(&(repo.to_string(), argv.to_vec()))
            .is_some_and(|v| v.len() >= LIGHT_RUNS && v.iter().all(|c| !c))
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

/// `MemTotal` from the text of `/proc/meminfo`, bytes.
pub fn parse_meminfo_total(s: &str) -> Option<u64> {
    let kb: u64 = s
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kb * 1024)
}

/// Size of the filesystem of `path` or of its nearest existing ancestor,
/// bytes.
pub fn disk_total(path: &Path) -> Option<u64> {
    let mut p = Some(path);
    while let Some(q) = p {
        if let Ok(st) = rustix::fs::statvfs(q) {
            return Some(st.f_blocks * st.f_frsize);
        }
        p = q.parent();
    }
    None
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
/// its floor. A job that compiled nothing in its last [`LIGHT_RUNS`] runs
/// (`light`) has the light floor, at most the build floor; every other job
/// the build floor. The text names the floor that holds the job.
pub fn disk_wait(free: &[(String, u64)], limits: Limits, light: bool) -> Option<String> {
    let build = limits.min_free_disk;
    if build == 0 {
        return None;
    }
    let light_floor = limits.min_free_disk_light.min(build);
    let (floor, needs) = if light && light_floor < build {
        (
            light_floor,
            format!(
                "this job needs {} (no compile in its last {LIGHT_RUNS} runs)",
                gb(light_floor)
            ),
        )
    } else {
        (build, format!("builds need {}", gb(build)))
    };
    free.iter()
        .find(|(_, f)| *f < floor)
        .map(|(p, f)| format!("{} free on the filesystem of {p}, {needs}", gb(*f)))
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
            checkout: None,
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
            local_reason: String::new(),
            target_dir: String::new(),
            priority: crate::types::Priority::Normal,
            promoted: false,
            cache_env: None,
            warnings: Vec::new(),
            limit_s: None,
            timed_out: false,
            predicted_ms: None,
            collected: 0,
            ahead: false,
            queue_class: None,
            share: None,
            share_used: None,
            lent_s: 0.0,
            admission: crate::shares::Mode::Slots,
            node: String::new(),
            from_node: false,
            sync_ms: None,
            sync_bytes: None,
            back_ms: None,
            compile_ms: None,
            test_ms: None,
            node_target_dir: None,
            seeded_from: None,
            width: None,
            warm: false,
            cancel_reason: String::new(),
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
    fn jobs_without_a_repository_share_only_toolchain_history() {
        let mut h = History::default();
        let j = job("", "cargo test", "rust", 5 * GB);
        for _ in 0..3 {
            h.record(&j);
        }
        assert_eq!(h.expected_runtime("", &j.argv), None);
        assert_eq!(h.expected("", &j.argv, "rust"), DEFAULT_EXPECTED);
        for _ in 0..2 {
            h.record(&j);
        }
        assert_eq!(h.expected("", &j.argv, "rust"), 5 * GB);
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
            min_free_disk_light: 0,
        };
        assert_eq!(disk_wait(&[("/w".into(), 30 * GB)], l, false), None);
        assert_eq!(
            disk_wait(&[("/w".into(), 30 * GB), ("/t".into(), 18 * GB)], l, false).as_deref(),
            Some("18.0 GB free on the filesystem of /t, builds need 25.0 GB")
        );
        assert_eq!(
            disk_wait(&[("/t".into(), 0)], Limits::default(), false),
            None,
            "off"
        );
    }

    /// A job that compiled nothing in its last 5 runs waits at the light
    /// floor and is told so; no job starts below it; the light floor is at
    /// most the build floor, and equal to it a light job reads as a build.
    #[test]
    fn a_light_job_waits_at_the_light_floor_and_none_starts_below_it() {
        let l = Limits {
            min_free_memory: 0,
            min_free_disk: 40 * GB,
            min_free_disk_light: 10 * GB,
        };
        let at = |f: u64, light: bool, l: Limits| disk_wait(&[("/w".into(), f)], l, light);
        assert_eq!(at(20 * GB, true, l), None, "light: starts at 20 GB");
        assert_eq!(
            at(20 * GB, false, l).as_deref(),
            Some("20.0 GB free on the filesystem of /w, builds need 40.0 GB")
        );
        assert_eq!(
            at(5 * GB, true, l).as_deref(),
            Some(
                "5.0 GB free on the filesystem of /w, \
                 this job needs 10.0 GB (no compile in its last 5 runs)"
            ),
            "the hard floor"
        );
        let same = Limits {
            min_free_disk_light: 40 * GB,
            ..l
        };
        assert_eq!(
            at(20 * GB, true, same).as_deref(),
            Some("20.0 GB free on the filesystem of /w, builds need 40.0 GB"),
            "the default: as before"
        );
        let above = Limits {
            min_free_disk_light: 60 * GB,
            ..l
        };
        assert_eq!(at(45 * GB, true, above), None, "at most the build floor");
        let off = Limits {
            min_free_disk: 0,
            ..l
        };
        assert_eq!(at(1, true, off), None, "the build floor 0 turns both off");
    }

    /// A command compiles nothing after 5 runs in its repository with no
    /// compile server CPU and no target directory; fewer runs, a compile or
    /// a target directory among the last 5, or no repository say it may.
    #[test]
    fn compiles_nothing_needs_5_runs_without_a_compile() {
        let mut h = History::default();
        let j = job("a", "task test", "other", 0);
        assert!(!h.compiles_nothing("a", &j.argv), "a first run");
        for _ in 0..4 {
            h.record(&j);
        }
        assert!(!h.compiles_nothing("a", &j.argv), "4 runs");
        h.record(&j);
        assert!(h.compiles_nothing("a", &j.argv), "5 runs");
        assert!(!h.compiles_nothing("b", &j.argv), "another repository");

        let mut built = j.clone();
        built.compile_server_cpu_s = 0.5;
        h.record(&built);
        assert!(!h.compiles_nothing("a", &j.argv), "a compile in the last 5");
        for _ in 0..4 {
            h.record(&j);
        }
        assert!(!h.compiles_nothing("a", &j.argv), "still in the last 5");
        h.record(&j);
        assert!(h.compiles_nothing("a", &j.argv), "out of the last 5");

        let mut targeted = j.clone();
        targeted.target_dir = "/w/target".into();
        h.record(&targeted);
        assert!(!h.compiles_nothing("a", &j.argv), "a target directory");

        let mut queued = job("q", "task test", "other", 0);
        queued.started_ms = 0;
        for _ in 0..5 {
            h.record(&queued);
        }
        assert!(!h.compiles_nothing("q", &queued.argv), "never started");
        let none = job("", "task test", "other", 0);
        for _ in 0..5 {
            h.record(&none);
        }
        assert!(!h.compiles_nothing("", &none.argv), "no repository");
    }

    #[test]
    fn memory_short_waits_unless_the_job_is_alone() {
        let l = Limits {
            min_free_memory: 4 * GB,
            min_free_disk: 0,
            min_free_disk_light: 0,
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

    #[test]
    fn expected_cores_are_the_median_of_three_runs_of_a_second_or_longer() {
        let mut h = History::default();
        let argv = vec!["cargo".to_string(), "test".to_string()];
        let mut j = job("a", "cargo test", "rust", 0);
        assert_eq!(h.expected_cores("a", &argv), None);
        for (cpu_s, ms) in [(8.0, 2_000), (30.0, 10_000)] {
            j.cpu_s = cpu_s;
            j.ended_ms = j.started_ms + ms;
            h.record(&j);
        }
        assert_eq!(h.expected_cores("a", &argv), None, "two runs");
        j.cpu_s = 50.0;
        j.ended_ms = j.started_ms + 500;
        h.record(&j);
        assert_eq!(
            h.expected_cores("a", &argv),
            None,
            "a run under a second says nothing"
        );
        j.state = JobState::Cancelled;
        j.cpu_s = 9.0;
        j.ended_ms = j.started_ms + 3_000;
        h.record(&j);
        assert_eq!(
            h.expected_cores("a", &argv),
            None,
            "a cancelled run says nothing"
        );
        j.state = JobState::Failed;
        j.cpu_s = 2.0;
        j.ended_ms = j.started_ms + 4_000;
        h.record(&j);
        assert_eq!(
            h.expected_cores("a", &argv),
            Some(3.0),
            "the median of 4.0, 3.0 and 0.5 CPUs"
        );
        assert_eq!(h.expected_cores("b", &argv), None, "another repository");
    }

    /// A rust job's record counts the compilers the compile cache server
    /// ran for it in its CPU seconds, so its share counts them too.
    #[test]
    fn a_share_comes_from_the_runs_with_their_compile_cache_work_and_their_age() {
        let mut h = History::default();
        let argv = vec!["cargo".to_string(), "build".to_string()];
        let mut j = job("a", "cargo build", "rust", 0);
        assert_eq!(h.predicted_share("a", &argv, 0), None);
        assert_eq!(h.best_runtime("a", &argv), None);
        for (cpu_s, ms) in [(20.0, 10_000), (16.0, 8_000), (30.0, 15_000)] {
            j.cpu_s = cpu_s;
            j.compile_server_cpu_s = cpu_s / 2.0;
            j.ended_ms = j.started_ms + ms;
            h.record(&j);
        }
        let share = h.predicted_share("a", &argv, j.ended_ms).unwrap();
        assert!((share - 2.3).abs() < 1e-9, "2 CPUs plus 15 %: {share}");
        assert_eq!(h.best_runtime("a", &argv), Some(8_000));
        assert_eq!(h.predicted_share("b", &argv, 0), None, "another repository");
    }

    /// The best earlier run a slowdown is measured against is a run that
    /// ended ok and did the command's usual work: a failed run that ended
    /// early and a run served from a cache (under half the median CPU
    /// seconds of the command's ok runs) would make every later run look
    /// slow.
    #[test]
    fn the_best_run_is_an_ok_run_that_was_not_served_from_a_cache() {
        let mut h = History::default();
        let argv = vec!["cargo".to_string(), "test".to_string()];
        let mut j = job("a", "cargo test", "rust", 0);
        for (cpu_s, ms) in [(20.0, 10_000), (16.0, 8_000), (30.0, 15_000)] {
            j.cpu_s = cpu_s;
            j.ended_ms = j.started_ms + ms;
            h.record(&j);
        }
        j.cpu_s = 2.0;
        j.ended_ms = j.started_ms + 2_000;
        h.record(&j);
        assert_eq!(h.best_runtime("a", &argv), Some(8_000), "a cache hit");
        j.state = JobState::Failed;
        j.cpu_s = 12.0;
        j.ended_ms = j.started_ms + 3_000;
        h.record(&j);
        assert_eq!(h.best_runtime("a", &argv), Some(8_000), "a failed run");
        j.state = JobState::Ok;
        j.cpu_s = 15.0;
        j.ended_ms = j.started_ms + 7_000;
        h.record(&j);
        assert_eq!(h.best_runtime("a", &argv), Some(7_000));
    }
}
