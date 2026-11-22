//! Watchdog for the jobserver inside sccache servers.
//!
//! An sccache server limits its own compiles with a private jobserver: a
//! pipe that starts with one token per CPU. Each compile takes a token and
//! gives it back. A token that is never given back is gone for the life of
//! the server; once all are gone every `sccache rustc` on the machine waits
//! behind the compiles still running, at 0 % CPU, which looks like a hung
//! server (seen on 2026-11-12: 0 free tokens and 1 compile on 20 CPUs after
//! 8 days). Restarting the server fails the compiles in flight.
//!
//! The watchdog reads each server's free tokens (`FIONREAD` on the read end
//! of its pipe, opened through `/proc/<pid>/fd`), counts its running compiles
//! (its child processes) and its waiting clients, and writes tokens back only
//! when, for a whole window: no token was free, fewer compiles ran than the
//! pool holds, a client waited, and the server process itself used under a
//! CPU threshold. It writes at most `size − running` tokens.
//!
//! The module also holds what the daemon records about the compile cache:
//! the counters of each running server, once a minute ([`StatsLog`], one
//! [`Counters`] line each in `cache.jsonl`), and per rust job the variables
//! that decide what the cache can do ([`cache_env`]) with the warnings they
//! give ([`cache_warnings`]).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::procstat::{self, Snapshot};

/// Port an sccache client and server use when `SCCACHE_SERVER_PORT` is unset.
pub const DEFAULT_PORT: &str = "4226";

/// How long the stall must last before tokens are written.
pub const WINDOW: Duration = Duration::from_secs(60);

/// Highest CPU share of one core the server process may use in the window.
pub const MAX_CPU: f64 = 0.05;

/// One open file of a process: fd number, link target, open flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fd {
    /// Descriptor number.
    pub fd: i32,
    /// `readlink /proc/<pid>/fd/<fd>`.
    pub target: String,
    /// `flags:` from `/proc/<pid>/fdinfo/<fd>`, octal parsed.
    pub flags: u32,
}

/// The jobserver pipe of a process: `(read fd, write fd)` of the only pipe
/// it holds both ends of, outside stdin, stdout and stderr.
pub fn jobserver_fds(fds: &[Fd]) -> Option<(i32, i32)> {
    let mut pipes: HashMap<&str, (Option<i32>, Option<i32>)> = HashMap::new();
    for f in fds {
        if f.fd <= 2 || !f.target.starts_with("pipe:") {
            continue;
        }
        let e = pipes.entry(f.target.as_str()).or_default();
        match f.flags & 0o3 {
            0 => e.0 = e.0.or(Some(f.fd)),
            1 => e.1 = e.1.or(Some(f.fd)),
            _ => {}
        }
    }
    let mut both = pipes.values().filter_map(|(r, w)| Some(((*r)?, (*w)?)));
    let first = both.next()?;
    both.next().is_none().then_some(first)
}

fn open_fds(pid: i32) -> Vec<Fd> {
    let Ok(dir) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| {
            let fd: i32 = e.file_name().to_str()?.parse().ok()?;
            let target = std::fs::read_link(e.path())
                .ok()?
                .to_string_lossy()
                .into_owned();
            let info = std::fs::read_to_string(format!("/proc/{pid}/fdinfo/{fd}")).ok()?;
            let flags = info
                .lines()
                .find_map(|l| l.strip_prefix("flags:"))
                .and_then(|v| u32::from_str_radix(v.trim(), 8).ok())?;
            Some(Fd { fd, target, flags })
        })
        .collect()
}

fn argv(pid: i32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| {
            b.split(|&c| c == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// What an sccache process is, from its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The server.
    Server,
    /// A client compiling through the server.
    Client,
    /// Anything else (`--show-stats`, `--stop-server`).
    Other,
}

/// The role of a process whose program is sccache; `None` for any other.
pub fn role(argv: &[String], start_server_env: bool) -> Option<Role> {
    let prog = argv.first()?;
    if std::path::Path::new(prog).file_name()?.to_str()? != "sccache" {
        return None;
    }
    Some(match argv.get(1).map(String::as_str) {
        None if start_server_env => Role::Server,
        None => Role::Other,
        Some("--start-server") => Role::Server,
        Some(a) if a.starts_with('-') => Role::Other,
        Some(_) => Role::Client,
    })
}

/// One look at one server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// When.
    pub at: Instant,
    /// Tokens in the pipe.
    pub free: usize,
    /// Compiles running (the server's child processes).
    pub running: usize,
    /// Clients of this server that wait or compile.
    pub clients: usize,
    /// CPU ticks of the server process.
    pub ticks: u64,
}

/// Tokens to write back, or `None` while the server is not stalled for a
/// whole [`WINDOW`]: no free token, fewer compiles than `size`, more clients
/// than compiles, and the server process under [`MAX_CPU`].
pub fn decide(samples: &VecDeque<Sample>, size: usize) -> Option<usize> {
    let last = samples.back()?;
    let first = samples
        .iter()
        .find(|s| last.at.duration_since(s.at) <= WINDOW)?;
    let span = last.at.duration_since(first.at);
    if span + Duration::from_secs(1) < WINDOW {
        return None;
    }
    let window = samples.iter().filter(|s| s.at >= first.at);
    let stalled = window
        .clone()
        .all(|s| s.free == 0 && s.running < size && s.clients > s.running);
    if !stalled {
        return None;
    }
    let cpu = last.ticks.saturating_sub(first.ticks) as f64 / procstat::CLK_TCK;
    if cpu / span.as_secs_f64().max(1.0) >= MAX_CPU {
        return None;
    }
    Some(size - last.running)
}

/// A server and what the watchdog knows of it.
#[derive(Debug, Clone, Default)]
struct Watched {
    samples: VecDeque<Sample>,
    max_seen: usize,
    port: String,
    fds: Option<(i32, i32)>,
}

/// A refill the watchdog made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refill {
    /// Server pid.
    pub pid: i32,
    /// Server port.
    pub port: String,
    /// Tokens written.
    pub tokens: usize,
    /// Compiles running then.
    pub running: usize,
    /// Clients then.
    pub clients: usize,
}

/// Counts of one pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pass {
    /// Servers seen.
    pub servers: usize,
    /// Servers with no free token and fewer compiles than their pool now.
    pub short: usize,
    /// Refills made in this pass.
    pub refills: Vec<Refill>,
}

/// Watches every sccache server of this user.
#[derive(Debug, Default)]
pub struct Watchdog {
    servers: HashMap<i32, Watched>,
    cpus: usize,
}

impl Watchdog {
    /// A watchdog for a host with `cpus` CPUs, the pool size sccache starts with.
    pub fn new(cpus: usize) -> Self {
        Self {
            servers: HashMap::new(),
            cpus,
        }
    }

    /// One pass: sample every server, refill the stalled ones.
    pub fn pass(&mut self, snap: &Snapshot) -> Pass {
        let mut servers = Vec::new();
        let mut clients: HashMap<String, usize> = HashMap::new();
        for (pid, ticks, role, port) in processes(snap) {
            match role {
                Role::Server => servers.push((pid, ticks, port)),
                Role::Client => *clients.entry(port).or_default() += 1,
                Role::Other => {}
            }
        }
        let now = Instant::now();
        let mut pass = Pass {
            servers: servers.len(),
            ..Pass::default()
        };
        let live: Vec<i32> = servers.iter().map(|s| s.0).collect();
        self.servers.retain(|pid, _| live.contains(pid));
        for (pid, ticks, port) in servers {
            let w = self.servers.entry(pid).or_default();
            w.port = port;
            if w.fds.is_none() {
                w.fds = jobserver_fds(&open_fds(pid));
            }
            let Some((rfd, wfd)) = w.fds else { continue };
            let Some(free) = free_tokens(pid, rfd) else {
                continue;
            };
            let running = snap.procs().filter(|c| c.ppid == pid).count();
            let s = Sample {
                at: now,
                free,
                running,
                clients: clients.get(&w.port).copied().unwrap_or(0),
                ticks,
            };
            w.max_seen = w.max_seen.max(free + running);
            w.samples.push_back(s);
            while w
                .samples
                .front()
                .is_some_and(|f| now.duration_since(f.at) > WINDOW + Duration::from_secs(30))
            {
                w.samples.pop_front();
            }
            let size = self.cpus.max(w.max_seen);
            if free == 0 && running < size {
                pass.short += 1;
            }
            if let Some(n) = decide(&w.samples, size)
                && n > 0
                && write_tokens(pid, wfd, n).is_ok()
            {
                pass.refills.push(Refill {
                    pid,
                    port: w.port.clone(),
                    tokens: n,
                    running,
                    clients: s.clients,
                });
                w.samples.clear();
            }
        }
        pass
    }
}

fn owned_by(pid: i32, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(format!("/proc/{pid}")).is_ok_and(|m| m.uid() == uid)
}

/// Every sccache process of this user: pid, CPU ticks, role, and the port
/// in its environment ([`DEFAULT_PORT`] when it has none).
fn processes(snap: &Snapshot) -> Vec<(i32, u64, Role, String)> {
    let uid = nix::unistd::getuid().as_raw();
    snap.procs()
        .filter(|p| p.comm == "sccache" && owned_by(p.pid, uid))
        .filter_map(|p| {
            let env_start = procstat::env_value(p.pid, "SCCACHE_START_SERVER").is_some();
            let role = role(&argv(p.pid), env_start)?;
            let port = procstat::env_value(p.pid, "SCCACHE_SERVER_PORT")
                .unwrap_or_else(|| DEFAULT_PORT.to_string());
            Some((p.pid, p.ticks, role, port))
        })
        .collect()
}

/// Bytes waiting in the read end of the server's pipe.
fn free_tokens(pid: i32, rfd: i32) -> Option<usize> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc_nonblock())
        .open(format!("/proc/{pid}/fd/{rfd}"))
        .ok()?;
    rustix::io::ioctl_fionread(&f).ok().map(|n| n as usize)
}

fn libc_nonblock() -> i32 {
    rustix::fs::OFlags::NONBLOCK.bits() as i32
}

/// Writes `n` tokens into the server's pipe.
fn write_tokens(pid: i32, wfd: i32, n: usize) -> std::io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .custom_flags(libc_nonblock())
        .open(format!("/proc/{pid}/fd/{wfd}"))?;
    f.write_all(&vec![b'|'; n])
}

/// Variables of a rust job's environment the job log records, besides
/// every variable that starts with [`PROFILE_PREFIX`]. sccache's cache key
/// holds the compiler, its flags and every `CARGO_*` variable, so these say
/// whether a job could hit the cache at all.
pub const CACHE_VARS: &[&str] = &[
    "RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_INCREMENTAL",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "SCCACHE_SERVER_PORT",
    "SCCACHE_DIR",
    "RUSTUP_TOOLCHAIN",
];

/// Prefix of cargo's profile overrides, recorded like [`CACHE_VARS`].
pub const PROFILE_PREFIX: &str = "CARGO_PROFILE_";

/// Longest recorded value, in characters; a longer one is cut and ends in `…`.
pub const VALUE_MAX: usize = 200;

/// The part of a job's environment (`KEY=VALUE`, a later entry wins) the
/// job log records: [`CACHE_VARS`] and every `CARGO_PROFILE_*`, nothing
/// else. An empty value stays an empty string.
pub fn cache_env(env: &[String]) -> BTreeMap<String, String> {
    env.iter()
        .filter_map(|e| e.split_once('='))
        .filter(|(k, _)| CACHE_VARS.contains(k) || k.starts_with(PROFILE_PREFIX))
        .map(|(k, v)| (k.to_string(), cut(v)))
        .collect()
}

fn cut(v: &str) -> String {
    if v.chars().count() <= VALUE_MAX {
        return v.to_string();
    }
    let mut s: String = v.chars().take(VALUE_MAX - 1).collect();
    s.push('…');
    s
}

/// What is unusual about a job's compile cache setup, from [`cache_env`]:
/// a wrapper variable set to nothing switches the cache off, and a port
/// other than [`DEFAULT_PORT`] or a cache directory of its own means a
/// second sccache server beside the host's.
pub fn cache_warnings(env: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    // cargo takes `RUSTC_WRAPPER` before `build.rustc-wrapper`, and an empty
    // value of the one it takes means no wrapper.
    let wrapper = ["RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER"]
        .into_iter()
        .find_map(|k| env.get(k).map(|v| (k, v)));
    if let Some((name, "")) = wrapper.map(|(k, v)| (k, v.as_str())) {
        out.push(format!("compile cache off: {name} is empty"));
    }
    if let Some(port) = env.get("SCCACHE_SERVER_PORT").map(|v| port_number(v))
        && port != DEFAULT_PORT_NUMBER
    {
        out.push(format!("private sccache server (port {port})"));
    }
    if env.contains_key("SCCACHE_DIR") {
        out.push("private sccache directory".into());
    }
    out
}

/// [`DEFAULT_PORT`] as a number.
const DEFAULT_PORT_NUMBER: u16 = 4226;

/// The port number in a `SCCACHE_SERVER_PORT` value; [`DEFAULT_PORT`] for a
/// value that is no port, as sccache reads it.
pub fn port_number(value: &str) -> u16 {
    value.parse().unwrap_or(DEFAULT_PORT_NUMBER)
}

/// One line of `cache.jsonl`: the counters of one sccache server at one
/// minute. The counters count from the server's start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counters {
    /// When, Unix milliseconds.
    pub t: i64,
    /// The server's port.
    pub port: u16,
    /// Rust compiles answered from the cache.
    pub rust_hits: u64,
    /// Rust compiles that ran after a miss.
    pub rust_misses: u64,
    /// C and C++ compiles answered from the cache.
    pub c_hits: u64,
    /// C and C++ compiles that ran after a miss.
    pub c_misses: u64,
    /// Calls the server refused to cache, by its reason.
    pub not_cached: BTreeMap<String, u64>,
    /// Bytes in the cache.
    pub cache_size: u64,
    /// The cache's size limit, bytes.
    pub max_cache_size: u64,
    /// A counter is lower than in the record before for the same port: the
    /// server started again in between.
    pub restarted: bool,
}

/// The counters in the output of `sccache --show-stats --stats-format json`.
/// A field the output does not have counts as 0; only text that is not a
/// JSON object is an error. `t`, `port` and `restarted` are left unset.
pub fn parse_stats(text: &str) -> Result<Counters, String> {
    use serde_json::Value;
    let v: Value = serde_json::from_str(text).map_err(|e| format!("its answer is no JSON: {e}"))?;
    if !v.is_object() {
        return Err("its answer is no JSON object".into());
    }
    let number = |v: Option<&Value>| v.and_then(Value::as_u64).unwrap_or(0);
    let stats = v.get("stats");
    let count = |kind: &str, language: &str| {
        number(
            stats
                .and_then(|s| s.get(kind))
                .and_then(|k| k.get("counts"))
                .and_then(|c| c.get(language)),
        )
    };
    let not_cached = stats
        .and_then(|s| s.get("not_cached"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(reason, n)| (reason.clone(), number(Some(n))))
                .collect()
        })
        .unwrap_or_default();
    Ok(Counters {
        rust_hits: count("cache_hits", "Rust"),
        rust_misses: count("cache_misses", "Rust"),
        c_hits: count("cache_hits", "C/C++"),
        c_misses: count("cache_misses", "C/C++"),
        not_cached,
        cache_size: number(v.get("cache_size")),
        max_cache_size: number(v.get("max_cache_size")),
        ..Counters::default()
    })
}

/// Whether a counter of `now` is lower than in `prev`. The cache size is no
/// counter.
pub fn went_down(prev: &Counters, now: &Counters) -> bool {
    now.rust_hits < prev.rust_hits
        || now.rust_misses < prev.rust_misses
        || now.c_hits < prev.c_hits
        || now.c_misses < prev.c_misses
        || prev
            .not_cached
            .iter()
            .any(|(reason, n)| now.not_cached.get(reason).copied().unwrap_or(0) < *n)
}

/// How long one `sccache --show-stats` may take.
pub const STATS_DEADLINE: Duration = Duration::from_secs(5);

/// Bytes at the end of `cache.jsonl` read for the last record of each port.
const TAIL: u64 = 64 * 1024;

/// The newest record of each port in the last [`TAIL`] bytes of the log at
/// `path`; empty when there is none.
pub fn last_records(path: &Path) -> HashMap<u16, Counters> {
    let mut out = HashMap::new();
    let Ok(mut f) = std::fs::File::open(path) else {
        return out;
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let offset = len.saturating_sub(TAIL);
    let mut tail = Vec::new();
    if f.seek(SeekFrom::Start(offset)).is_err() || f.read_to_end(&mut tail).is_err() {
        return out;
    }
    let text = String::from_utf8_lossy(&tail);
    // The first line of a tail that starts inside the file is cut.
    for line in text.lines().skip(usize::from(offset > 0)) {
        if let Ok(c) = serde_json::from_str::<Counters>(line)
            && c.port != 0
        {
            out.insert(c.port, c);
        }
    }
    out
}

/// `sccache --show-stats --stats-format json` for the server on `port`.
pub fn stats_command(exe: &Path, port: u16) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(["--show-stats", "--stats-format", "json"])
        .env("SCCACHE_SERVER_PORT", port.to_string())
        .env_remove("SCCACHE_SERVER_UDS")
        .env_remove("SCCACHE_START_SERVER");
    cmd
}

/// Whether something accepts connections on `127.0.0.1:port`.
pub fn listening(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok()
}

/// Runs `cmd` and returns its standard output; an error says why when it
/// cannot start, exits non-zero or runs longer than `deadline` (it is
/// killed then).
pub fn output_within(mut cmd: Command, deadline: Duration) -> Result<Vec<u8>, String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", cmd.get_program().to_string_lossy()))?;
    let Some(mut stdout) = child.stdout.take() else {
        return Err("no output pipe".into());
    };
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(Some(st)) => return Err(format!("{st}")),
            Err(e) => return Err(e.to_string()),
            Ok(None) if start.elapsed() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer within {} ms", deadline.as_millis()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    reader
        .join()
        .map_err(|_| "reading its output failed".into())
}

/// A running sccache server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Server {
    /// Process id.
    pub pid: i32,
    /// The port in its environment.
    pub port: u16,
    /// CPU ticks of the server process.
    pub ticks: u64,
}

/// The running sccache servers of this user, one per port.
pub fn servers(snap: &Snapshot) -> Vec<Server> {
    let mut seen = HashSet::new();
    let mut out: Vec<Server> = processes(snap)
        .into_iter()
        .filter(|(_, _, role, _)| *role == Role::Server)
        .map(|(pid, ticks, _, port)| Server {
            pid,
            port: port_number(&port),
            ticks,
        })
        .collect();
    out.sort_unstable();
    out.retain(|s| seen.insert(s.port));
    out
}

/// Whether to ask a server whose process has used `ticks` CPU ticks and
/// had used `asked` when this daemon last asked it (`None`: never).
///
/// A server that used no CPU time since is not asked: its counters stand.
/// Every request restarts a server's idle timeout (`SCCACHE_IDLE_TIMEOUT`,
/// 600 s unless set), so a reading each minute would keep every server of
/// the host running for good. Measured with sccache 0.18.0 and a timeout
/// of 15 s: the server still ran after 40 s of readings every 5 s, and was
/// gone 15 s after the last. A reading itself costs the server far less
/// than a tick (50 readings: 0 ticks).
pub fn due(asked: Option<u64>, ticks: u64) -> bool {
    asked.is_none_or(|a| ticks > a)
}

/// The counter log: the last record of each port, to see a restart, the
/// ports whose last reading failed, to report a failure once, and the CPU
/// ticks of each server process when it was last asked.
#[derive(Debug, Default)]
pub struct StatsLog {
    prev: HashMap<u16, Counters>,
    failing: HashSet<u16>,
    asked: HashMap<i32, u64>,
}

/// What one pass over the servers gave.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StatsPass {
    /// One record per server that was asked and answered.
    pub records: Vec<Counters>,
    /// One line per server that failed now and had not failed before.
    pub errors: Vec<String>,
}

impl StatsLog {
    /// A log that continues the one at `path`.
    pub fn continuing(path: &Path) -> Self {
        Self {
            prev: last_records(path),
            ..Self::default()
        }
    }

    /// The record for `stats` read at `t` from the server on `port`.
    pub fn record(&mut self, t: i64, port: u16, stats: Counters) -> Counters {
        let restarted = self.prev.get(&port).is_some_and(|p| went_down(p, &stats));
        let c = Counters {
            t,
            port,
            restarted,
            ..stats
        };
        self.failing.remove(&port);
        self.prev.insert(port, c.clone());
        c
    }

    /// The line to log for a failed reading of `port`: `Some` the first
    /// time, `None` while the port keeps failing.
    pub fn failure(&mut self, port: u16, why: &str) -> Option<String> {
        self.failing.insert(port).then(|| {
            format!(
                "slot: sccache server on port {port}: counters not read ({why}); \
                 skipped until it answers"
            )
        })
    }

    /// Whether `server` is asked in this pass ([`due`]); notes its ticks
    /// when it is.
    pub fn ask(&mut self, server: Server) -> bool {
        let yes = due(self.asked.get(&server.pid).copied(), server.ticks);
        if yes {
            self.asked.insert(server.pid, server.ticks);
        }
        yes
    }

    /// Reads every running server that worked since its last reading. A
    /// server is asked only while its process runs and its port accepts
    /// connections, so the call can start none. sccache 0.18.0 answers
    /// `--show-stats` with zeros when no server listens, which would read
    /// as a restart.
    pub fn pass(&mut self, snap: &Snapshot, t: i64) -> StatsPass {
        let mut pass = StatsPass::default();
        let servers = servers(snap);
        self.failing
            .retain(|port| servers.iter().any(|s| s.port == *port));
        self.asked
            .retain(|pid, _| servers.iter().any(|s| s.pid == *pid));
        for s in servers {
            // An idle server gets no connection at all, not even the probe.
            if !self.ask(s) {
                continue;
            }
            let read = if listening(s.port) {
                output_within(stats_command(&server_exe(s.pid), s.port), STATS_DEADLINE)
                    .and_then(|b| parse_stats(&String::from_utf8_lossy(&b)))
            } else {
                Err("its port does not accept connections".into())
            };
            match read {
                Ok(stats) => pass.records.push(self.record(t, s.port, stats)),
                Err(why) => pass.errors.extend(self.failure(s.port, &why)),
            }
        }
        pass
    }
}

/// The program of process `pid`, or `sccache` from `PATH` when its file is
/// gone (replaced by an upgrade).
fn server_exe(pid: i32) -> PathBuf {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("sccache"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    /// `sccache --show-stats --stats-format json` of sccache 0.18.0, with
    /// synthetic numbers.
    const STATS: &str = r#"{
      "stats": {
        "compile_requests": 4127,
        "requests_not_cacheable": 2298,
        "requests_executed": 1818,
        "cache_errors": {"counts": {"Rust": 5}, "adv_counts": {"rust": 5}},
        "cache_hits": {
          "counts": {"Assembler": 81, "C/C++": 256, "Rust": 856},
          "adv_counts": {"rust": 856, "c [gcc]": 256}
        },
        "cache_misses": {
          "counts": {"Rust": 563, "Assembler": 10, "C/C++": 33},
          "adv_counts": {"rust": 563, "c [gcc]": 33}
        },
        "cache_timeouts": 4,
        "cache_writes": 607,
        "not_cached": {"incremental": 1000, "crate-type": 700, "-": 598}
      },
      "cache_location": "Local disk: \"/cache/sccache\"",
      "cache_size": 21474296535,
      "max_cache_size": 21474836480,
      "version": "0.18.0"
    }"#;

    fn reasons(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    #[test]
    fn stats_are_read_from_the_json_output() {
        assert_eq!(
            parse_stats(STATS),
            Ok(Counters {
                rust_hits: 856,
                rust_misses: 563,
                c_hits: 256,
                c_misses: 33,
                not_cached: reasons(&[("-", 598), ("crate-type", 700), ("incremental", 1000)]),
                cache_size: 21_474_296_535,
                max_cache_size: 21_474_836_480,
                ..Counters::default()
            })
        );
    }

    #[test]
    fn a_field_the_stats_do_not_have_is_zero() {
        let full = parse_stats(STATS).unwrap();
        let without = |pointer: &str, key: &str| {
            let mut v: serde_json::Value = serde_json::from_str(STATS).unwrap();
            let parent = v.pointer_mut(pointer).unwrap().as_object_mut().unwrap();
            assert!(parent.remove(key).is_some(), "{pointer} has no {key}");
            parse_stats(&v.to_string()).unwrap()
        };
        let hits = "/stats/cache_hits/counts";
        let misses = "/stats/cache_misses/counts";
        assert_eq!(
            without(hits, "Rust"),
            Counters {
                rust_hits: 0,
                ..full.clone()
            }
        );
        assert_eq!(
            without(misses, "Rust"),
            Counters {
                rust_misses: 0,
                ..full.clone()
            }
        );
        assert_eq!(
            without(hits, "C/C++"),
            Counters {
                c_hits: 0,
                ..full.clone()
            }
        );
        assert_eq!(
            without(misses, "C/C++"),
            Counters {
                c_misses: 0,
                ..full.clone()
            }
        );
        assert_eq!(
            without("/stats", "cache_hits"),
            Counters {
                rust_hits: 0,
                c_hits: 0,
                ..full.clone()
            }
        );
        assert_eq!(
            without("/stats", "not_cached"),
            Counters {
                not_cached: BTreeMap::new(),
                ..full.clone()
            }
        );
        assert_eq!(
            without("", "cache_size"),
            Counters {
                cache_size: 0,
                ..full.clone()
            }
        );
        assert_eq!(
            without("", "max_cache_size"),
            Counters {
                max_cache_size: 0,
                ..full.clone()
            }
        );
        assert_eq!(
            without("", "stats"),
            Counters {
                cache_size: full.cache_size,
                max_cache_size: full.max_cache_size,
                ..Counters::default()
            }
        );
        assert_eq!(parse_stats("{}"), Ok(Counters::default()));
    }

    #[test]
    fn text_that_is_no_json_object_is_an_error() {
        assert!(parse_stats("").is_err());
        assert!(parse_stats("sccache: error: Connection refused").is_err());
        assert!(parse_stats("[1]").is_err());
    }

    #[test]
    fn a_counter_that_went_down_marks_a_restart() {
        let mut log = StatsLog::default();
        let first = log.record(60_000, 4226, parse_stats(STATS).unwrap());
        assert_eq!(
            (first.t, first.port, first.restarted),
            (60_000, 4226, false)
        );
        assert_eq!(first.rust_hits, 856);
        let mut grown = parse_stats(STATS).unwrap();
        grown.rust_hits += 10;
        grown.cache_size -= 1000;
        let second = log.record(120_000, 4226, grown.clone());
        assert!(
            !second.restarted,
            "counters grew; the cache size is no counter"
        );
        let mut fresh = Counters {
            rust_hits: 3,
            ..grown.clone()
        };
        fresh.not_cached = reasons(&[("incremental", 1000), ("crate-type", 700), ("-", 598)]);
        assert!(log.record(180_000, 4226, fresh.clone()).restarted);
        assert!(
            !log.record(240_000, 4226, fresh.clone()).restarted,
            "compared with the record before, not with the highest"
        );
        let other = log.record(240_000, 4303, Counters::default());
        assert!(!other.restarted, "another port has its own history");
        for lower in [
            Counters {
                rust_misses: fresh.rust_misses - 1,
                ..fresh.clone()
            },
            Counters {
                c_hits: fresh.c_hits - 1,
                ..fresh.clone()
            },
            Counters {
                c_misses: fresh.c_misses - 1,
                ..fresh.clone()
            },
            Counters {
                not_cached: reasons(&[("incremental", 999), ("crate-type", 700), ("-", 598)]),
                ..fresh.clone()
            },
            Counters {
                not_cached: reasons(&[("incremental", 1000), ("-", 598)]),
                ..fresh.clone()
            },
        ] {
            assert!(went_down(&fresh, &lower), "{lower:?}");
        }
        assert!(!went_down(&fresh, &fresh));
    }

    #[test]
    fn the_log_continues_after_the_daemon_started_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.jsonl");
        assert!(last_records(&path).is_empty(), "no log yet");
        let mut log = StatsLog::default();
        let mut text = String::new();
        for (t, port, hits) in [(1, 4226, 5), (2, 4303, 9), (3, 4226, 8)] {
            let c = log.record(
                t,
                port,
                Counters {
                    rust_hits: hits,
                    ..Counters::default()
                },
            );
            text += &serde_json::to_string(&c).unwrap();
            text.push('\n');
        }
        std::fs::write(&path, format!("not json\n{text}")).unwrap();
        let last = last_records(&path);
        assert_eq!(last.len(), 2);
        assert_eq!((last[&4226].t, last[&4226].rust_hits), (3, 8));
        assert_eq!((last[&4303].t, last[&4303].rust_hits), (2, 9));
        let mut next = StatsLog::continuing(&path);
        let lower = Counters {
            rust_hits: 2,
            ..Counters::default()
        };
        assert!(next.record(4, 4226, lower).restarted);
    }

    #[test]
    fn a_record_reads_back_and_an_older_line_without_a_field_reads_as_zero() {
        let c = StatsLog::default().record(7, 4226, parse_stats(STATS).unwrap());
        let line = serde_json::to_string(&c).unwrap();
        for key in [
            "\"t\":7",
            "\"port\":4226",
            "\"rust_hits\":856",
            "\"rust_misses\":563",
            "\"c_hits\":256",
            "\"c_misses\":33",
            "\"not_cached\":{",
            "\"cache_size\":21474296535",
            "\"max_cache_size\":21474836480",
            "\"restarted\":false",
        ] {
            assert!(line.contains(key), "{key} in {line}");
        }
        assert_eq!(serde_json::from_str::<Counters>(&line).unwrap(), c);
        let old: Counters = serde_json::from_str(r#"{"t":1,"port":4226,"later_field":1}"#).unwrap();
        assert_eq!(
            old,
            Counters {
                t: 1,
                port: 4226,
                ..Counters::default()
            }
        );
    }

    #[test]
    fn a_failed_reading_is_reported_once_until_the_port_answers_again() {
        let mut log = StatsLog::default();
        let line = log.failure(4226, "no answer within 5 s").unwrap();
        assert!(
            line.contains("4226") && line.contains("no answer within 5 s"),
            "{line}"
        );
        assert_eq!(log.failure(4226, "no answer within 5 s"), None);
        assert!(log.failure(4303, "exit 2").is_some(), "per port");
        log.record(1, 4226, Counters::default());
        assert!(log.failure(4226, "no answer within 5 s").is_some());
    }

    #[test]
    fn a_server_that_used_no_cpu_time_is_not_asked_again() {
        assert!(due(None, 500), "never asked by this daemon");
        assert!(!due(Some(500), 500), "its counters stand");
        assert!(due(Some(500), 501));
        let mut log = StatsLog::default();
        let s = Server {
            pid: 7,
            port: 4226,
            ticks: 500,
        };
        assert!(log.ask(s));
        assert!(!log.ask(s), "idle since the last reading");
        assert!(log.ask(Server { ticks: 503, ..s }));
        assert!(!log.ask(Server { ticks: 503, ..s }));
        assert!(log.ask(Server { pid: 8, ..s }), "another process");
    }

    #[test]
    fn the_stats_command_names_the_port_and_cannot_be_told_to_be_a_server() {
        let cmd = stats_command(Path::new("/opt/bin/sccache"), 4303);
        assert_eq!(cmd.get_program(), "/opt/bin/sccache");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["--show-stats", "--stats-format", "json"]);
        let envs: HashMap<_, _> = cmd.get_envs().collect();
        assert_eq!(
            envs[std::ffi::OsStr::new("SCCACHE_SERVER_PORT")],
            Some(std::ffi::OsStr::new("4303"))
        );
        assert_eq!(
            envs.get(std::ffi::OsStr::new("SCCACHE_START_SERVER")),
            Some(&None),
            "removed from the daemon's environment"
        );
    }

    #[test]
    fn a_port_listens_or_not() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(listening(port));
        drop(l);
        assert!(!listening(port));
    }

    #[test]
    fn a_command_is_read_within_its_deadline_or_killed() {
        let sh = |script: &str| {
            let mut c = Command::new("sh");
            c.args(["-c", script]);
            c
        };
        assert_eq!(
            output_within(sh("printf '{}'"), Duration::from_secs(5)),
            Ok(b"{}".to_vec())
        );
        let failed = output_within(sh("echo no >&2; exit 2"), Duration::from_secs(5));
        assert!(failed.is_err_and(|e| e.contains('2')));
        let start = Instant::now();
        let hung = output_within(sh("sleep 20"), Duration::from_millis(200));
        assert!(hung.is_err_and(|e| e.contains("no answer")));
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "killed, not waited for"
        );
        let missing = output_within(Command::new("/nonexistent/sccache"), Duration::from_secs(5));
        assert!(missing.is_err());
    }

    #[test]
    fn a_port_value_that_is_no_port_is_the_default() {
        assert_eq!(port_number("4303"), 4303);
        assert_eq!(port_number(DEFAULT_PORT), 4226);
        assert_eq!(port_number(""), 4226);
        assert_eq!(port_number("cache"), 4226);
        assert_eq!(port_number("70000"), 4226);
    }

    fn env(pairs: &[&str]) -> Vec<String> {
        pairs.iter().map(|s| (*s).to_string()).collect()
    }

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn only_the_cache_variables_of_a_job_are_recorded() {
        let got = cache_env(&env(&[
            "PATH=/usr/bin",
            "HOME=/srv/dev",
            "API_TOKEN=secret",
            "RUSTC_WRAPPER=",
            "CARGO_BUILD_RUSTC_WRAPPER=sccache",
            "CARGO_INCREMENTAL=0",
            "RUSTFLAGS=-C target-cpu=native",
            "CARGO_ENCODED_RUSTFLAGS=-Clink-arg=-fuse-ld=lld",
            "SCCACHE_SERVER_PORT=4303",
            "SCCACHE_DIR=/cache/own",
            "RUSTUP_TOOLCHAIN=1.99.1",
            "CARGO_PROFILE_DEV_DEBUG=line-tables-only",
            "CARGO_PROFILE_RELEASE_LTO=",
            "CARGO_HOME=/srv/dev/.cargo",
            "CARGO_TARGET_DIR=/t",
            "CARGO_REGISTRY_TOKEN=secret",
            "SCCACHE_BUCKET=b",
            "RUSTC=/x/rustc",
            "RUSTC_WRAPPER_EXTRA=1",
            "XCARGO_PROFILE_DEV_DEBUG=1",
            "not a pair",
        ]));
        assert_eq!(
            got,
            vars(&[
                ("RUSTC_WRAPPER", ""),
                ("CARGO_BUILD_RUSTC_WRAPPER", "sccache"),
                ("CARGO_INCREMENTAL", "0"),
                ("RUSTFLAGS", "-C target-cpu=native"),
                ("CARGO_ENCODED_RUSTFLAGS", "-Clink-arg=-fuse-ld=lld"),
                ("SCCACHE_SERVER_PORT", "4303"),
                ("SCCACHE_DIR", "/cache/own"),
                ("RUSTUP_TOOLCHAIN", "1.99.1"),
                ("CARGO_PROFILE_DEV_DEBUG", "line-tables-only"),
                ("CARGO_PROFILE_RELEASE_LTO", ""),
            ])
        );
        assert_eq!(cache_env(&env(&["PATH=/usr/bin"])), BTreeMap::new());
        assert_eq!(
            cache_env(&env(&["RUSTFLAGS=a", "RUSTFLAGS=b=c"])),
            vars(&[("RUSTFLAGS", "b=c")]),
            "the later entry is the one the job gets"
        );
    }

    #[test]
    fn a_recorded_value_is_cut_at_200_characters() {
        let exact = "é".repeat(VALUE_MAX);
        let long = "é".repeat(VALUE_MAX + 1);
        let got = cache_env(&[
            format!("RUSTFLAGS={exact}"),
            format!("CARGO_PROFILE_DEV_RUSTFLAGS={long}"),
        ]);
        assert_eq!(got["RUSTFLAGS"], exact, "200 characters fit");
        let cut = &got["CARGO_PROFILE_DEV_RUSTFLAGS"];
        assert_eq!(cut.chars().count(), VALUE_MAX);
        assert!(cut.ends_with('…') && cut.starts_with("éé"), "{cut}");
    }

    #[test]
    fn an_empty_wrapper_a_private_port_and_a_private_directory_are_warned_about() {
        let off = "compile cache off: RUSTC_WRAPPER is empty";
        assert_eq!(cache_warnings(&vars(&[("RUSTC_WRAPPER", "")])), [off]);
        assert_eq!(
            cache_warnings(&vars(&[
                ("RUSTC_WRAPPER", ""),
                ("CARGO_BUILD_RUSTC_WRAPPER", "sccache")
            ])),
            [off],
            "RUSTC_WRAPPER wins over the cargo setting"
        );
        assert_eq!(
            cache_warnings(&vars(&[("CARGO_BUILD_RUSTC_WRAPPER", "")])),
            ["compile cache off: CARGO_BUILD_RUSTC_WRAPPER is empty"]
        );
        assert_eq!(
            cache_warnings(&vars(&[("SCCACHE_SERVER_PORT", "4303")])),
            ["private sccache server (port 4303)"]
        );
        assert_eq!(
            cache_warnings(&vars(&[("SCCACHE_DIR", "/cache/own")])),
            ["private sccache directory"]
        );
        assert_eq!(
            cache_warnings(&vars(&[
                ("RUSTC_WRAPPER", ""),
                ("SCCACHE_SERVER_PORT", "4303"),
                ("SCCACHE_DIR", "/cache/own"),
            ])),
            [
                off,
                "private sccache server (port 4303)",
                "private sccache directory"
            ]
        );
    }

    #[test]
    fn the_usual_setup_gives_no_warning() {
        let none: [&str; 0] = [];
        assert_eq!(cache_warnings(&BTreeMap::new()), none, "nothing set");
        assert_eq!(cache_warnings(&vars(&[("RUSTC_WRAPPER", "sccache")])), none);
        assert_eq!(
            cache_warnings(&vars(&[("CARGO_BUILD_RUSTC_WRAPPER", "/usr/bin/sccache")])),
            none
        );
        assert_eq!(
            cache_warnings(&vars(&[
                ("RUSTC_WRAPPER", "sccache"),
                ("CARGO_BUILD_RUSTC_WRAPPER", "")
            ])),
            none,
            "RUSTC_WRAPPER wins over the cargo setting"
        );
        assert_eq!(
            cache_warnings(&vars(&[("SCCACHE_SERVER_PORT", "4226")])),
            none
        );
        assert_eq!(
            cache_warnings(&vars(&[("SCCACHE_SERVER_PORT", "")])),
            none,
            "no port: sccache uses the default"
        );
        assert_eq!(
            cache_warnings(&vars(&[
                ("CARGO_INCREMENTAL", "0"),
                ("CARGO_PROFILE_DEV_DEBUG", "0"),
                ("RUSTFLAGS", ""),
            ])),
            none
        );
    }

    fn fd(fd: i32, target: &str, flags: u32) -> Fd {
        Fd {
            fd,
            target: target.into(),
            flags,
        }
    }

    #[test]
    fn the_jobserver_is_the_one_pipe_held_at_both_ends() {
        let server = [
            fd(0, "/dev/null", 0o2_000_000),
            fd(1, "/dev/null", 0o2_000_001),
            fd(3, "pipe:[100]", 0o2_000_000),
            fd(4, "pipe:[100]", 0o2_000_001),
            fd(5, "anon_inode:[eventpoll]", 0o2_000_002),
            fd(12, "pipe:[200]", 0o2_004_000),
            fd(16, "pipe:[201]", 0o2_004_000),
        ];
        assert_eq!(jobserver_fds(&server), Some((3, 4)));
        let two = [
            fd(3, "pipe:[100]", 0),
            fd(4, "pipe:[100]", 1),
            fd(7, "pipe:[300]", 0),
            fd(8, "pipe:[300]", 1),
        ];
        assert_eq!(jobserver_fds(&two), None, "two candidates: leave it alone");
        let stdio = [fd(1, "pipe:[9]", 1), fd(0, "pipe:[9]", 0)];
        assert_eq!(jobserver_fds(&stdio), None);
    }

    #[test]
    fn roles() {
        let a = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(role(&a(&["/usr/bin/sccache"]), true), Some(Role::Server));
        assert_eq!(role(&a(&["sccache"]), false), Some(Role::Other));
        assert_eq!(
            role(&a(&["sccache", "--start-server"]), false),
            Some(Role::Server)
        );
        assert_eq!(
            role(&a(&["sccache", "/x/bin/rustc", "--crate-name", "a"]), false),
            Some(Role::Client)
        );
        assert_eq!(
            role(&a(&["sccache", "--show-stats"]), false),
            Some(Role::Other)
        );
        assert_eq!(role(&a(&["rustc"]), false), None);
    }

    fn run(n: u64, every: u64, f: impl Fn(u64) -> (usize, usize, usize, u64)) -> VecDeque<Sample> {
        let t0 = Instant::now();
        (0..n)
            .map(|i| {
                let (free, running, clients, ticks) = f(i);
                Sample {
                    at: t0 + Duration::from_secs(i * every),
                    free,
                    running,
                    clients,
                    ticks,
                }
            })
            .collect()
    }

    #[test]
    fn a_stall_for_the_whole_window_is_refilled_up_to_the_pool() {
        let s = run(13, 5, |_| (0, 1, 6, 0));
        assert_eq!(decide(&s, 20), Some(19));
    }

    #[test]
    fn a_short_stall_is_left_alone() {
        let s = run(6, 5, |_| (0, 1, 6, 0));
        assert_eq!(decide(&s, 20), None, "25 s is less than the window");
    }

    #[test]
    fn a_free_token_a_full_pool_or_no_waiting_client_is_no_stall() {
        assert_eq!(
            decide(&run(13, 5, |i| (usize::from(i == 4), 1, 6, 0)), 20),
            None
        );
        assert_eq!(decide(&run(13, 5, |_| (0, 20, 26, 0)), 20), None);
        assert_eq!(decide(&run(13, 5, |_| (0, 1, 1, 0)), 20), None);
    }

    #[test]
    fn a_busy_server_is_left_alone() {
        // 60 s at 0.1 core = 600 ticks: over the 5 % threshold.
        assert_eq!(decide(&run(13, 5, |i| (0, 1, 6, i * 50)), 20), None);
        // 60 s at 0.02 core = 120 ticks.
        assert_eq!(decide(&run(13, 5, |i| (0, 1, 6, i * 10)), 20), Some(19));
    }

    #[test]
    fn free_tokens_are_read_and_written_through_proc() {
        let (r, w) = std::io::pipe().unwrap();
        let pid = std::process::id() as i32;
        assert_eq!(free_tokens(pid, r.as_raw_fd()), Some(0));
        write_tokens(pid, w.as_raw_fd(), 3).unwrap();
        assert_eq!(free_tokens(pid, r.as_raw_fd()), Some(3));
    }
}
