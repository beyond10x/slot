//! CPU time and RSS of process trees, read from `/proc`.

use std::collections::{HashMap, HashSet};
use std::fs;

/// Clock ticks per second on Linux.
pub const CLK_TCK: f64 = 100.0;
const PAGE: u64 = 4096;

/// CPUs online on the host, from `/sys/devices/system/cpu/online`.
/// Unlike [`std::thread::available_parallelism`] it ignores the cgroup CPU
/// quota (`cpu.max`), which the daemon sets on its own unit for a headroom:
/// counted under that quota, a re-executed daemon would size its pool and its
/// next quota from the capped number and shrink both on every upgrade.
pub fn host_cpus() -> usize {
    fs::read_to_string("/sys/devices/system/cpu/online")
        .ok()
        .and_then(|s| count_cpu_list(&s))
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
}

/// CPUs in a kernel CPU list such as `0-7,16-23`; `None` when malformed.
pub fn count_cpu_list(s: &str) -> Option<usize> {
    let mut n = 0;
    for part in s.trim().split(',').filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let (a, b): (usize, usize) = (a.parse().ok()?, b.parse().ok()?);
        n += b.checked_sub(a)? + 1;
    }
    (n > 0).then_some(n)
}

/// One process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    /// Process id.
    pub pid: i32,
    /// Parent process id.
    pub ppid: i32,
    /// utime + stime in clock ticks.
    pub ticks: u64,
    /// Resident set size in bytes.
    pub rss: u64,
    /// Command name (`comm`).
    pub comm: String,
    /// Start time in clock ticks after boot; with `pid` it identifies a process.
    pub start: u64,
}

/// Parses one `/proc/<pid>/stat` line.
pub fn parse_stat(pid: i32, s: &str) -> Option<Proc> {
    let open = s.find('(')?;
    let close = s.rfind(')')?;
    let comm = s.get(open + 1..close)?.to_string();
    let f: Vec<&str> = s.get(close + 2..)?.split_whitespace().collect();
    if f.len() < 22 {
        return None;
    }
    let ppid = f[1].parse().ok()?;
    let ut: u64 = f[11].parse().ok()?;
    let st: u64 = f[12].parse().ok()?;
    let start: u64 = f[19].parse().ok()?;
    let rss: u64 = f[21].parse().ok()?;
    Some(Proc {
        pid,
        ppid,
        ticks: ut + st,
        rss: rss * PAGE,
        comm,
        start,
    })
}

/// Total CPU seconds of a process including its reaped children:
/// utime + stime + cutime + cstime. Works on a zombie that is not reaped yet.
pub fn total_cpu_seconds(pid: i32) -> Option<f64> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = s.rfind(')')?;
    let f: Vec<&str> = s.get(close + 2..)?.split_whitespace().collect();
    let sum: u64 = f
        .get(11..15)?
        .iter()
        .map(|x| x.parse::<i64>().unwrap_or(0).max(0) as u64)
        .sum();
    Some(sum as f64 / CLK_TCK)
}

/// One pass over `/proc`.
#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    procs: HashMap<i32, Proc>,
    children: HashMap<i32, Vec<i32>>,
}

impl Snapshot {
    /// Reads every process.
    pub fn scan() -> Self {
        let mut s = Self::default();
        let Ok(rd) = fs::read_dir("/proc") else {
            return s;
        };
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else {
                continue;
            };
            let Ok(text) = fs::read_to_string(e.path().join("stat")) else {
                continue;
            };
            if let Some(p) = parse_stat(pid, &text) {
                s.insert(p);
            }
        }
        s
    }

    /// Every process.
    pub fn procs(&self) -> impl Iterator<Item = &Proc> {
        self.procs.values()
    }

    /// Adds a process (used by tests).
    pub fn insert(&mut self, p: Proc) {
        self.children.entry(p.ppid).or_default().push(p.pid);
        self.procs.insert(p.pid, p);
    }

    /// `root` and every live descendant.
    pub fn tree(&self, root: i32) -> Vec<Proc> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        let mut seen = HashSet::new();
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(p) = self.procs.get(&pid) {
                out.push(p.clone());
            }
            if let Some(c) = self.children.get(&pid) {
                stack.extend(c);
            }
        }
        out
    }

    /// Processes named `comm` whose parent is not in `exclude`: compile
    /// servers that run work for jobs outside the jobs' trees.
    pub fn daemons(&self, comm: &str, exclude: &HashSet<i32>) -> Vec<i32> {
        self.procs
            .values()
            .filter(|p| p.comm == comm && !exclude.contains(&p.pid) && !exclude.contains(&p.ppid))
            .map(|p| p.pid)
            .collect()
    }
}

/// One variable from `/proc/<pid>/environ`.
pub fn env_value(pid: i32, key: &str) -> Option<String> {
    let b = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let pre = format!("{key}=");
    b.split(|&c| c == 0).find_map(|kv| {
        kv.strip_prefix(pre.as_bytes())
            .map(|v| String::from_utf8_lossy(v).into_owned())
    })
}

/// What one process of a build is doing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Activity {
    /// `compile`, `build-script`, `test`, `native` or `link`.
    pub kind: &'static str,
    /// Crate, package or test binary name.
    pub name: String,
}

fn strip_hash(s: &str) -> &str {
    match s.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 16 && tail.bytes().all(|b| b.is_ascii_hexdigit()) => {
            head
        }
        _ => s,
    }
}

/// Classifies a process from its `comm` and command line; `None` for
/// processes that are not build work (cargo, shells, slot itself).
pub fn classify(comm: &str, argv: &[String]) -> Option<Activity> {
    if let Some(i) = argv.iter().position(|a| a == "--crate-name") {
        let name = argv.get(i + 1)?.clone();
        return Some(Activity {
            kind: "compile",
            name,
        });
    }
    let exe = argv.first().map_or("", String::as_str);
    let base = exe.rsplit('/').next().unwrap_or(exe);
    if base.starts_with("build-script-") {
        let dir = exe.rsplit('/').nth(1).unwrap_or("");
        return Some(Activity {
            kind: "build-script",
            name: strip_hash(dir).to_string(),
        });
    }
    if exe.contains("/deps/") {
        return Some(Activity {
            kind: "test",
            name: strip_hash(base).to_string(),
        });
    }
    let kind = match comm {
        "ld" | "ld.lld" | "lld" | "ld.gold" | "ld.bfd" | "mold" | "collect2" => "link",
        "cc" | "c++" | "gcc" | "g++" | "clang" | "clang++" | "cc1" | "cc1plus" => "native",
        _ => return None,
    };
    Some(Activity {
        kind,
        name: base.to_string(),
    })
}

fn cmdline(pid: i32) -> Vec<String> {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| {
            b.split(|&c| c == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// One unit of work running in a job right now.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// What it is.
    pub activity: Activity,
    /// Cores it used since the previous sample.
    pub cores: f64,
    /// Samples it has been seen in (about seconds).
    pub secs: u32,
}

/// Turns successive snapshots of one tree into cores in use.
#[derive(Debug, Default)]
pub struct Tracker {
    prev: Option<HashMap<i32, u64>>,
    kinds: HashMap<i32, Option<Activity>>,
    age: HashMap<i32, u32>,
    seen: HashSet<Activity>,
}

/// One tracker sample.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Sample {
    /// Cores used since the previous sample.
    pub cores: f64,
    /// Current resident memory of the tree plus extra processes.
    pub rss: u64,
    /// Live processes counted.
    pub procs: usize,
    /// Ticks spent by `extra` processes in this interval.
    pub extra_ticks: u64,
    /// Work units running now, busiest first.
    pub units: Vec<Unit>,
    /// Distinct units seen since the job started.
    pub units_seen: usize,
}

impl Tracker {
    /// Samples `root`'s tree plus `extra` processes; `dt` is seconds since
    /// the previous sample (0 for the baseline).
    pub fn sample(&mut self, s: &Snapshot, root: i32, extra: &[Proc], dt: f64) -> Sample {
        let mut cur = HashMap::new();
        let mut out = Sample::default();
        let mut delta = 0u64;
        let tree = s.tree(root);
        let in_tree = tree.len();
        for (i, p) in tree.iter().chain(extra.iter()).enumerate() {
            if cur.contains_key(&p.pid) {
                continue;
            }
            cur.insert(p.pid, p.ticks);
            out.rss += p.rss;
            let d = match self.prev.as_ref() {
                None => 0,
                Some(prev) => match prev.get(&p.pid) {
                    Some(&old) if p.ticks >= old => p.ticks - old,
                    _ => p.ticks,
                },
            };
            delta += d;
            if i >= in_tree {
                out.extra_ticks += d;
                continue;
            }
            let activity = self
                .kinds
                .entry(p.pid)
                .or_insert_with(|| classify(&p.comm, &cmdline(p.pid)))
                .clone();
            if let Some(activity) = activity {
                let age = self.age.entry(p.pid).or_insert(0);
                *age += 1;
                self.seen.insert(activity.clone());
                out.units.push(Unit {
                    activity,
                    cores: if dt > 0.0 {
                        d as f64 / CLK_TCK / dt
                    } else {
                        0.0
                    },
                    secs: *age,
                });
            }
        }
        out.procs = cur.len();
        self.kinds.retain(|pid, _| cur.contains_key(pid));
        self.age.retain(|pid, _| cur.contains_key(pid));
        self.prev = Some(cur);
        if dt > 0.0 {
            out.cores = delta as f64 / CLK_TCK / dt;
        }
        out.units.sort_by(|a, b| b.cores.total_cmp(&a.cores));
        out.units_seen = self.seen.len();
        out
    }
}

/// Aggregate busy and total jiffies from `/proc/stat`.
pub fn system_cpu() -> (u64, u64) {
    let Ok(s) = fs::read_to_string("/proc/stat") else {
        return (0, 0);
    };
    let line = s.lines().next().unwrap_or("");
    let mut busy = 0;
    let mut total = 0;
    for (i, x) in line.split_whitespace().skip(1).enumerate() {
        let v: u64 = x.parse().unwrap_or(0);
        total += v;
        if i != 3 && i != 4 {
            busy += v;
        }
    }
    (busy, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_list_counts_ranges_and_singles() {
        assert_eq!(count_cpu_list("0-19\n"), Some(20));
        assert_eq!(count_cpu_list("0-7,16-23"), Some(16));
        assert_eq!(count_cpu_list("0,2,4-5"), Some(4));
        assert_eq!(count_cpu_list("0"), Some(1));
        assert_eq!(count_cpu_list(""), None);
        assert_eq!(
            count_cpu_list("1200000 100000"),
            None,
            "a cpu.max line is not a cpu list"
        );
        assert_eq!(count_cpu_list("5-3"), None);
    }

    #[test]
    fn host_cpus_ignore_the_cgroup_quota() {
        let capped = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        assert!(host_cpus() >= capped, "{} < {capped}", host_cpus());
    }

    fn p(pid: i32, ppid: i32, ticks: u64) -> Proc {
        Proc {
            pid,
            ppid,
            ticks,
            rss: 10,
            comm: "x".into(),
            start: 0,
        }
    }

    #[test]
    fn parses_comm_with_spaces_and_parens() {
        let line = "42 (my (odd) cmd) S 7 1 1 0 -1 0 0 0 0 0 150 50 0 0 20 0 1 0 1 100 25 0";
        let got = parse_stat(42, line).unwrap();
        assert_eq!(got.comm, "my (odd) cmd");
        assert_eq!(got.ppid, 7);
        assert_eq!(got.ticks, 200);
        assert_eq!(got.rss, 25 * PAGE);
    }

    #[test]
    fn tracker_counts_tree_and_extra() {
        let mut s = Snapshot::default();
        s.insert(p(1, 0, 100));
        s.insert(p(2, 1, 100));
        let mut t = Tracker::default();
        t.sample(&s, 1, &[], 0.0);
        let mut s2 = Snapshot::default();
        s2.insert(p(1, 0, 150));
        s2.insert(p(2, 1, 150));
        s2.insert(p(3, 2, 100));
        let extra = [p(9, 5, 40)];
        let got = t.sample(&s2, 1, &extra, 1.0);
        assert_eq!(got.procs, 4);
        assert_eq!(got.extra_ticks, 40);
        assert!((got.cores - 2.4).abs() < 1e-9, "{}", got.cores);
    }

    #[test]
    fn classifies_build_work() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let cases = [
            (
                "rustc",
                "rustc --crate-name serde_json --edition=2021",
                Some(("compile", "serde_json")),
            ),
            (
                "sccache",
                "sccache rustc --crate-name syn src/lib.rs",
                Some(("compile", "syn")),
            ),
            (
                "build-script-bu",
                "/w/target/debug/build/ring-0123456789abcdef/build-script-build",
                Some(("build-script", "ring")),
            ),
            (
                "slot-3f2a",
                "/w/target/debug/deps/slot-0123456789abcdef --test-threads 4",
                Some(("test", "slot")),
            ),
            ("ld.lld", "ld.lld -o out", Some(("link", "ld.lld"))),
            ("cc", "cc -c foo.c", Some(("native", "cc"))),
            ("cargo", "cargo test", None),
            ("sh", "sh -c true", None),
        ];
        for (comm, line, want) in cases {
            let got = classify(comm, &a(line));
            assert_eq!(
                got.as_ref().map(|g| (g.kind, g.name.as_str())),
                want,
                "{line}"
            );
        }
    }
}
