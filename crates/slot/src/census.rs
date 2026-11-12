//! Every build process on the host, classified, whether slot runs it or
//! not. The daemon samples this each second; the aggregates are what an
//! admission policy is judged against.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;

use crate::procstat::{CLK_TCK, Proc, Snapshot};

/// Environment variables that name the agent session a process belongs to,
/// checked in order.
pub const SESSION_ENV: &[&str] = &["CLAUDE_CODE_SESSION_ID", "CODEX_SESSION_ID"];

/// Build tool family of one process.
pub fn class_of(comm: &str, argv: &[String]) -> Option<&'static str> {
    let exe = argv.first().map_or("", String::as_str);
    let base = exe.rsplit('/').next().unwrap_or(exe);
    if argv.iter().any(|a| a == "--crate-name") || comm == "rustc" {
        return Some("rust-compile");
    }
    if base.starts_with("build-script-") {
        return Some("rust-build-script");
    }
    if exe.contains("/target/") && exe.contains("/deps/") {
        return Some("rust-test");
    }
    if exe.contains("/pkg/tool/") {
        return Some("go-compile");
    }
    let is_go_test = std::path::Path::new(base)
        .extension()
        .is_some_and(|e| e == "test");
    if is_go_test || exe.contains("/go-build") {
        return Some("go-test");
    }
    if matches!(
        comm,
        "ld" | "ld.lld" | "lld" | "ld.gold" | "ld.bfd" | "mold" | "collect2"
    ) {
        return Some("link");
    }
    if matches!(
        comm,
        "cc" | "c++" | "gcc" | "g++" | "clang" | "clang++" | "cc1" | "cc1plus"
    ) {
        return Some("native");
    }
    if comm == "node" || comm.starts_with("node-") || comm == "MainThread" || base == "node" {
        let joined = argv.join(" ");
        for (needle, class) in [
            ("vitest", "node-test"),
            ("jest", "node-test"),
            ("playwright", "node-e2e"),
            ("vue-tsc", "node-typecheck"),
            ("/tsc", "node-typecheck"),
            ("eslint", "node-lint"),
            ("docusaurus", "node-build"),
            ("vite", "node-build"),
            ("webpack", "node-build"),
        ] {
            if joined.contains(needle) {
                return Some(class);
            }
        }
        return None;
    }
    match comm {
        "helm" => Some("helm"),
        "buildkitd" | "docker-buildx" | "buildctl" => Some("container-build"),
        _ => None,
    }
}

/// The command line of `pid`; empty when it cannot be read.
pub fn cmdline(pid: i32) -> Vec<String> {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| {
            b.split(|&c| c == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The agent session `pid` belongs to ([`SESSION_ENV`]); empty when none.
pub fn session_of(pid: i32) -> String {
    let Ok(b) = fs::read(format!("/proc/{pid}/environ")) else {
        return String::new();
    };
    for key in SESSION_ENV {
        let pre = format!("{key}=");
        if let Some(v) = b
            .split(|&c| c == 0)
            .find_map(|kv| kv.strip_prefix(pre.as_bytes()))
        {
            return String::from_utf8_lossy(v).into_owned();
        }
    }
    String::new()
}

/// Pressure stall information: share of time some task waited, last 10 s.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pressure {
    /// CPU `some avg10`, percent.
    pub cpu: f64,
    /// Memory `some avg10`, percent.
    pub memory: f64,
    /// IO `some avg10`, percent.
    pub io: f64,
}

fn psi_some_avg10(text: &str) -> f64 {
    text.lines()
        .find(|l| l.starts_with("some"))
        .and_then(|l| {
            l.split_whitespace()
                .find_map(|f| f.strip_prefix("avg10="))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0.0)
}

/// Reads `/proc/pressure/{cpu,memory,io}`; zeros where unavailable.
pub fn pressure() -> Pressure {
    let read = |name: &str| {
        fs::read_to_string(format!("/proc/pressure/{name}")).map_or(0.0, |t| psi_some_avg10(&t))
    };
    Pressure {
        cpu: read("cpu"),
        memory: read("memory"),
        io: read("io"),
    }
}

/// One-minute load average.
pub fn load1() -> f64 {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
        .unwrap_or(0.0)
}

#[derive(Debug, Clone)]
struct Known {
    start: u64,
    class: Option<&'static str>,
    session: String,
}

/// Cores and memory of one group of build processes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Group {
    /// Cores used over the last interval.
    pub cores: f64,
    /// Resident memory, bytes.
    pub rss: u64,
    /// Processes.
    pub procs: u32,
    /// Cores of processes slot does not run.
    pub unmanaged_cores: f64,
}

/// One census sample.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    /// By (class, managed by slot).
    pub classes: BTreeMap<(&'static str, bool), Group>,
    /// By session id ("" = no session), unmanaged and managed together.
    pub sessions: BTreeMap<String, Group>,
    /// Cores of every process on the host over the interval.
    pub host_cores: f64,
    /// Pressure stall information.
    pub pressure: Pressure,
    /// One-minute load average.
    pub load1: f64,
}

impl Sample {
    /// Cores of all build processes, and of those slot does not run.
    pub fn build_cores(&self) -> (f64, f64) {
        let mut all = 0.0;
        let mut unmanaged = 0.0;
        for ((_, managed), g) in &self.classes {
            all += g.cores;
            if !managed {
                unmanaged += g.cores;
            }
        }
        (all, unmanaged)
    }
}

/// Turns successive host snapshots into census samples.
#[derive(Debug, Default)]
pub struct Census {
    prev: HashMap<i32, u64>,
    known: HashMap<i32, Known>,
}

impl Census {
    /// Samples the whole host. `managed` holds every pid inside a slot job.
    pub fn sample(&mut self, s: &Snapshot, managed: &HashSet<i32>, dt: f64) -> Sample {
        let mut out = Sample {
            pressure: pressure(),
            load1: load1(),
            ..Sample::default()
        };
        let mut cur = HashMap::new();
        let mut total = 0u64;
        for p in s.procs() {
            let d = match self.prev.get(&p.pid) {
                Some(&old) if p.ticks >= old => p.ticks - old,
                Some(_) => p.ticks,
                None if self.prev.is_empty() => 0,
                None => p.ticks.min((dt * CLK_TCK * 64.0) as u64),
            };
            cur.insert(p.pid, p.ticks);
            total += d;
            let k = self.classify(p);
            let Some(class) = k.class else { continue };
            let cores = if dt > 0.0 {
                d as f64 / CLK_TCK / dt
            } else {
                0.0
            };
            let is_managed = managed.contains(&p.pid);
            for g in [
                out.classes.entry((class, is_managed)).or_default(),
                out.sessions.entry(k.session.clone()).or_default(),
            ] {
                g.cores += cores;
                g.rss += p.rss;
                g.procs += 1;
                if !is_managed {
                    g.unmanaged_cores += cores;
                }
            }
        }
        self.known.retain(|pid, _| cur.contains_key(pid));
        self.prev = cur;
        if dt > 0.0 {
            out.host_cores = total as f64 / CLK_TCK / dt;
        }
        out
    }

    fn classify(&mut self, p: &Proc) -> Known {
        if let Some(k) = self.known.get(&p.pid)
            && k.start == p.start
        {
            return k.clone();
        }
        let class = class_of(&p.comm, &cmdline(p.pid));
        let session = if class.is_some() {
            session_of(p.pid)
        } else {
            String::new()
        };
        let k = Known {
            start: p.start,
            class,
            session,
        };
        self.known.insert(p.pid, k.clone());
        k
    }
}

/// The title of a Claude Code session, read from its local transcript
/// (`~/.claude/projects/*/<id>.jsonl`, last `customTitle`); empty when
/// there is none. Slow for long transcripts: call it off the hot path.
pub fn session_title(id: &str) -> String {
    use std::io::{BufRead, BufReader};
    if id.is_empty() || id.contains('/') {
        return String::new();
    }
    let Some(home) = std::env::var_os("HOME") else {
        return String::new();
    };
    let root = std::path::Path::new(&home).join(".claude/projects");
    let Ok(dirs) = fs::read_dir(root) else {
        return String::new();
    };
    let needle = "\"customTitle\":\"";
    for d in dirs.flatten() {
        let Ok(f) = fs::File::open(d.path().join(format!("{id}.jsonl"))) else {
            continue;
        };
        let mut title = String::new();
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Some(i) = line.find(needle) {
                let rest = &line[i + needle.len()..];
                if let Some(end) = rest.find('"') {
                    title = rest[..end].to_string();
                }
            }
        }
        return title;
    }
    String::new()
}

/// Wire view of a sample, busiest groups first.
pub fn view<S: std::hash::BuildHasher>(
    s: &Sample,
    titles: &HashMap<String, String, S>,
) -> crate::types::HostCensus {
    use crate::types::{CensusClass, CensusSession, HostCensus};
    let (build, unmanaged) = s.build_cores();
    let mut classes: Vec<CensusClass> = s
        .classes
        .iter()
        .map(|((class, managed), g)| CensusClass {
            class: (*class).to_string(),
            managed: *managed,
            cores: g.cores,
            rss: g.rss,
            procs: g.procs,
        })
        .collect();
    classes.sort_by(|a, b| b.cores.total_cmp(&a.cores));
    let mut sessions: Vec<CensusSession> = s
        .sessions
        .iter()
        .map(|(id, g)| CensusSession {
            session: id.clone(),
            title: titles.get(id).cloned().unwrap_or_default(),
            cores: g.cores,
            unmanaged_cores: g.unmanaged_cores,
            rss: g.rss,
            procs: g.procs,
        })
        .collect();
    sessions.sort_by(|a, b| b.cores.total_cmp(&a.cores));
    HostCensus {
        host_cores: s.host_cores,
        build_cores: build,
        unmanaged_cores: unmanaged,
        psi_cpu: s.pressure.cpu,
        psi_memory: s.pressure.memory,
        psi_io: s.pressure.io,
        load1: s.load1,
        classes,
        sessions,
        recommended_tokens: 0,
        unmanaged_smoothed: 0.0,
    }
}

/// Averages samples over a window for the census log.
#[derive(Debug, Default)]
pub struct Window {
    n: u32,
    start_ms: i64,
    host: f64,
    psi_cpu: f64,
    psi_memory: f64,
    psi_io: f64,
    load1: f64,
    classes: BTreeMap<(&'static str, bool), f64>,
    sessions: BTreeMap<String, (f64, f64)>,
    tokens_used: f64,
    waiting: f64,
    recommended: f64,
}

impl Window {
    /// Adds one sample with the pool state at that moment.
    pub fn add(&mut self, now_ms: i64, s: &Sample, tokens_used: usize, waiting: usize) {
        if self.n == 0 {
            self.start_ms = now_ms;
        }
        self.n += 1;
        self.host += s.host_cores;
        self.psi_cpu += s.pressure.cpu;
        self.psi_memory += s.pressure.memory;
        self.psi_io += s.pressure.io;
        self.load1 += s.load1;
        self.tokens_used += tokens_used as f64;
        self.waiting += waiting as f64;
        for (k, g) in &s.classes {
            *self.classes.entry(*k).or_default() += g.cores;
        }
        for (k, g) in &s.sessions {
            let e = self.sessions.entry(k.clone()).or_default();
            e.0 += g.cores;
            e.1 += g.unmanaged_cores;
        }
    }

    /// Adds the controller recommendation for the latest sample.
    pub fn recommend(&mut self, tokens: u32) {
        self.recommended += f64::from(tokens);
    }

    /// Samples in the window.
    pub fn len(&self) -> u32 {
        self.n
    }

    /// Whether the window holds no sample.
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// One JSON line with the window's means; resets the window.
    pub fn flush(&mut self) -> serde_json::Value {
        let n = f64::from(self.n.max(1));
        let r = |v: f64| (v / n * 100.0).round() / 100.0;
        let classes: serde_json::Map<String, serde_json::Value> = self
            .classes
            .iter()
            .map(|((c, m), v)| {
                let key = format!("{c}{}", if *m { "@slot" } else { "" });
                (key, serde_json::json!(r(*v)))
            })
            .collect();
        let sessions: serde_json::Map<String, serde_json::Value> = self
            .sessions
            .iter()
            .filter(|(_, (c, _))| *c / n >= 0.05)
            .map(|(k, (c, u))| (k.clone(), serde_json::json!([r(*c), r(*u)])))
            .collect();
        let line = serde_json::json!({
            "t": self.start_ms,
            "n": self.n,
            "host_cores": r(self.host),
            "psi_cpu": r(self.psi_cpu),
            "psi_memory": r(self.psi_memory),
            "psi_io": r(self.psi_io),
            "load1": r(self.load1),
            "tokens_used": r(self.tokens_used),
            "waiting": r(self.waiting),
            "recommended_tokens": r(self.recommended),
            "classes": classes,
            "sessions": sessions,
        });
        *self = Self::default();
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    #[test]
    fn classes_cover_the_build_tools() {
        for (comm, line, want) in [
            ("rustc", "rustc --crate-name x", Some("rust-compile")),
            (
                "sccache",
                "sccache rustc --crate-name x",
                Some("rust-compile"),
            ),
            (
                "build-script-bu",
                "/w/target/debug/build/ring-0123456789abcdef/build-script-build",
                Some("rust-build-script"),
            ),
            (
                "slot-0123",
                "/w/target/debug/deps/slot-0123456789abcdef",
                Some("rust-test"),
            ),
            (
                "compile",
                "/usr/lib/go/pkg/tool/linux_amd64/compile -o x",
                Some("go-compile"),
            ),
            (
                "pkg.test",
                "/tmp/go-build123/b001/pkg.test -test.v",
                Some("go-test"),
            ),
            (
                "node",
                "node /w/node_modules/.bin/vitest run",
                Some("node-test"),
            ),
            (
                "node",
                "node /w/node_modules/.bin/vue-tsc --noEmit",
                Some("node-typecheck"),
            ),
            (
                "node",
                "node /w/node_modules/.bin/playwright test",
                Some("node-e2e"),
            ),
            ("node", "node server.js", None),
            ("helm", "helm template x", Some("helm")),
            ("ld.lld", "ld.lld -o x", Some("link")),
            ("cargo", "cargo test", None),
            ("bash", "bash -c x", None),
        ] {
            assert_eq!(class_of(comm, &a(line)), want, "{line}");
        }
    }

    #[test]
    fn parses_psi() {
        let t = "some avg10=12.50 avg60=7.95 avg300=24.10 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
        assert!((psi_some_avg10(t) - 12.5).abs() < 1e-9);
    }
}
