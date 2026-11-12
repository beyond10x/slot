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

use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

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
        let uid = nix::unistd::getuid().as_raw();
        let mut servers = Vec::new();
        let mut clients: HashMap<String, usize> = HashMap::new();
        for p in snap.procs() {
            if p.comm != "sccache" || !owned_by(p.pid, uid) {
                continue;
            }
            let a = argv(p.pid);
            let env_start = procstat::env_value(p.pid, "SCCACHE_START_SERVER").is_some();
            let port = procstat::env_value(p.pid, "SCCACHE_SERVER_PORT")
                .unwrap_or_else(|| DEFAULT_PORT.to_string());
            match role(&a, env_start) {
                Some(Role::Server) => servers.push((p.pid, p.ticks, port)),
                Some(Role::Client) => *clients.entry(port).or_default() += 1,
                _ => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

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
