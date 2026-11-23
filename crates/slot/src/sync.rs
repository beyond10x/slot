//! The sync of remote mode: the repository to its mirror on the build
//! server, by rsync over ssh.
//!
//! Before rsync runs, the client asks the server for sibling mirrors of the
//! same repository (`mirror.rs`); a server that answers gets them as
//! `--copy-dest`, and `--stats` for the count of bytes sent. A server whose
//! slot does not know the step gets the plain sync of older clients.
//!
//! A sync is watched: when rsync neither reads nor writes for the stall
//! window (`SLOT_SYNC_STALL`, 60 s), or runs past the bound
//! (`SLOT_SYNC_LIMIT`, 5 min), it is killed and the caller runs the job on
//! this machine's daemon.

use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};

use crate::mirror;

/// Default stall window: a sync that moves no byte for this long is killed.
pub const DEFAULT_STALL: Duration = Duration::from_secs(60);

/// Default bound on a whole sync.
pub const DEFAULT_LIMIT: Duration = Duration::from_secs(300);

/// How often a running sync is looked at.
const POLL: Duration = Duration::from_millis(100);

/// Where a sync goes.
#[derive(Debug, Clone, Copy)]
pub struct Remote<'a> {
    /// ssh host.
    pub host: &'a str,
    /// ssh command and options.
    pub ssh: &'a [String],
    /// Workspace root on the server.
    pub ws_root: &'a str,
    /// slot executable on the server.
    pub remote_slot: &'a str,
}

/// When a sync is given up.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// No byte read or written for this long.
    pub stall: Duration,
    /// The whole sync, the sibling step included, runs this long.
    pub limit: Duration,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            stall: DEFAULT_STALL,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// What a finished sync did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Synced {
    /// Bytes rsync sent (its `--stats`); `None` from a server too old for it.
    pub bytes: Option<u64>,
    /// Wall time of the sibling step and rsync together.
    pub elapsed: Duration,
    /// Sibling mirrors rsync could copy from.
    pub siblings: usize,
}

impl Synced {
    /// The line the job's caller sees on stderr.
    pub fn line(&self, host: &str) -> String {
        let bytes = self.bytes.map_or_else(
            || "bytes sent not counted (the server's slot is older)".to_string(),
            |b| format!("{b} bytes sent"),
        );
        let sib = match self.siblings {
            0 => String::new(),
            1 => ", 1 sibling mirror as base".to_string(),
            n => format!(", {n} sibling mirrors as base"),
        };
        format!(
            "slot: synced to {host} in {:.1} s, {bytes}{sib}",
            self.elapsed.as_secs_f64()
        )
    }
}

/// Why a watched process was killed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// No byte read or written for this long.
    Stalled(Duration),
    /// It ran past its deadline.
    OverLimit(Duration),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stalled(d) => write!(f, "no byte moved for {} s", d.as_secs()),
            Self::OverLimit(d) => write!(f, "ran over {} s", d.as_secs()),
        }
    }
}

/// Bytes `pid` has read and written so far, from `/proc/<pid>/io`.
fn io_bytes(pid: u32) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let field = |name: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    Some(field("rchar:")? + field("wchar:")?)
}

/// Kills `child` and the processes it started (rsync's ssh), then reaps it.
fn kill_tree(child: &mut Child) {
    let pid = child.id();
    let kids =
        std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap_or_default();
    for k in kids
        .split_whitespace()
        .filter_map(|k| k.parse::<i32>().ok())
    {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(k),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits for `child`. Kills it (and its children) when `stall` is set and
/// its read and written bytes do not change for that long, or when
/// `deadline` passes; `limit` is the bound the deadline stands for, for the
/// message. A process whose counters cannot be read is bounded by the
/// deadline only.
pub fn watch(
    child: &mut Child,
    stall: Option<Duration>,
    deadline: Instant,
    limit: Duration,
) -> Result<ExitStatus, Stop> {
    let pid = child.id();
    let mut last = io_bytes(pid);
    let mut moved = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Ok(st),
            Ok(None) => {}
            Err(_) => {
                kill_tree(child);
                return Err(Stop::OverLimit(limit));
            }
        }
        let now = Instant::now();
        if now >= deadline {
            kill_tree(child);
            return Err(Stop::OverLimit(limit));
        }
        let io = io_bytes(pid);
        if io.is_none() || io != last {
            last = io;
            moved = now;
        } else if let Some(s) = stall
            && now.duration_since(moved) >= s
        {
            kill_tree(child);
            return Err(Stop::Stalled(s));
        }
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(now)));
    }
}

/// `Total bytes sent: 1,234` of rsync's `--stats`.
pub fn stats_bytes_sent(out: &str) -> Option<u64> {
    out.lines()
        .find_map(|l| l.trim().strip_prefix("Total bytes sent:"))
        .map(|v| v.chars().filter(char::is_ascii_digit).collect::<String>())
        .and_then(|d| d.parse().ok())
}

/// Asks the server for the sibling mirrors of `id`. `None` when it does not
/// answer within `wait` or does not know the step (an older slot).
fn ask_siblings(r: &Remote<'_>, id: &str, key: &str, wait: Duration) -> Option<Vec<String>> {
    let (ssh, opts) = r.ssh.split_first()?;
    let mut child = Command::new(ssh)
        .args(opts)
        .arg(r.host)
        .arg(r.remote_slot)
        .args(["__siblings", r.ws_root, id, key])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let st = watch(&mut child, None, Instant::now() + wait, wait).ok()?;
    let out = reader.join().ok()?;
    if !st.success() {
        return None;
    }
    let ids: Vec<String> = out.lines().map(str::trim).map(String::from).collect();
    ids.iter()
        .all(|s| mirror::valid_id(s) && s != id)
        .then(|| ids.into_iter().take(mirror::MAX_SIBLINGS).collect())
}

/// Syncs the checkout at `top` to mirror `id` on the server, with the
/// toolchain's `excludes`, within `b`.
pub fn sync(top: &Path, id: &str, excludes: &[&str], r: &Remote<'_>, b: &Bounds) -> Result<Synced> {
    let t0 = Instant::now();
    let deadline = t0 + b.limit;
    let key = mirror::repo_key(top);
    let siblings = ask_siblings(r, id, &key, b.stall.min(b.limit));
    let mut rsync = Command::new("rsync");
    rsync.args([
        "-a",
        "--delete",
        "--mkpath",
        "--filter=:- .gitignore",
        "--exclude=/.git",
    ]);
    for x in excludes {
        rsync.arg(format!("--exclude={x}"));
    }
    if let Some(sibs) = &siblings {
        rsync.arg("--stats");
        for s in sibs {
            rsync.arg(format!("--copy-dest=../{s}"));
        }
    }
    rsync
        .arg("-e")
        .arg(r.ssh.join(" "))
        .arg(format!("{}/", top.display()))
        .arg(format!("{}:{}/{id}/", r.host, r.ws_root))
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    let mut child = rsync.spawn().map_err(|e| anyhow!("rsync: {e}"))?;
    let mut out = child.stdout.take().ok_or_else(|| anyhow!("rsync stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let st = watch(&mut child, Some(b.stall), deadline, b.limit);
    let out = reader.join().unwrap_or_default();
    let st = st.map_err(|stop| anyhow!("sync killed: {stop}"))?;
    if !st.success() {
        bail!("rsync exited with {st}");
    }
    Ok(Synced {
        bytes: siblings.as_ref().and_then(|_| stats_bytes_sent(&out)),
        elapsed: t0.elapsed(),
        siblings: siblings.map_or(0, |s| s.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsync_stats_give_the_bytes_sent() {
        let out = "Number of files: 3\nTotal bytes sent: 1,234,567\nTotal bytes received: 46\n";
        assert_eq!(stats_bytes_sent(out), Some(1_234_567));
        assert_eq!(stats_bytes_sent("Total bytes sent: 153"), Some(153));
        assert_eq!(stats_bytes_sent("nothing"), None);
    }

    #[test]
    fn the_line_names_bytes_seconds_and_siblings() {
        let s = Synced {
            bytes: Some(4096),
            elapsed: Duration::from_millis(1250),
            siblings: 2,
        };
        assert_eq!(
            s.line("node"),
            "slot: synced to node in 1.2 s, 4096 bytes sent, 2 sibling mirrors as base"
        );
        let old = Synced {
            bytes: None,
            siblings: 0,
            ..s
        };
        assert!(
            old.line("node").contains("not counted"),
            "{}",
            old.line("node")
        );
    }

    #[test]
    fn a_process_that_moves_no_byte_is_killed_at_the_stall_window() {
        let mut c = Command::new("sleep").arg("20").spawn().unwrap();
        let t0 = Instant::now();
        let got = watch(
            &mut c,
            Some(Duration::from_millis(500)),
            t0 + Duration::from_secs(20),
            Duration::from_secs(20),
        );
        assert_eq!(got, Err(Stop::Stalled(Duration::from_millis(500))));
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        assert!(c.try_wait().unwrap().is_some(), "reaped");
    }

    #[test]
    fn a_process_that_keeps_moving_bytes_is_killed_at_the_limit() {
        let mut c = Command::new("sh")
            .args(["-c", "while :; do echo xxxxxxxxxxxxxxxx; sleep 0.05; done"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let t0 = Instant::now();
        let limit = Duration::from_millis(1500);
        let got = watch(&mut c, Some(Duration::from_millis(700)), t0 + limit, limit);
        assert_eq!(got, Err(Stop::OverLimit(limit)), "not a stall: it wrote");
        assert!(t0.elapsed() >= limit && t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_process_that_ends_in_time_gives_its_status() {
        let mut c = Command::new("sh").args(["-c", "exit 3"]).spawn().unwrap();
        let t0 = Instant::now();
        let st = watch(
            &mut c,
            Some(Duration::from_secs(5)),
            t0 + Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(st.code(), Some(3));
    }
}
