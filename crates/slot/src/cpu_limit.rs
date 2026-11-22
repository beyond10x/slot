//! Admission by the host's busy CPUs (`slot cpu-limit`).
//!
//! Slots bound what the pool starts, not what its work uses: a compile cache
//! server compiles in a cgroup of its own, and a job may use more CPUs than
//! it holds slots. With a mark set, a queued job that could start is held
//! while the pool and the compile cache use more CPUs than the mark and at
//! least `floor` jobs of the pool run. Work outside both does not hold it:
//! the pool cannot make room for that by holding its own jobs.
//!
//! The figure is the census sampler's: the CPU time of the pool's processes
//! and of the compile cache servers', not of work outside both ([`Parts`]),
//! averaged over its last [`WINDOW`] samples ([`RecentParts`]). A job that
//! started moments ago is not in that average yet, so it counts with the
//! CPUs it is expected to use, less what it already shows ([`Load::starting`]):
//! without that every held job would start in the moment the figure dips
//! under the mark. A job held longer than [`HOLD_MAX`] starts whatever the
//! figure says ([`Verdict::Overdue`]).

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

/// Jobs that may always run while a mark is set and no floor was named.
pub const DEFAULT_FLOOR: usize = 4;

/// The longest a job is held for the host's CPUs; then it starts.
pub const HOLD_MAX: Duration = Duration::from_secs(600);

/// Samples of the census (one a second) the busy figure is the mean of.
pub const WINDOW: usize = 10;

/// A figure whose last sample is older than this is no figure: the sampler
/// stalled, and the hold does not apply.
pub const STALE_MS: i64 = 5_000;

/// A job that started this recently counts with the CPUs it is expected to
/// use: the window has to fill, and a build reads its configuration before
/// it compiles.
pub const STARTING_MS: i64 = 30_000;

/// A daemon this young waits for its first samples before it starts a job
/// past the floor, so a reload does not start every held job at once.
pub const GRACE_MS: i64 = 5_000;

/// CPUs a job with no history is expected to use.
pub const DEFAULT_CORES: f64 = 1.0;

/// The mark and the floor, as saved in `limits.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Limits {
    /// Busy CPUs of the host from which a start is held; `None` = no limit.
    pub cpu_busy_max: Option<f64>,
    /// Jobs that may always run; `None` = [`DEFAULT_FLOOR`].
    pub cpu_busy_floor: Option<usize>,
}

/// Whether `mark` is a mark: a finite number of CPUs from 1.
fn valid(mark: f64) -> bool {
    mark.is_finite() && mark >= 1.0
}

impl Limits {
    /// What a `limits.json` holds, without values no request would set.
    pub fn loaded(max: Option<f64>, floor: Option<usize>) -> Self {
        let cpu_busy_max = max.filter(|&m| valid(m));
        Self {
            cpu_busy_max,
            cpu_busy_floor: floor.filter(|&f| f > 0 && cpu_busy_max.is_some()),
        }
    }

    /// Jobs that may always run.
    pub fn floor(&self) -> usize {
        self.cpu_busy_floor.unwrap_or(DEFAULT_FLOOR)
    }

    /// The limits after `req`, or why it is refused.
    pub fn changed(&self, req: &Request) -> Result<Self, String> {
        if req.cpu_busy_max.is_none() && req.cpu_busy_floor.is_none() {
            return Err(
                "slot: name cpu_busy_max (CPUs, or null for no limit), cpu_busy_floor, or both"
                    .into(),
            );
        }
        let max = match req.cpu_busy_max {
            None => self.cpu_busy_max,
            Some(None) => None,
            Some(Some(m)) if valid(m) => Some(m),
            Some(Some(_)) => {
                return Err(
                    "slot: cpu_busy_max must be a number of CPUs from 1; null turns the limit off"
                        .into(),
                );
            }
        };
        match req.cpu_busy_floor {
            Some(0) => return Err("slot: cpu_busy_floor must be at least 1".into()),
            Some(_) if max.is_none() => {
                return Err(
                    "slot: cpu_busy_floor needs cpu_busy_max: there is no CPU limit to keep a floor under"
                        .into(),
                );
            }
            _ => {}
        }
        Ok(Self {
            cpu_busy_max: max,
            cpu_busy_floor: max.and(req.cpu_busy_floor.or(self.cpu_busy_floor)),
        })
    }

    /// The limits as `GET /api/cpu-limit` answers, on a host of `cpus` CPUs
    /// that is as busy as `parts` now, with `held` jobs held.
    pub fn view(&self, cpus: u32, parts: Option<Parts>, held: usize) -> View {
        View {
            cpu_busy_max: self.cpu_busy_max,
            cpu_busy_floor: self.cpu_busy_max.map(|_| self.floor()),
            cpus,
            busy: parts.map(|p| p.host()),
            held,
            pool: parts.map(|p| p.pool),
            compile_cache: parts.map(|p| p.compile_cache),
            outside: parts.map(|p| p.outside),
        }
    }
}

/// Body of `PUT /api/cpu-limit`; a field left out is not changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Busy CPUs of the host from which a start is held, at least 1; `null`
    /// for no limit.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub cpu_busy_max: Option<Option<f64>>,
    /// Jobs that may always run, at least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_busy_floor: Option<usize>,
}

/// A field that tells `null` (`Some(None)`) from left out (`None`).
fn named<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// Answer of `GET` and `PUT /api/cpu-limit`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// Busy CPUs of the host from which a start is held; `None` = no limit.
    #[serde(default)]
    pub cpu_busy_max: Option<f64>,
    /// Jobs that may always run, while a mark is set.
    #[serde(default)]
    pub cpu_busy_floor: Option<usize>,
    /// CPUs of the host. A mark above them never holds a start.
    #[serde(default)]
    pub cpus: u32,
    /// Busy CPUs of the host as the daemon last sampled them; `None` before
    /// its first sample.
    #[serde(default)]
    pub busy: Option<f64>,
    /// Queued jobs held for the host's CPUs now.
    #[serde(default)]
    pub held: usize,
    /// Busy CPUs of the pool's jobs, with the compilers a compile cache
    /// server runs for them: counted against the mark. `None` before the
    /// first sample and from a daemon that predates the parts.
    #[serde(default)]
    pub pool: Option<f64>,
    /// Busy CPUs of the rest of the compile cache servers: counted against
    /// the mark.
    #[serde(default)]
    pub compile_cache: Option<f64>,
    /// Busy CPUs outside the pool and the compile cache: not counted.
    #[serde(default)]
    pub outside: Option<f64>,
}

/// The argument of `slot cpu-limit`: a number of CPUs, or `off`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Setting {
    /// No limit.
    Off,
    /// Busy CPUs of the host from which a start is held.
    At(f64),
}

impl Setting {
    /// The value the API carries.
    pub fn value(self) -> Option<f64> {
        match self {
            Self::Off => None,
            Self::At(m) => Some(m),
        }
    }
}

impl std::str::FromStr for Setting {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        if s == "off" {
            return Ok(Self::Off);
        }
        match s.parse::<f64>() {
            Ok(m) if valid(m) => Ok(Self::At(m)),
            Ok(_) => Err("the mark is a number of CPUs from 1; `off` for no limit".into()),
            Err(_) => Err(format!(
                "`{s}` is not a number of CPUs: give one from 1, such as 17 or 17.5, or `off`"
            )),
        }
    }
}

/// The last samples of a figure in CPUs, one a second.
#[derive(Debug, Clone, Default)]
pub struct Recent {
    samples: VecDeque<f64>,
    at_ms: i64,
}

impl Recent {
    /// Adds the sample taken at `now_ms`; keeps the last [`WINDOW`].
    pub fn push(&mut self, cores: f64, now_ms: i64) {
        self.samples.push_back(cores);
        if self.samples.len() > WINDOW {
            self.samples.pop_front();
        }
        self.at_ms = now_ms;
    }

    /// Samples kept.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether no sample was taken yet.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The mean of the samples; `None` without one, or when the last is
    /// older than [`STALE_MS`] at `now_ms`.
    pub fn mean(&self, now_ms: i64) -> Option<f64> {
        if self.samples.is_empty() || now_ms - self.at_ms > STALE_MS {
            return None;
        }
        Some(self.samples.iter().sum::<f64>() / self.samples.len() as f64)
    }

    /// What these samples add to a mean over the last `n` samples of the
    /// host: a job that started 3 s ago has 3 samples, and adds their sum
    /// over `n`.
    pub fn share(&self, n: usize) -> f64 {
        let n = n.clamp(1, WINDOW);
        self.samples.iter().rev().take(n).sum::<f64>() / n as f64
    }
}

/// Busy CPUs of the host in three parts that do not overlap
/// ([`crate::census::Sample::parts`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Parts {
    /// The processes of the pool's running jobs, with the compilers a
    /// compile cache server runs for them.
    pub pool: f64,
    /// The rest of the compile cache servers' processes.
    pub compile_cache: f64,
    /// Everything else on the host.
    pub outside: f64,
}

impl Parts {
    /// What the limit counts: the pool and the compile cache.
    pub fn counted(&self) -> f64 {
        self.pool + self.compile_cache
    }

    /// Busy CPUs of the whole host.
    pub fn host(&self) -> f64 {
        self.pool + self.compile_cache + self.outside
    }
}

/// The last samples of each part, one a second.
#[derive(Debug, Clone, Default)]
pub struct RecentParts {
    pool: Recent,
    compile_cache: Recent,
    outside: Recent,
}

impl RecentParts {
    /// Adds the sample taken at `now_ms`; keeps the last [`WINDOW`].
    pub fn push(&mut self, p: Parts, now_ms: i64) {
        self.pool.push(p.pool, now_ms);
        self.compile_cache.push(p.compile_cache, now_ms);
        self.outside.push(p.outside, now_ms);
    }

    /// Samples kept.
    pub fn len(&self) -> usize {
        self.pool.len()
    }

    /// Whether no sample was taken yet.
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }

    /// The mean of each part, as [`Recent::mean`].
    pub fn mean(&self, now_ms: i64) -> Option<Parts> {
        Some(Parts {
            pool: self.pool.mean(now_ms)?,
            compile_cache: self.compile_cache.mean(now_ms)?,
            outside: self.outside.mean(now_ms)?,
        })
    }
}

/// What the daemon knows of the host's busy CPUs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Figure {
    /// Busy CPUs of the host, in parts.
    Busy(Parts),
    /// The daemon started moments ago ([`GRACE_MS`]) and has no sample yet.
    Pending,
    /// No sample: the hold does not apply.
    Unknown,
}

/// What a queued job is weighed against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Load {
    /// The host's busy CPUs.
    pub figure: Figure,
    /// CPUs of the host.
    pub cpus: u32,
    /// Other jobs of the pool that run or are about to.
    pub running: usize,
    /// CPUs that jobs started in the last [`STARTING_MS`] are expected to
    /// use and do not show in the figure yet.
    pub starting: f64,
    /// CPUs expected of the jobs held for CPU that are served before this
    /// one, so the held jobs start in the queue's order.
    pub ahead: f64,
    /// How long this job has been held for CPU, ms; `None` when it was not.
    pub held_ms: Option<i64>,
}

/// Whether a queued job may start as far as the host's CPUs go.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Nothing holds it.
    Free,
    /// Held; the text is the job's wait reason.
    Held(String),
    /// It would be held, and has been for `hold_max`: it starts.
    Overdue,
}

/// Weighs one queued job: held while at least `floor` jobs run and the
/// busy CPUs of the pool and the compile cache ([`Parts::counted`]), with
/// what is expected of jobs that just started and of held jobs ahead of it,
/// reach the mark. CPUs outside both are not counted.
pub fn verdict(limits: Limits, load: &Load, hold_max: Duration) -> Verdict {
    let Some(mark) = limits.cpu_busy_max else {
        return Verdict::Free;
    };
    if load.running < limits.floor() || mark > f64::from(load.cpus) {
        return Verdict::Free;
    }
    let text = match load.figure {
        Figure::Unknown => return Verdict::Free,
        Figure::Pending => "host busy: no sample of its CPUs yet".to_string(),
        Figure::Busy(parts) => {
            if parts.counted() + load.starting + load.ahead < mark {
                return Verdict::Free;
            }
            // The `host busy` prefix is what the web view and status read.
            let mut text = format!(
                "host busy: pool {:.1} + compile cache {:.1} of {} CPUs",
                parts.pool, parts.compile_cache, load.cpus
            );
            if load.starting >= 0.05 {
                let _ = write!(text, " + {:.1} for jobs that just started", load.starting);
            }
            if load.ahead >= 0.05 {
                let _ = write!(text, " + {:.1} for held jobs ahead", load.ahead);
            }
            let _ = write!(text, ", mark {mark:.1}");
            text
        }
    };
    let max = i64::try_from(hold_max.as_millis()).unwrap_or(i64::MAX);
    if load.held_ms.is_some_and(|h| h >= max) {
        Verdict::Overdue
    } else {
        Verdict::Held(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(mark: f64, floor: Option<usize>) -> Limits {
        Limits {
            cpu_busy_max: Some(mark),
            cpu_busy_floor: floor,
        }
    }

    fn load(busy: f64, running: usize) -> Load {
        Load {
            figure: Figure::Busy(Parts {
                pool: busy,
                ..Parts::default()
            }),
            cpus: 20,
            running,
            starting: 0.0,
            ahead: 0.0,
            held_ms: None,
        }
    }

    fn held(text: &str) -> Verdict {
        Verdict::Held(text.into())
    }

    #[test]
    fn the_pool_and_the_compile_cache_count_and_work_outside_them_does_not() {
        let l = at(17.0, Some(6));
        let mut d = load(0.0, 6);
        d.figure = Figure::Busy(Parts {
            pool: 3.0,
            compile_cache: 2.0,
            outside: 10.0,
        });
        assert_eq!(verdict(l, &d, HOLD_MAX), Verdict::Free, "5 of 17");
        d.figure = Figure::Busy(Parts {
            pool: 4.0,
            compile_cache: 2.0,
            outside: 14.0,
        });
        assert_eq!(
            verdict(l, &d, HOLD_MAX),
            Verdict::Free,
            "a saturated host, 6 of it the pool's"
        );
        d.figure = Figure::Busy(Parts {
            pool: 12.0,
            compile_cache: 6.0,
            outside: 0.0,
        });
        assert_eq!(
            verdict(l, &d, HOLD_MAX),
            held("host busy: pool 12.0 + compile cache 6.0 of 20 CPUs, mark 17.0")
        );
        d.running = 5;
        assert_eq!(verdict(l, &d, HOLD_MAX), Verdict::Free, "under the floor");
    }

    #[test]
    fn the_parts_window_is_the_mean_of_each_part_and_goes_stale() {
        let mut r = RecentParts::default();
        assert!(r.is_empty());
        assert_eq!(r.mean(0), None);
        let p = |pool, compile_cache, outside| Parts {
            pool,
            compile_cache,
            outside,
        };
        r.push(p(4.0, 2.0, 1.0), 1_000);
        r.push(p(8.0, 4.0, 3.0), 2_000);
        assert_eq!(r.len(), 2);
        assert_eq!(r.mean(2_000), Some(p(6.0, 3.0, 2.0)));
        assert_eq!(r.mean(2_001 + STALE_MS), None);
        assert_eq!(p(6.0, 3.0, 2.0).counted(), 9.0);
        assert_eq!(p(6.0, 3.0, 2.0).host(), 11.0);
    }

    #[test]
    fn a_start_is_held_above_the_mark_once_the_floor_runs() {
        let l = at(17.0, None);
        assert_eq!(l.floor(), 4, "the floor nobody named");
        assert_eq!(
            verdict(l, &load(18.4, 4), HOLD_MAX),
            held("host busy: pool 18.4 + compile cache 0.0 of 20 CPUs, mark 17.0")
        );
        assert_eq!(
            verdict(l, &load(17.0, 9), HOLD_MAX),
            held("host busy: pool 17.0 + compile cache 0.0 of 20 CPUs, mark 17.0"),
            "at the mark"
        );
        assert_eq!(
            verdict(l, &load(18.4, 3), HOLD_MAX),
            Verdict::Free,
            "under the floor"
        );
        assert_eq!(
            verdict(l, &load(16.9, 30), HOLD_MAX),
            Verdict::Free,
            "under the mark"
        );
        assert_eq!(
            verdict(at(17.0, Some(1)), &load(18.4, 1), HOLD_MAX),
            held("host busy: pool 18.4 + compile cache 0.0 of 20 CPUs, mark 17.0"),
            "a floor of 1"
        );
        assert_eq!(
            verdict(Limits::default(), &load(20.0, 30), HOLD_MAX),
            Verdict::Free,
            "off"
        );
    }

    #[test]
    fn jobs_that_just_started_and_held_jobs_ahead_count_before_the_figure_shows_them() {
        let l = at(17.0, None);
        let mut d = load(14.5, 4);
        assert_eq!(verdict(l, &d, HOLD_MAX), Verdict::Free);
        d.starting = 2.0;
        assert_eq!(verdict(l, &d, HOLD_MAX), Verdict::Free, "16.5");
        d.starting = 3.0;
        assert_eq!(
            verdict(l, &d, HOLD_MAX),
            held(
                "host busy: pool 14.5 + compile cache 0.0 of 20 CPUs + 3.0 for jobs that just started, mark 17.0"
            )
        );
        d.starting = 1.0;
        d.ahead = 2.0;
        assert_eq!(
            verdict(l, &d, HOLD_MAX),
            held(
                "host busy: pool 14.5 + compile cache 0.0 of 20 CPUs + 1.0 for jobs that just started + 2.0 for held jobs ahead, mark 17.0"
            )
        );
    }

    #[test]
    fn a_mark_above_the_hosts_cpus_never_holds_and_no_figure_holds_nothing() {
        assert_eq!(
            verdict(at(25.0, Some(1)), &load(20.0, 9), HOLD_MAX),
            Verdict::Free,
            "20 CPUs never reach 25"
        );
        assert_eq!(
            verdict(at(20.0, Some(1)), &load(20.0, 9), HOLD_MAX),
            held("host busy: pool 20.0 + compile cache 0.0 of 20 CPUs, mark 20.0"),
            "the CPU count itself can be reached"
        );
        let mut d = load(0.0, 9);
        d.figure = Figure::Unknown;
        assert_eq!(verdict(at(2.0, None), &d, HOLD_MAX), Verdict::Free);
        d.figure = Figure::Pending;
        assert_eq!(
            verdict(at(2.0, None), &d, HOLD_MAX),
            held("host busy: no sample of its CPUs yet"),
            "a daemon that just started waits for its first sample"
        );
        d.running = 3;
        assert_eq!(
            verdict(at(2.0, None), &d, HOLD_MAX),
            Verdict::Free,
            "the floor starts meanwhile"
        );
    }

    #[test]
    fn a_job_held_for_the_bound_starts() {
        let l = at(17.0, None);
        let mut d = load(19.0, 8);
        d.held_ms = Some(599_999);
        assert!(matches!(verdict(l, &d, HOLD_MAX), Verdict::Held(_)));
        d.held_ms = Some(600_000);
        assert_eq!(verdict(l, &d, HOLD_MAX), Verdict::Overdue);
        d.figure = Figure::Busy(Parts {
            pool: 3.0,
            ..Parts::default()
        });
        assert_eq!(
            verdict(l, &d, HOLD_MAX),
            Verdict::Free,
            "not overdue when nothing holds it"
        );
    }

    #[test]
    fn the_figure_is_the_mean_of_the_last_ten_samples_and_goes_stale() {
        let mut r = Recent::default();
        assert_eq!(r.mean(0), None, "no sample");
        r.push(4.0, 1_000);
        r.push(8.0, 2_000);
        assert_eq!(r.mean(2_000), Some(6.0));
        assert_eq!(r.mean(2_000 + STALE_MS), Some(6.0));
        assert_eq!(r.mean(2_001 + STALE_MS), None, "the sampler stalled");
        for i in 0..20 {
            r.push(10.0, 3_000 + i);
        }
        assert_eq!(r.len(), WINDOW);
        assert_eq!(r.mean(3_019), Some(10.0), "the two first samples left");
    }

    #[test]
    fn a_young_job_adds_its_samples_over_the_hosts_window() {
        let mut job = Recent::default();
        assert_eq!(job.share(10), 0.0);
        job.push(2.0, 0);
        job.push(4.0, 0);
        job.push(6.0, 0);
        assert!((job.share(10) - 1.2).abs() < 1e-9, "12 CPU-seconds in 10 s");
        assert!(
            (job.share(2) - 5.0).abs() < 1e-9,
            "a host window of 2 samples"
        );
        assert!((job.share(0) - 6.0).abs() < 1e-9, "never over nothing");
    }

    #[test]
    fn the_argument_is_a_number_of_cpus_from_one_or_off() {
        assert_eq!("off".parse(), Ok(Setting::Off));
        assert_eq!("17".parse(), Ok(Setting::At(17.0)));
        assert_eq!("17.5".parse(), Ok(Setting::At(17.5)));
        assert_eq!("1".parse(), Ok(Setting::At(1.0)));
        assert_eq!(Setting::Off.value(), None);
        assert_eq!(Setting::At(2.5).value(), Some(2.5));
        for bad in ["0", "0.9", "-3", "nan", "inf", "-inf", "many", "", "Off"] {
            assert!(bad.parse::<Setting>().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_request_names_the_mark_the_floor_or_both() {
        let req = |body: &str| serde_json::from_str::<Request>(body).unwrap();
        let off = Limits::default();
        let on = off.changed(&req(r#"{"cpu_busy_max":17}"#)).unwrap();
        assert_eq!(on, at(17.0, None));
        assert_eq!(on.floor(), DEFAULT_FLOOR);
        let six = on.changed(&req(r#"{"cpu_busy_floor":6}"#)).unwrap();
        assert_eq!(six, at(17.0, Some(6)), "the floor alone keeps the mark");
        assert_eq!(
            six.changed(&req(r#"{"cpu_busy_max":12.5}"#)).unwrap(),
            at(12.5, Some(6)),
            "the mark alone keeps the floor"
        );
        assert_eq!(
            off.changed(&req(r#"{"cpu_busy_max":8,"cpu_busy_floor":2}"#))
                .unwrap(),
            at(8.0, Some(2))
        );
        assert_eq!(
            six.changed(&req(r#"{"cpu_busy_max":null}"#)).unwrap(),
            off,
            "null switches it off and drops the floor"
        );
        for bad in [
            "{}",
            r#"{"cpu_busy_max":0}"#,
            r#"{"cpu_busy_max":0.99}"#,
            r#"{"cpu_busy_max":-4}"#,
            r#"{"cpu_busy_max":17,"cpu_busy_floor":0}"#,
            r#"{"cpu_busy_floor":3}"#,
            r#"{"cpu_busy_max":null,"cpu_busy_floor":3}"#,
        ] {
            let e = off.changed(&req(bad)).unwrap_err();
            assert!(e.starts_with("slot: "), "{bad}: {e}");
        }
        for unreadable in [
            r#"{"cpu_busy_max":"17"}"#,
            r#"{"cpu_busy_max":1e999}"#,
            r#"{"cpu_busy_floor":-1}"#,
            r#"{"cpu_busy_floor":1.5}"#,
        ] {
            assert!(
                serde_json::from_str::<Request>(unreadable).is_err(),
                "{unreadable}"
            );
        }
        let body = serde_json::to_string(&Request {
            cpu_busy_max: Some(None),
            cpu_busy_floor: None,
        })
        .unwrap();
        assert_eq!(body, r#"{"cpu_busy_max":null}"#);
        let body = serde_json::to_string(&Request {
            cpu_busy_max: None,
            cpu_busy_floor: Some(6),
        })
        .unwrap();
        assert_eq!(body, r#"{"cpu_busy_floor":6}"#);
    }

    #[test]
    fn a_saved_file_is_read_without_values_no_request_would_set() {
        assert_eq!(Limits::loaded(Some(17.0), Some(6)), at(17.0, Some(6)));
        assert_eq!(Limits::loaded(Some(0.5), Some(6)), Limits::default());
        assert_eq!(Limits::loaded(None, Some(6)), Limits::default());
        assert_eq!(Limits::loaded(Some(17.0), Some(0)), at(17.0, None));
        let parts = Parts {
            pool: 12.0,
            compile_cache: 6.5,
            outside: 1.5,
        };
        let v = at(17.0, None).view(20, Some(parts), 3);
        assert_eq!(
            (v.cpu_busy_max, v.cpu_busy_floor, v.cpus, v.busy, v.held),
            (Some(17.0), Some(4), 20, Some(20.0), 3)
        );
        assert_eq!(
            (v.pool, v.compile_cache, v.outside),
            (Some(12.0), Some(6.5), Some(1.5))
        );
        assert_eq!(Limits::default().view(20, None, 0).cpu_busy_floor, None);
        let old: View = serde_json::from_str("{}").unwrap();
        assert_eq!(old, View::default(), "an answer without the fields");
    }
}
