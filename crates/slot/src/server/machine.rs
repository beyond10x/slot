//! Headroom of the machine (`--base machine`): the pool's `CPUQuota` follows
//! what runs outside it, so the whole host, not only the pool, keeps the
//! headroom free.
//!
//! Each second the sampler reads the host's busy CPUs from `/proc/stat` and
//! the CPU of the daemon's cgroup (`usage_usec` in `cpu.stat`). The difference
//! is the CPU used outside the pool, smoothed with a 30 s time constant. The
//! quota is the target less that, between a floor and the target, and is
//! set again only when it moved by half a CPU and 10 s passed. Everything here
//! is a pure function or reads a cgroup tree under a given root, so tests run
//! against a `tempfile` directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::types::OutsideOwner;

/// Time constant of the smoothing, seconds.
pub const TAU_S: f64 = 30.0;

/// The pool always keeps at least this many CPUs.
pub const FLOOR_CORES: f64 = 2.0;

/// A new quota is applied only when it differs this much from the last one.
pub const MIN_STEP_CORES: f64 = 0.5;

/// Shortest time between two applied quotas.
pub const MIN_APPLY_GAP: Duration = Duration::from_secs(10);

/// Owners outside the pool that are shown.
pub const TOP_OWNERS: usize = 5;

/// CPUs the pool and everything else may use together with `headroom`
/// percent kept free on `cores` CPUs.
pub fn target_cores(cores: u32, headroom: u8) -> f64 {
    let h = f64::from(headroom.min(super::limits::MAX_HEADROOM));
    f64::from(cores) * (100.0 - h) / 100.0
}

/// CPUs used outside the pool: the host's busy CPUs less the pool's; never
/// below 0.
pub fn outside_cores(busy: f64, pool: f64) -> f64 {
    (busy - pool).max(0.0)
}

/// Exponential moving average with time constant [`TAU_S`]; the first sample
/// is taken as it is.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ewma {
    value: Option<f64>,
}

impl Ewma {
    /// Adds a sample `dt_s` seconds after the last one; the new average.
    pub fn update(&mut self, x: f64, dt_s: f64) -> f64 {
        let v = match self.value {
            None => x,
            Some(v) => v + (x - v) * (1.0 - (-dt_s.max(0.0) / TAU_S).exp()),
        };
        self.value = Some(v);
        v
    }

    /// The average; `None` before the first sample.
    pub fn value(&self) -> Option<f64> {
        self.value
    }
}

/// The pool's quota for one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quota {
    /// CPUs.
    pub cores: f64,
    /// Whether the floor holds it up: the outside load leaves less.
    pub floor: bool,
}

/// `target − outside`, at least [`FLOOR_CORES`] (or the target when that is
/// smaller) and at most the target.
pub fn quota(target: f64, outside: f64) -> Quota {
    let floor = FLOOR_CORES.min(target);
    let want = target - outside;
    Quota {
        cores: want.clamp(floor, target),
        floor: want < floor,
    }
}

/// Whether `q` should be applied now, given the last applied quota and when.
pub fn should_apply(last: Option<(f64, Instant)>, q: f64, now: Instant) -> bool {
    match last {
        None => true,
        Some((v, at)) => {
            (q - v).abs() >= MIN_STEP_CORES && now.saturating_duration_since(at) >= MIN_APPLY_GAP
        }
    }
}

/// `CPUQuota` percent for `cores` CPUs.
pub fn quota_percent(cores: f64) -> u32 {
    (cores * 100.0).round().max(1.0) as u32
}

/// Whether the host is over the target although the pool is at the floor.
pub fn over_target(q: Quota, busy: f64, target: f64) -> bool {
    q.floor && busy > target
}

/// The cgroup v2 path of a process from its `/proc/<pid>/cgroup`, such as
/// `/user.slice/user-1000.slice/user@1000.service/app.slice/slot.service`.
pub fn own_cgroup(proc_cgroup: &str) -> Option<String> {
    proc_cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .filter(|p| p.starts_with('/'))
        .map(str::to_string)
}

/// `usage_usec` from `cpu.stat` in the cgroup directory `dir`.
pub fn usage_usec(dir: &Path) -> Option<u64> {
    let s = std::fs::read_to_string(dir.join("cpu.stat")).ok()?;
    s.lines()
        .find_map(|l| l.strip_prefix("usage_usec "))
        .and_then(|v| v.trim().parse().ok())
}

/// Cgroups the pool's jobs run in, relative to the cgroup root: the daemon's
/// own and `slot.slice` beside the user manager's units, where jobs under
/// `--memory-max` run.
pub fn pool_cgroups(own: &str) -> Vec<String> {
    let mut out = vec![own.to_string()];
    let mut manager = String::new();
    for part in own.split('/').filter(|p| !p.is_empty()) {
        manager.push('/');
        manager.push_str(part);
        if part.starts_with("user@") && part.ends_with(".service") {
            out.push(format!("{manager}/{}", super::JOB_SLICE));
            break;
        }
    }
    out
}

/// Joins a cgroup path such as `/system.slice` onto `root`.
fn under(root: &Path, cgroup: &str) -> PathBuf {
    root.join(cgroup.trim_start_matches('/'))
}

/// Subdirectories of `dir` with their names; none when it cannot be read.
fn children(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .collect();
    out.sort();
    out
}

/// Top-level cgroups outside the pool under `root` (`/sys/fs/cgroup`), with
/// a name each: every unit in the user's `app.slice` but the daemon's, every
/// login session scope, every docker container and `system.slice`. Paths
/// that do not exist are left out.
pub fn owner_cgroups(root: &Path, uid: u32, own: &str) -> Vec<(String, PathBuf)> {
    let own = under(root, own);
    let user = under(root, &format!("/user.slice/user-{uid}.slice"));
    let mut out: Vec<(String, PathBuf)> =
        children(&user.join(format!("user@{uid}.service/app.slice")))
            .into_iter()
            .filter(|(_, p)| !own.starts_with(p))
            .collect();
    out.extend(children(&user).into_iter().filter(|(_, p)| {
        p.file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("session-"))
            && p.extension().is_some_and(|e| e == "scope")
    }));
    out.extend(children(&root.join("docker")).into_iter().map(|(n, p)| {
        let short: String = n.chars().take(12).collect();
        (format!("docker {short}"), p)
    }));
    let system = root.join("system.slice");
    if system.is_dir() {
        out.push(("system.slice".to_string(), system));
    }
    out
}

/// Smoothed CPUs of each owner outside the pool.
#[derive(Debug, Default)]
pub struct Owners {
    prev: HashMap<String, u64>,
    avg: HashMap<String, Ewma>,
}

impl Owners {
    /// Adds one reading of `usage_usec` per owner, `dt_s` after the last;
    /// the busiest [`TOP_OWNERS`] by smoothed CPUs. An owner first seen now
    /// counts from the next reading; one no longer read is forgotten.
    pub fn sample(&mut self, usage: &[(String, u64)], dt_s: f64) -> Vec<OutsideOwner> {
        let mut prev = HashMap::with_capacity(usage.len());
        let mut avg = HashMap::with_capacity(usage.len());
        for (name, usec) in usage {
            if let Some(p) = self.prev.get(name)
                && dt_s > 0.0
            {
                let cores = usec.saturating_sub(*p) as f64 / (dt_s * 1e6);
                let mut e = self.avg.get(name).copied().unwrap_or_default();
                e.update(cores, dt_s);
                avg.insert(name.clone(), e);
            }
            prev.insert(name.clone(), *usec);
        }
        self.prev = prev;
        self.avg = avg;
        let mut out: Vec<OutsideOwner> = self
            .avg
            .iter()
            .filter_map(|(n, e)| {
                Some(OutsideOwner {
                    name: n.clone(),
                    cores: e.value()?,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.cores
                .total_cmp(&a.cores)
                .then_with(|| a.name.cmp(&b.name))
        });
        out.truncate(TOP_OWNERS);
        out
    }
}

/// What one tick read from the cgroup tree.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    /// `usage_usec` of the pool's cgroups together; `None` when the daemon's
    /// own cgroup cannot be read.
    pub pool_usec: Option<u64>,
    /// `usage_usec` per owner outside the pool.
    pub owners: Vec<(String, u64)>,
}

/// Reads the pool's and every owner's CPU under `root`.
pub fn read(root: &Path, uid: u32, own: &str) -> Reading {
    let mut groups = pool_cgroups(own).into_iter();
    let pool_usec = groups
        .next()
        .and_then(|g| usage_usec(&under(root, &g)))
        .map(|u| {
            u + groups
                .filter_map(|g| usage_usec(&under(root, &g)))
                .sum::<u64>()
        });
    let owners = owner_cgroups(root, uid, own)
        .into_iter()
        .filter_map(|(n, p)| Some((n, usage_usec(&p)?)))
        .collect();
    Reading { pool_usec, owners }
}

/// The machine numbers of one tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Now {
    /// CPUs busy on the host over the last second.
    pub busy: f64,
    /// Smoothed CPUs used outside the pool.
    pub outside: f64,
    /// Busiest owners outside the pool.
    pub owners: Vec<OutsideOwner>,
}

/// What a tick shows of the machine base, and the quota it asks for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shown {
    /// CPUs busy on the host.
    pub busy: f64,
    /// CPUs the host may use.
    pub target: f64,
    /// Smoothed CPUs used outside the pool.
    pub outside: f64,
    /// Busiest owners outside the pool.
    pub owners: Vec<OutsideOwner>,
    /// The pool's quota for this tick, CPUs.
    pub quota: f64,
    /// The floor holds the quota up.
    pub floor: bool,
    /// At the floor and the host still over the target.
    pub over: bool,
}

/// The numbers of one tick with `headroom` percent on `cores` CPUs.
pub fn shown(now: Now, cores: u32, headroom: u8) -> Shown {
    let target = target_cores(cores, headroom);
    let q = quota(target, now.outside);
    Shown {
        busy: now.busy,
        target,
        outside: now.outside,
        owners: now.owners,
        quota: q.cores,
        floor: q.floor,
        over: over_target(q, now.busy, target),
    }
}

/// Sampler state: the last pool reading and the averages.
#[derive(Debug, Default)]
pub struct Monitor {
    prev_pool: Option<u64>,
    outside: Ewma,
    owners: Owners,
}

impl Monitor {
    /// Adds one reading with the host's `busy` CPUs, `dt_s` after the last.
    /// `None` until two readings of the pool's cgroup exist.
    pub fn sample(&mut self, r: &Reading, busy: f64, dt_s: f64) -> Option<Now> {
        let owners = self.owners.sample(&r.owners, dt_s);
        let prev = std::mem::replace(&mut self.prev_pool, r.pool_usec);
        let (Some(p), Some(u)) = (prev, r.pool_usec) else {
            return None;
        };
        if dt_s <= 0.0 {
            return None;
        }
        let pool = u.saturating_sub(p) as f64 / (dt_s * 1e6);
        let outside = self.outside.update(outside_cores(busy, pool), dt_s);
        Some(Now {
            busy,
            outside,
            owners,
        })
    }

    /// The smoothed outside CPUs; `None` before the first sample.
    pub fn outside(&self) -> Option<f64> {
        self.outside.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn target_is_the_share_of_the_machine_left_after_the_headroom() {
        assert!(close(target_cores(20, 45), 11.0));
        assert!(close(target_cores(20, 0), 20.0));
        assert!(close(target_cores(8, 50), 4.0));
        assert!(close(target_cores(20, 90), 10.0), "headroom capped at 50 %");
    }

    #[test]
    fn outside_is_busy_less_the_pool_never_negative() {
        assert!(close(outside_cores(18.0, 6.5), 11.5));
        assert!(close(outside_cores(3.0, 3.4), 0.0));
    }

    #[test]
    fn the_average_has_a_30_second_time_constant() {
        let mut e = Ewma::default();
        assert_eq!(e.value(), None);
        assert!(
            close(e.update(4.0, 1.0), 4.0),
            "the first sample is taken as it is"
        );
        let v = e.update(10.0, 30.0);
        assert!(close(v, 4.0 + 6.0 * (1.0 - (-1.0f64).exp())), "{v}");
        let mut e = Ewma::default();
        e.update(0.0, 1.0);
        for _ in 0..30 {
            e.update(10.0, 1.0);
        }
        let v = e.value().unwrap();
        assert!((v - 10.0 * (1.0 - (-1.0f64).exp())).abs() < 0.01, "{v}");
    }

    #[test]
    fn quota_is_target_less_outside_between_floor_and_target() {
        assert_eq!(
            quota(11.0, 4.0),
            Quota {
                cores: 7.0,
                floor: false
            }
        );
        assert_eq!(
            quota(11.0, 10.0),
            Quota {
                cores: 2.0,
                floor: true
            }
        );
        assert_eq!(
            quota(11.0, 0.0),
            Quota {
                cores: 11.0,
                floor: false
            }
        );
        // A target below the floor (2 CPUs at 50 %) is the floor.
        assert_eq!(
            quota(1.0, 0.0),
            Quota {
                cores: 1.0,
                floor: false
            }
        );
        assert_eq!(
            quota(1.0, 3.0),
            Quota {
                cores: 1.0,
                floor: true
            }
        );
    }

    #[test]
    fn a_quota_is_applied_after_half_a_cpu_and_ten_seconds() {
        let t0 = Instant::now();
        assert!(should_apply(None, 7.0, t0), "nothing applied yet");
        let last = Some((7.0, t0));
        assert!(!should_apply(last, 9.0, t0 + Duration::from_secs(9)));
        assert!(should_apply(last, 9.0, t0 + Duration::from_secs(10)));
        assert!(!should_apply(last, 7.4, t0 + Duration::from_secs(60)));
        assert!(should_apply(last, 6.5, t0 + Duration::from_secs(60)));
    }

    #[test]
    fn quota_percent_rounds_and_over_target_needs_the_floor() {
        assert_eq!(quota_percent(7.0), 700);
        assert_eq!(quota_percent(4.326), 433);
        let floor = Quota {
            cores: 2.0,
            floor: true,
        };
        let free = Quota {
            cores: 6.0,
            floor: false,
        };
        assert!(over_target(floor, 18.2, 11.0));
        assert!(!over_target(floor, 10.0, 11.0));
        assert!(!over_target(free, 18.2, 11.0));
    }

    const OWN: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice/slot.service";

    #[test]
    fn own_cgroup_is_the_v2_line() {
        let s = format!("0::{OWN}\n");
        assert_eq!(own_cgroup(&s).as_deref(), Some(OWN));
        assert_eq!(own_cgroup("12:cpu:/x\n"), None);
        assert_eq!(own_cgroup(""), None);
    }

    #[test]
    fn pool_is_the_daemons_cgroup_and_the_job_slice() {
        assert_eq!(
            pool_cgroups(OWN),
            vec![
                OWN.to_string(),
                "/user.slice/user-1000.slice/user@1000.service/slot.slice".to_string()
            ]
        );
        assert_eq!(
            pool_cgroups("/system.slice/slot.service"),
            vec!["/system.slice/slot.service".to_string()]
        );
    }

    fn cgroup(root: &Path, rel: &str, usec: u64) {
        let d = root.join(rel.trim_start_matches('/'));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("cpu.stat"),
            format!("usage_usec {usec}\nuser_usec 1\nsystem_usec 2\n"),
        )
        .unwrap();
    }

    #[test]
    fn usage_is_read_from_cpu_stat() {
        let dir = tempfile::tempdir().unwrap();
        cgroup(dir.path(), "a", 1234);
        assert_eq!(usage_usec(&dir.path().join("a")), Some(1234));
        assert_eq!(usage_usec(&dir.path().join("missing")), None);
        std::fs::create_dir_all(dir.path().join("b")).unwrap();
        std::fs::write(dir.path().join("b/cpu.stat"), "user_usec 3\n").unwrap();
        assert_eq!(usage_usec(&dir.path().join("b")), None);
    }

    fn tree(root: &Path) {
        let user = "/user.slice/user-1000.slice";
        let app = format!("{user}/user@1000.service/app.slice");
        cgroup(root, OWN, 100);
        cgroup(root, &format!("{app}/app-editor.scope"), 10);
        cgroup(root, &format!("{app}/app-terminal.service"), 20);
        cgroup(root, &format!("{user}/session-3.scope"), 30);
        cgroup(root, &format!("{user}/user@1000.service/slot.slice"), 5);
        cgroup(root, "/user.slice/user-1001.slice/session-9.scope", 99);
        cgroup(
            root,
            "/docker/3f2a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8",
            40,
        );
        cgroup(root, "/system.slice", 50);
    }

    #[test]
    fn owners_are_the_top_level_cgroups_outside_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let mut names: Vec<String> = owner_cgroups(dir.path(), 1000, OWN)
            .into_iter()
            .map(|(n, p)| {
                assert!(p.starts_with(dir.path()));
                n
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "app-editor.scope",
                "app-terminal.service",
                "docker 3f2a1b2c3d4e",
                "session-3.scope",
                "system.slice"
            ]
        );
        let r = read(dir.path(), 1000, OWN);
        assert_eq!(r.pool_usec, Some(105), "the daemon's unit and slot.slice");
        assert_eq!(r.owners.len(), 5);
    }

    #[test]
    fn a_host_without_these_cgroups_has_no_owners() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(owner_cgroups(dir.path(), 1000, OWN), Vec::new());
        let r = read(dir.path(), 1000, OWN);
        assert_eq!(r.pool_usec, None);
        assert_eq!(r.owners, Vec::new());
    }

    fn usage(v: &[(&str, u64)]) -> Vec<(String, u64)> {
        v.iter().map(|(n, u)| ((*n).to_string(), *u)).collect()
    }

    #[test]
    fn owners_keep_the_busiest_five_by_smoothed_cpus() {
        let mut o = Owners::default();
        let names = ["a", "b", "c", "d", "e", "f"];
        let first: Vec<(&str, u64)> = names.iter().map(|n| (*n, 0)).collect();
        assert!(o.sample(&usage(&first), 1.0).is_empty(), "no delta yet");
        // a uses 1 CPU, b 2, ... f 6 over one second.
        let second: Vec<(&str, u64)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (*n, (i as u64 + 1) * 1_000_000))
            .collect();
        let top = o.sample(&usage(&second), 1.0);
        let got: Vec<(&str, f64)> = top.iter().map(|x| (x.name.as_str(), x.cores)).collect();
        assert_eq!(
            got,
            [("f", 6.0), ("e", 5.0), ("d", 4.0), ("c", 3.0), ("b", 2.0)]
        );
        // f goes away; it is forgotten.
        let third: Vec<(&str, u64)> = names[..5]
            .iter()
            .enumerate()
            .map(|(i, n)| (*n, (i as u64 + 1) * 2_000_000))
            .collect();
        let top = o.sample(&usage(&third), 1.0);
        assert_eq!(top.len(), 5);
        assert_eq!(top[0].name, "e");
    }

    #[test]
    fn a_tick_shows_target_quota_and_over_target() {
        let now = Now {
            busy: 18.2,
            outside: 9.5,
            owners: Vec::new(),
        };
        let s = shown(now.clone(), 20, 45);
        assert!(close(s.target, 11.0));
        assert!(close(s.quota, 2.0));
        assert!(s.floor && s.over);
        let s = shown(
            Now {
                busy: 10.0,
                outside: 3.0,
                ..now
            },
            20,
            45,
        );
        assert!(close(s.quota, 8.0));
        assert!(!s.floor && !s.over);
        assert!(close(s.busy, 10.0) && close(s.outside, 3.0));
    }

    #[test]
    fn monitor_smooths_busy_less_pool() {
        let mut m = Monitor::default();
        let r = |pool: u64| Reading {
            pool_usec: Some(pool),
            owners: Vec::new(),
        };
        assert_eq!(m.sample(&r(0), 15.0, 1.0), None, "first pool reading");
        // The pool used 6 CPUs over 1 s while 15 were busy: 9 outside.
        let now = m.sample(&r(6_000_000), 15.0, 1.0).unwrap();
        assert!(close(now.busy, 15.0));
        assert!(close(now.outside, 9.0));
        assert_eq!(m.outside(), Some(9.0));
        // The daemon's cgroup became unreadable: no numbers, and the next
        // reading starts over.
        assert_eq!(
            m.sample(
                &Reading {
                    pool_usec: None,
                    owners: Vec::new()
                },
                15.0,
                1.0
            ),
            None
        );
        assert_eq!(m.sample(&r(9_000_000), 15.0, 1.0), None);
    }
}
