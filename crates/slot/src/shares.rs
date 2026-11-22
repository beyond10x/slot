//! Admission by CPU share (`slot admission shares`).
//!
//! With `slots`, the default, a job holds slots of the pool and the pool's
//! size bounds how many jobs run. With `shares` a job asks for a share of
//! the CPUs and holds no slot of its own beyond the implicit one every
//! jobserver client has; the daemon admits the head of the queue while the
//! share is free:
//!
//! - A job's share is predicted from the job log ([`predict`]): CPU seconds
//!   over wall time of earlier runs of the same repository and command,
//!   weighted by age, the 90th percentile plus 15 %, at least
//!   [`MIN_SHARE`]; [`UNKNOWN_SHARE`] without enough runs. A client that asks
//!   for N slots (`--tokens N`) asks for a share of N ([`share_for`]).
//! - The free share ([`room`]) is the budget less the CPUs the pool and the
//!   compile cache used over the last [`WINDOW_S`] seconds, less the shares
//!   of jobs that started in that time and do not show in the figure yet.
//!   A job that just started and has used almost nothing for
//!   [`IDLE_SAMPLES`] seconds counts with what it used, not with its share:
//!   its room is lent to the queue.
//! - The budget ([`Budget`]) starts at [`START_FRACTION`] of the host's CPUs
//!   and never passes the pool's CPU quota. Every [`STEP_MS`] the median
//!   slowdown of the jobs that ended (wall time over the best earlier wall
//!   time of the same repository and command) moves it: under
//!   [`SLOW_OK`] it rises by [`RAISE`], over [`SLOW_BAD`] it falls to
//!   [`LOWER`] of itself.
//! - Bounds: at least `floor` jobs run whatever the share says, at most
//!   `max`. CPU pressure of the host over its mark is a brake: only jobs
//!   predicted short start.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Seconds of samples the busy figure covers, and how long a job that
/// started counts with its share.
pub const WINDOW_S: usize = 5;

/// How long a job that started counts with its share, ms.
pub const START_MS: i64 = 5_000;

/// How often the budget moves, ms.
pub const STEP_MS: i64 = 5_000;

/// The budget a daemon starts with, as a share of the host's CPUs: 15 of
/// 20, the CPUs measured busy with 17 slots.
pub const START_FRACTION: f64 = 0.75;

/// The budget never falls under this, CPUs.
pub const MIN_BUDGET: f64 = 1.0;

/// CPUs the budget rises by when jobs run about as fast as they can.
pub const RAISE: f64 = 0.5;

/// What the budget falls to, as a share of itself, when jobs run slow.
pub const LOWER: f64 = 0.85;

/// A median slowdown under this raises the budget.
pub const SLOW_OK: f64 = 1.3;

/// A median slowdown over this lowers the budget.
pub const SLOW_BAD: f64 = 2.0;

/// The least share a job is predicted.
pub const MIN_SHARE: f64 = 0.1;

/// The share of a command without enough earlier runs.
pub const UNKNOWN_SHARE: f64 = 1.0;

/// Earlier runs a prediction needs.
pub const MIN_RUNS: usize = 3;

/// The margin on the predicted percentile.
pub const MARGIN: f64 = 1.15;

/// The percentile of earlier runs a share is predicted from.
pub const PERCENTILE: f64 = 0.9;

/// Half-life of an earlier run's weight, ms: 12 hours.
pub const HALF_LIFE_MS: f64 = 12.0 * 3600.0 * 1000.0;

/// Jobs that run whatever the share says, until one is named.
pub const DEFAULT_FLOOR: usize = 4;

/// The most jobs that run at once, until one is named.
pub const DEFAULT_MAX: usize = 48;

/// CPU pressure of the host (`some avg10`, percent) over which only jobs
/// predicted short start, until one is named: the median pressure of the
/// census windows with 15 or more of 20 CPUs busy (`docs/design.md`).
pub const DEFAULT_PRESSURE: f64 = 60.0;

/// A job that just started and used less than this many CPUs in each of
/// its last [`IDLE_SAMPLES`] samples is idle: it counts with what it uses.
pub const IDLE_CORES: f64 = MIN_SHARE;

/// Samples, one a second, under [`IDLE_CORES`] that make a job idle.
pub const IDLE_SAMPLES: usize = 2;

/// How the daemon admits queued jobs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "Admission")]
pub enum Mode {
    /// By slots of the pool: a job holds its slots while it runs.
    #[default]
    Slots,
    /// By CPU share: a job starts while its share of the CPUs is free.
    Shares,
}

impl Mode {
    /// The name the command line and the API use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slots => "slots",
            Self::Shares => "shares",
        }
    }
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "slots" => Ok(Self::Slots),
            "shares" => Ok(Self::Shares),
            _ => Err(format!("`{s}` is not an admission: `shares` or `slots`")),
        }
    }
}

/// The settings, as saved in `limits.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Settings {
    /// How queued jobs are admitted.
    pub mode: Mode,
    /// Jobs that run whatever the share says; `None` = [`DEFAULT_FLOOR`].
    pub floor: Option<usize>,
    /// The most jobs that run at once; `None` = [`DEFAULT_MAX`].
    pub max: Option<usize>,
    /// CPU pressure over which only short jobs start; `None` =
    /// [`DEFAULT_PRESSURE`].
    pub pressure_max: Option<f64>,
}

/// Whether `p` is a pressure mark: a percent over 0, at most 100.
fn valid_pressure(p: f64) -> bool {
    p.is_finite() && p > 0.0 && p <= 100.0
}

impl Settings {
    /// What a `limits.json` holds, without values no request would set.
    pub fn loaded(
        mode: Mode,
        floor: Option<usize>,
        max: Option<usize>,
        pressure_max: Option<f64>,
    ) -> Self {
        let mut s = Self {
            mode,
            floor: floor.filter(|&f| f > 0),
            max: max.filter(|&m| m > 0),
            pressure_max: pressure_max.filter(|&p| valid_pressure(p)),
        };
        if s.floor() > s.max() {
            s.floor = None;
            s.max = None;
        }
        s
    }

    /// Jobs that run whatever the share says.
    pub fn floor(&self) -> usize {
        self.floor.unwrap_or(DEFAULT_FLOOR)
    }

    /// The most jobs that run at once.
    pub fn max(&self) -> usize {
        self.max.unwrap_or(DEFAULT_MAX)
    }

    /// CPU pressure over which only short jobs start.
    pub fn pressure_max(&self) -> f64 {
        self.pressure_max.unwrap_or(DEFAULT_PRESSURE)
    }

    /// The settings after `req`, or why it is refused.
    pub fn changed(&self, req: &Request) -> Result<Self, String> {
        if *req == Request::default() {
            return Err(
                "slot: name admission (shares or slots), floor, max, pressure_max, or several"
                    .into(),
            );
        }
        if req.floor == Some(0) {
            return Err("slot: floor must be at least 1".into());
        }
        if req.max == Some(0) {
            return Err("slot: max must be at least 1".into());
        }
        if let Some(p) = req.pressure_max
            && !valid_pressure(p)
        {
            return Err("slot: pressure_max is a percent over 0, at most 100".into());
        }
        let next = Self {
            mode: req.admission.unwrap_or(self.mode),
            floor: req.floor.or(self.floor),
            max: req.max.or(self.max),
            pressure_max: req.pressure_max.or(self.pressure_max),
        };
        if next.floor() > next.max() {
            return Err(format!(
                "slot: the floor of {} jobs is over the most of {}",
                next.floor(),
                next.max()
            ));
        }
        Ok(next)
    }

    /// The view of these settings with nothing measured yet.
    pub fn view(&self, cpus: u32) -> View {
        View {
            admission: self.mode,
            floor: self.floor(),
            max: self.max(),
            pressure_max: self.pressure_max(),
            cpus,
            ..View::default()
        }
    }
}

/// Body of `PUT /api/admission`; a field left out is not changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// `shares` or `slots`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<Mode>,
    /// Jobs that run whatever the share says, at least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<usize>,
    /// The most jobs that run at once, at least the floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
    /// CPU pressure of the host, percent, over which only jobs predicted
    /// short start; over 0, at most 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_max: Option<f64>,
}

/// Answer of `GET` and `PUT /api/admission`, and the figures `slot status`
/// and the web view show while shares are on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(rename = "SharesView")]
pub struct View {
    /// How queued jobs are admitted.
    #[serde(default)]
    pub admission: Mode,
    /// Jobs that run whatever the share says.
    #[serde(default)]
    #[ts(type = "number")]
    pub floor: usize,
    /// The most jobs that run at once.
    #[serde(default)]
    #[ts(type = "number")]
    pub max: usize,
    /// CPU pressure over which only jobs predicted short start, percent.
    #[serde(default)]
    pub pressure_max: f64,
    /// CPUs of the host.
    #[serde(default)]
    pub cpus: u32,
    /// CPUs the running jobs may use together now.
    #[serde(default)]
    pub budget: f64,
    /// The most the budget may reach: the pool's CPU quota.
    #[serde(default)]
    pub cap: f64,
    /// CPUs of the budget a queued job may ask for now.
    #[serde(default)]
    pub free: f64,
    /// CPUs the pool and the compile cache used over the last 5 s.
    #[serde(default)]
    pub busy: f64,
    /// CPUs promised to jobs that started in the last 5 s and do not show
    /// in `busy` yet.
    #[serde(default)]
    pub starting: f64,
    /// Jobs that run or were let through.
    #[serde(default)]
    #[ts(type = "number")]
    pub running: usize,
    /// Running jobs that use almost no CPU.
    #[serde(default)]
    #[ts(type = "number")]
    pub idle: usize,
    /// CPUs those idle jobs asked for and do not use: lent to the queue.
    #[serde(default)]
    pub lent: f64,
    /// CPU pressure of the host now, percent.
    #[serde(default)]
    pub pressure: f64,
    /// Whether the pressure is over its mark: only short jobs start.
    #[serde(default)]
    pub braking: bool,
}

/// The share a job asks for: `tokens` when the client named slots, else
/// the prediction from earlier runs, else [`UNKNOWN_SHARE`]; at least
/// [`MIN_SHARE`], at most `cap`.
pub fn share_for(tokens: Option<u32>, predicted: Option<f64>, cap: f64) -> f64 {
    let share = match tokens {
        Some(n) => f64::from(n),
        None => predicted.unwrap_or(UNKNOWN_SHARE),
    };
    share.min(cap).max(MIN_SHARE)
}

/// The share predicted from earlier runs at `now_ms`, each `(CPUs over the
/// run, end in ms)`: the age-weighted [`PERCENTILE`] times [`MARGIN`], at
/// least [`MIN_SHARE`]; `None` with fewer than [`MIN_RUNS`] runs.
pub fn predict(runs: &[(f64, i64)], now_ms: i64) -> Option<f64> {
    if runs.len() < MIN_RUNS {
        return None;
    }
    let mut weighted: Vec<(f64, f64)> = runs
        .iter()
        .filter(|(cores, _)| cores.is_finite())
        .map(|&(cores, end)| {
            let age = (now_ms - end).max(0) as f64;
            (cores.max(0.0), 0.5_f64.powf(age / HALF_LIFE_MS))
        })
        .collect();
    if weighted.len() < MIN_RUNS {
        return None;
    }
    weighted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = weighted.iter().map(|(_, w)| w).sum();
    let mark = total * PERCENTILE;
    let mut sum = 0.0;
    let mut at = weighted[weighted.len() - 1].0;
    for (cores, w) in &weighted {
        sum += w;
        // A hair under the mark counts as reaching it: ten equal weights
        // of 0.1 sum to a little less than 0.9 × 1.0.
        if sum >= mark - 1e-9 * total {
            at = *cores;
            break;
        }
    }
    Some((at * MARGIN).max(MIN_SHARE))
}

/// The slowdown of a run that took `wall_ms`, against the best earlier run
/// of the same command (`best_ms`); `None` without an earlier run of a
/// second or longer.
pub fn slowdown(wall_ms: i64, best_ms: Option<u64>) -> Option<f64> {
    let best = best_ms.filter(|&b| b >= 1000)?;
    Some(wall_ms.max(0) as f64 / best as f64)
}

/// The CPUs the running jobs may use together.
#[derive(Debug, Clone, Default)]
pub struct Budget {
    value: f64,
    last_ms: Option<i64>,
    slowdowns: Vec<f64>,
}

impl Budget {
    /// The budget a daemon on `cpus` CPUs with a quota of `cap` starts with.
    pub fn new(cpus: u32, cap: f64) -> Self {
        Self {
            value: bounded(START_FRACTION * f64::from(cpus), cap),
            last_ms: None,
            slowdowns: Vec::new(),
        }
    }

    /// CPUs now.
    pub fn value(&self) -> f64 {
        self.value
    }

    /// A job ended `slowdown` times slower than its best earlier run.
    pub fn finished(&mut self, slowdown: f64) {
        if slowdown.is_finite() {
            self.slowdowns.push(slowdown);
        }
    }

    /// Moves the budget at `now_ms` when [`STEP_MS`] passed since the last
    /// step, and keeps it between [`MIN_BUDGET`] and `cap`; returns it.
    pub fn step(&mut self, now_ms: i64, cap: f64) -> f64 {
        match self.last_ms {
            None => self.last_ms = Some(now_ms),
            Some(last) if now_ms - last >= STEP_MS => {
                self.last_ms = Some(now_ms);
                if !self.slowdowns.is_empty() {
                    let mut s = std::mem::take(&mut self.slowdowns);
                    s.sort_by(f64::total_cmp);
                    let median = s[s.len() / 2];
                    if median < SLOW_OK {
                        self.value += RAISE;
                    } else if median > SLOW_BAD {
                        self.value *= LOWER;
                    }
                }
            }
            Some(_) => {}
        }
        self.value = bounded(self.value, cap);
        self.value
    }
}

/// `v` between [`MIN_BUDGET`] and `cap`; the least wins when `cap` is under it.
fn bounded(v: f64, cap: f64) -> f64 {
    v.min(cap).max(MIN_BUDGET)
}

/// One job that runs or was let through, as the free share weighs it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Running {
    /// The share it asked for.
    pub share: f64,
    /// CPUs it used over the busy figure's window: the sum of its samples
    /// over [`WINDOW_S`].
    pub shown: f64,
    /// It started less than [`START_MS`] ago, or has not started yet.
    pub young: bool,
    /// Each of its last [`IDLE_SAMPLES`] samples was under [`IDLE_CORES`].
    pub idle: bool,
}

/// What the free share is made of.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Room {
    /// CPUs a queued job may ask for.
    pub free: f64,
    /// CPUs promised to young jobs beyond what they show.
    pub starting: f64,
    /// Idle jobs.
    pub idle: usize,
    /// CPUs idle jobs asked for and do not use.
    pub lent: f64,
}

/// The free share of `budget` with `busy` CPUs used by the pool and the
/// compile cache over the last [`WINDOW_S`] seconds and `jobs` running.
pub fn room(budget: f64, busy: f64, jobs: &[Running]) -> Room {
    let mut r = Room::default();
    for j in jobs {
        let unshown = (j.share - j.shown).max(0.0);
        if j.idle {
            r.idle += 1;
            r.lent += unshown;
        } else if j.young {
            r.starting += unshown;
        }
    }
    r.free = budget - busy - r.starting;
    r
}

/// The line `slot status` and `slot admission` print while shares are on.
pub fn line(v: &View) -> String {
    let mut s = format!(
        "shares: budget {:.1} of {:.1} CPUs, {:.1} free ({:.1} busy",
        v.budget, v.cap, v.free, v.busy
    );
    if v.starting >= 0.05 {
        let _ = write!(s, ", {:.1} for jobs that just started", v.starting);
    }
    let _ = write!(s, "), {} running ({} to {})", v.running, v.floor, v.max);
    if v.idle > 0 {
        let _ = write!(s, ", {} idle lend {:.1}", v.idle, v.lent);
    }
    if v.braking {
        let _ = write!(
            s,
            ", CPU pressure {:.0} % over {:.0} %: only short jobs start",
            v.pressure, v.pressure_max
        );
    }
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600 * 1000;

    #[test]
    fn a_share_is_the_age_weighted_high_percentile_plus_a_margin() {
        assert_eq!(predict(&[], 0), None, "no run");
        assert_eq!(predict(&[(2.0, 0), (2.0, 0)], 0), None, "two runs");
        let p = predict(&[(1.0, 0), (1.0, 0), (1.0, 0)], 0).unwrap();
        assert!((p - 1.15).abs() < 1e-9, "{p}");
        // Ten runs of 0.2 CPUs and one of 4: the 90th percentile is 0.2.
        let mut runs = vec![(0.2, 0); 10];
        runs.push((4.0, 0));
        let p = predict(&runs, 0).unwrap();
        assert!((p - 0.23).abs() < 1e-9, "{p}");
        // The same runs, the light ones two days old: the heavy run of
        // today outweighs them and the percentile is 4.
        let mut aged: Vec<(f64, i64)> = vec![(0.2, 0); 10];
        aged.push((4.0, 48 * H));
        let p = predict(&aged, 48 * H).unwrap();
        assert!((p - 4.6).abs() < 1e-9, "{p}");
        let p = predict(&[(0.01, 0); 5], 0).unwrap();
        assert!((p - MIN_SHARE).abs() < 1e-9, "at least {MIN_SHARE}: {p}");
    }

    #[test]
    fn named_slots_are_a_share_and_an_unknown_command_asks_for_one_cpu() {
        assert!((share_for(Some(3), Some(0.5), 15.0) - 3.0).abs() < 1e-9);
        assert!((share_for(None, Some(0.23), 15.0) - 0.23).abs() < 1e-9);
        assert!((share_for(None, None, 15.0) - UNKNOWN_SHARE).abs() < 1e-9);
        assert!(
            (share_for(Some(59), None, 17.0) - 17.0).abs() < 1e-9,
            "capped at the quota"
        );
        assert!((share_for(None, Some(0.01), 17.0) - MIN_SHARE).abs() < 1e-9);
    }

    #[test]
    fn slowdown_is_wall_time_over_the_best_earlier_run() {
        assert_eq!(slowdown(10_000, None), None);
        assert_eq!(slowdown(10_000, Some(500)), None, "under a second");
        assert!((slowdown(30_000, Some(10_000)).unwrap() - 3.0).abs() < 1e-9);
        assert!((slowdown(5_000, Some(10_000)).unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_budget_starts_at_three_quarters_of_the_cpus_and_never_passes_the_quota() {
        assert!((Budget::new(20, 17.0).value() - 15.0).abs() < 1e-9);
        assert!(
            (Budget::new(20, 12.0).value() - 12.0).abs() < 1e-9,
            "the quota"
        );
        assert!((Budget::new(1, 1.0).value() - MIN_BUDGET).abs() < 1e-9);
        let mut b = Budget::new(20, 17.0);
        assert!(
            (b.step(0, 17.0) - 15.0).abs() < 1e-9,
            "the first step only starts the clock"
        );
        for i in 1..=10 {
            b.finished(1.0);
            b.step(i * STEP_MS, 17.0);
        }
        assert!(
            (b.value() - 17.0).abs() < 1e-9,
            "fast jobs raise it to the quota, not past it"
        );
        assert!(
            (b.step(11 * STEP_MS, 12.0) - 12.0).abs() < 1e-9,
            "a lower quota"
        );
    }

    #[test]
    fn the_median_slowdown_of_ended_jobs_moves_the_budget_every_five_seconds() {
        let mut b = Budget::new(20, 20.0);
        b.step(0, 20.0);
        b.finished(1.1);
        b.finished(1.2);
        b.finished(5.0);
        assert!((b.step(STEP_MS - 1, 20.0) - 15.0).abs() < 1e-9, "not yet");
        assert!((b.step(STEP_MS, 20.0) - 15.5).abs() < 1e-9, "median 1.2");
        b.finished(2.5);
        b.finished(3.0);
        b.finished(1.0);
        assert!(
            (b.step(2 * STEP_MS, 20.0) - 15.5 * LOWER).abs() < 1e-9,
            "median 2.5"
        );
        let v = b.value();
        b.finished(1.6);
        assert!(
            (b.step(3 * STEP_MS, 20.0) - v).abs() < 1e-9,
            "between the marks"
        );
        assert!((b.step(4 * STEP_MS, 20.0) - v).abs() < 1e-9, "no job ended");
        for i in 5..60 {
            b.finished(10.0);
            b.step(i * STEP_MS, 20.0);
        }
        assert!((b.value() - MIN_BUDGET).abs() < 1e-9, "{}", b.value());
    }

    #[test]
    fn the_free_share_is_the_budget_less_the_figure_and_the_jobs_that_just_started() {
        let r = room(15.0, 9.0, &[]);
        assert!((r.free - 6.0).abs() < 1e-9);
        let young = Running {
            share: 2.0,
            shown: 0.5,
            young: true,
            idle: false,
        };
        let old = Running {
            share: 4.0,
            shown: 0.2,
            young: false,
            idle: false,
        };
        let r = room(15.0, 9.0, &[young, old]);
        assert!(
            (r.starting - 1.5).abs() < 1e-9,
            "the young job less what it shows"
        );
        assert!(
            (r.free - 4.5).abs() < 1e-9,
            "an old job counts by the figure only"
        );
        let r = room(15.0, 16.0, &[]);
        assert!(r.free < 0.0, "over the budget");
    }

    #[test]
    fn a_young_job_that_uses_almost_nothing_lends_its_share_to_the_queue() {
        let sleeper = Running {
            share: 1.0,
            shown: 0.0,
            young: true,
            idle: true,
        };
        let r = room(2.25, 0.0, &[sleeper, sleeper]);
        assert!((r.free - 2.25).abs() < 1e-9, "{r:?}");
        assert_eq!(r.idle, 2);
        assert!((r.lent - 2.0).abs() < 1e-9);
        let busy = Running {
            idle: false,
            ..sleeper
        };
        let r = room(2.25, 0.0, &[busy, busy]);
        assert!((r.free - 0.25).abs() < 1e-9, "not idle: its share counts");
        assert_eq!((r.idle, r.lent), (0, 0.0));
        let old_idle = Running {
            young: false,
            ..sleeper
        };
        let r = room(2.25, 0.0, &[old_idle]);
        assert!((r.free - 2.25).abs() < 1e-9);
        assert_eq!(r.idle, 1, "an old idle job lends too");
    }

    #[test]
    fn a_request_names_the_admission_the_bounds_or_the_pressure_mark() {
        let req = |body: &str| serde_json::from_str::<Request>(body).unwrap();
        let off = Settings::default();
        assert_eq!(off.mode, Mode::Slots, "slots until shares are named");
        assert_eq!(
            (off.floor(), off.max(), off.pressure_max()),
            (DEFAULT_FLOOR, DEFAULT_MAX, DEFAULT_PRESSURE)
        );
        let on = off.changed(&req(r#"{"admission":"shares"}"#)).unwrap();
        assert_eq!(on.mode, Mode::Shares);
        let b = on.changed(&req(r#"{"floor":1,"max":3}"#)).unwrap();
        assert_eq!((b.mode, b.floor(), b.max()), (Mode::Shares, 1, 3));
        let p = b.changed(&req(r#"{"pressure_max":40}"#)).unwrap();
        assert!((p.pressure_max() - 40.0).abs() < 1e-9);
        assert_eq!(
            p.changed(&req(r#"{"admission":"slots"}"#)).unwrap().mode,
            Mode::Slots,
            "the bounds stay for the next time"
        );
        for bad in [
            "{}",
            r#"{"floor":0}"#,
            r#"{"max":0}"#,
            r#"{"floor":5,"max":3}"#,
            r#"{"pressure_max":0}"#,
            r#"{"pressure_max":101}"#,
        ] {
            let e = off.changed(&req(bad)).unwrap_err();
            assert!(e.starts_with("slot: "), "{bad}: {e}");
        }
        assert!(
            b.changed(&req(r#"{"floor":4}"#)).is_err(),
            "a floor over the max in force"
        );
        assert!(serde_json::from_str::<Request>(r#"{"admission":"cores"}"#).is_err());
        assert_eq!("shares".parse(), Ok(Mode::Shares));
        assert_eq!("slots".parse(), Ok(Mode::Slots));
        assert!("Shares".parse::<Mode>().is_err());
        let old: View = serde_json::from_str("{}").unwrap();
        assert_eq!(old, View::default(), "an answer without the fields");
    }

    #[test]
    fn a_saved_file_is_read_without_values_no_request_would_set() {
        let s = Settings::loaded(Mode::Shares, Some(0), Some(0), Some(500.0));
        assert_eq!(
            s,
            Settings {
                mode: Mode::Shares,
                ..Settings::default()
            }
        );
        let s = Settings::loaded(Mode::Shares, Some(6), Some(3), None);
        assert_eq!((s.floor, s.max), (None, None), "a max under the floor");
        let s = Settings::loaded(Mode::Slots, Some(2), Some(8), Some(45.0));
        assert_eq!((s.floor(), s.max(), s.pressure_max()), (2, 8, 45.0));
    }

    #[test]
    fn the_status_line_names_the_budget_the_free_share_and_what_idle_jobs_lend() {
        let v = View {
            admission: Mode::Shares,
            floor: 4,
            max: 48,
            pressure_max: 60.0,
            cpus: 20,
            budget: 15.0,
            cap: 17.0,
            free: 3.24,
            busy: 9.0,
            starting: 2.76,
            running: 12,
            idle: 2,
            lent: 2.0,
            pressure: 31.0,
            braking: false,
        };
        assert_eq!(
            line(&v),
            "shares: budget 15.0 of 17.0 CPUs, 3.2 free (9.0 busy, 2.8 for jobs that just \
             started), 12 running (4 to 48), 2 idle lend 2.0\n"
        );
        let braking = View {
            braking: true,
            pressure: 72.0,
            idle: 0,
            lent: 0.0,
            ..v
        };
        assert_eq!(
            line(&braking),
            "shares: budget 15.0 of 17.0 CPUs, 3.2 free (9.0 busy, 2.8 for jobs that just \
             started), 12 running (4 to 48), CPU pressure 72 % over 60 %: only short jobs start\n"
        );
    }
}
