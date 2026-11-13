//! Handing running jobs to the re-executed daemon (`docs/design.md` § 15.2).
//!
//! On reload the old process stops the jobserver reader and the output
//! pumps between two reads, flushes nothing to clients but hands every
//! unsent byte on, writes [`State`] to a file, clears `FD_CLOEXEC` on the
//! descriptors it lists and re-executes with [`ENV`] naming the file. The
//! job processes stay children of the same pid, so the new image reaps them.
//!
//! The new process calls [`take_inherited`] before it opens anything. When
//! the file cannot be read it closes every inherited descriptor and kills the
//! listed jobs' process groups, so no client waits on a connection nobody
//! serves and no job runs outside the pool.

use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use nix::fcntl::{FcntlArg, FdFlag, fcntl};
use serde::{Deserialize, Serialize};

use crate::jobserver::PoolState;
use crate::proto::Request;
use crate::types::Job;

/// Format version this release writes and reads.
pub const VERSION: u32 = 1;

/// Environment variable naming the state file in the re-executed process.
pub const ENV: &str = "SLOT_HANDOVER";

/// State file name under the state directory.
pub const FILE: &str = "handover.json";

/// The first release that reads a handover: an older binary installed over
/// the path gets the bounded hold instead.
pub const SINCE: (u64, u64, u64) = (0, 11, 0);

/// Everything the new process takes over.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// [`VERSION`] of the writer.
    pub version: u32,
    /// Descriptor of the listening socket.
    pub listener: i32,
    /// Descriptor of the jobserver FIFO. It stays open across exec: a FIFO
    /// that no process holds open loses the tokens in it.
    pub fifo: i32,
    /// The jobserver pool's counts.
    pub pool: PoolState,
    /// Last job id handed out.
    pub next_id: i64,
    /// Client connections, referenced by index.
    pub conns: Vec<Conn>,
    /// Jobs whose process runs or ended unreaped.
    pub running: Vec<Running>,
    /// Jobs waiting for admission or slots, oldest first.
    pub queued: Vec<Queued>,
    /// A reload was asked for while this handover ran: the new process
    /// hands over again once it serves.
    #[serde(default)]
    pub reload: bool,
}

/// One client connection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conn {
    /// Descriptor of the socket.
    pub fd: i32,
    /// User id of the client.
    #[serde(default)]
    pub uid: Option<u32>,
    /// Frames serialized but not yet written, sent first.
    #[serde(default)]
    pub pending: Vec<u8>,
}

/// A job whose process was started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Running {
    /// The job record.
    pub job: Job,
    /// Process id, a child of the daemon.
    pub pid: i32,
    /// Tokens the job holds.
    pub held: usize,
    /// Read end of the job's stdout pipe; `None` once it reached its end.
    #[serde(default)]
    pub stdout: Option<i32>,
    /// Read end of the job's stderr pipe.
    #[serde(default)]
    pub stderr: Option<i32>,
    /// Path rewrite of remote jobs: from, to.
    #[serde(default)]
    pub rewrite: (String, String),
    /// Set when the old process reaped the job already.
    #[serde(default)]
    pub exited: Option<Exited>,
    /// Index into [`State::conns`].
    #[serde(default)]
    pub conn: Option<usize>,
}

/// Exit of a job the old process reaped.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Exited {
    /// Exit code, 128 + signal for a signalled process.
    pub code: i32,
    /// CPU seconds of the job's process and reaped descendants.
    pub cpu: Option<f64>,
}

/// A job that had not started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Queued {
    /// Job id, kept.
    pub id: i64,
    /// When it was queued, Unix milliseconds.
    pub enqueued_ms: i64,
    /// The client's request.
    pub req: Request,
    /// Index into [`State::conns`].
    #[serde(default)]
    pub conn: Option<usize>,
}

impl State {
    /// Every descriptor number the state lists.
    pub fn fds(&self) -> Vec<i32> {
        let mut v = vec![self.listener, self.fifo];
        v.extend(self.conns.iter().map(|c| c.fd));
        for r in &self.running {
            v.extend(r.stdout);
            v.extend(r.stderr);
        }
        v
    }
}

/// Writes `state` to `dir/FILE`, mode 0600, and returns the path.
pub fn write(dir: &Path, state: &State) -> Result<PathBuf> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let path = dir.join(FILE);
    let tmp = dir.join(format!("{FILE}.tmp"));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("create {}", tmp.display()))?;
    f.write_all(&serde_json::to_vec(state)?)?;
    f.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Reads and deletes the state file.
pub fn read(path: &Path) -> Result<State> {
    let b = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let _ = std::fs::remove_file(path);
    let s: State = serde_json::from_slice(&b).context("parse handover state")?;
    if s.version != VERSION {
        bail!(
            "handover version {} (this release reads {VERSION})",
            s.version
        );
    }
    Ok(s)
}

/// Clears `FD_CLOEXEC` so `fd` stays open in the re-executed image.
pub fn keep_on_exec(fd: BorrowedFd<'_>) -> Result<()> {
    fcntl(fd, FcntlArg::F_SETFD(FdFlag::empty())).context("clear FD_CLOEXEC")?;
    Ok(())
}

/// Whether the binary at `exe` reads a handover: its `--version` is
/// [`SINCE`] or later.
pub fn supports(exe: &Path) -> bool {
    Command::new(exe)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .env_remove(ENV)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| parse_version(&String::from_utf8_lossy(&o.stdout)))
        .is_some_and(|v| v >= SINCE)
}

/// `slot 1.2.3` → `(1, 2, 3)`.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let v = s.split_whitespace().nth(1)?;
    let v = v.split(['-', '+']).next()?;
    let mut it = v.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

/// What the new process inherited: the state and its descriptors, owned.
#[derive(Debug, Default)]
pub struct Inherited {
    /// The state as written.
    pub state: State,
    /// Adopted descriptors by their number in the state.
    pub fds: HashMap<i32, OwnedFd>,
}

impl Inherited {
    /// Takes the descriptor the state listed as `n`.
    pub fn take(&mut self, n: Option<i32>) -> Option<OwnedFd> {
        n.and_then(|n| self.fds.remove(&n))
    }
}

/// SIGHUPs `slot serve` received that the server has not acted on yet.
static HUP: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicBool>> =
    std::sync::OnceLock::new();

/// Catches SIGHUP from the start of `slot serve`, before the server's own
/// handler exists. Without it a reload asked for while the new image
/// starts (it may take seconds under load) would end the daemon, since
/// SIGHUP's default action is to terminate. Call right after
/// [`take_inherited`].
pub fn catch_hup() -> Result<()> {
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGHUP, std::sync::Arc::clone(&flag))
        .context("catch SIGHUP")?;
    let _ = HUP.set(flag);
    Ok(())
}

/// Whether a SIGHUP came since the last call; clears it.
pub fn take_hup() -> bool {
    HUP.get()
        .is_some_and(|f| f.swap(false, std::sync::atomic::Ordering::SeqCst))
}

/// Ignores SIGHUP until the new image installs [`catch_hup`]: an ignored
/// disposition survives exec, a handler does not. Call before the state is
/// written, then [`take_hup`] for a reload asked for meanwhile.
pub fn ignore_hup() -> Result<()> {
    sig::ignore_hup()
}

/// Call first thing in `slot serve`, before any descriptor is opened. With
/// no [`ENV`] in the environment it returns `None` and does nothing.
/// Otherwise it reads the state and adopts its descriptors; when the state
/// cannot be read it closes every inherited descriptor above 2, kills the
/// process group of every child of this process, and returns `None`.
pub fn take_inherited() -> Option<Inherited> {
    let path = std::env::var_os(ENV)?;
    match read(Path::new(&path)) {
        Ok(state) => {
            let mut fds = HashMap::new();
            for n in state.fds() {
                match fd::adopt(n) {
                    Ok(f) => {
                        fds.insert(n, f);
                    }
                    Err(e) => eprintln!("slot: handover: descriptor {n}: {e:#}"),
                }
            }
            fd::close_others(&fds.keys().copied().collect::<Vec<_>>());
            Some(Inherited { state, fds })
        }
        Err(e) => {
            eprintln!("slot: handover state unreadable ({e:#}); stopping inherited jobs");
            fd::close_others(&[]);
            kill_children();
            None
        }
    }
}

/// Gives up a handover that was read but cannot be used (the pool differs):
/// kills the listed jobs' process groups and closes every descriptor, so
/// clients see the connection end.
pub fn abandon(h: Inherited) {
    for r in &h.state.running {
        kill_group(r.pid);
    }
    drop(h);
}

fn kill_group(pid: i32) {
    let p = nix::unistd::Pid::from_raw(pid);
    let _ = nix::sys::signal::killpg(p, nix::sys::signal::Signal::SIGKILL);
    eprintln!("slot: killed inherited job process group {pid}");
    std::thread::spawn(move || {
        let _ = nix::sys::wait::waitpid(p, None);
    });
}

/// Kills the process group of every child of this process and reaps it in
/// the background: jobs of a handover that could not be read.
fn kill_children() {
    let me = std::process::id() as i32;
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return;
    };
    let mut kids = Vec::new();
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
            continue;
        };
        if rest
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse::<i32>().ok())
            == Some(me)
        {
            kids.push(pid);
        }
    }
    for pid in kids {
        kill_group(pid);
    }
}

/// Setting a signal disposition is `unsafe` in nix; ignoring is safe here
/// since no handler code runs.
#[allow(unsafe_code)]
mod sig {
    use anyhow::{Context as _, Result};
    use nix::sys::signal::{SigHandler, Signal, signal};

    pub(super) fn ignore_hup() -> Result<()> {
        // SAFETY: SIG_IGN runs no code in signal context.
        unsafe { signal(Signal::SIGHUP, SigHandler::SigIgn) }
            .map(|_| ())
            .context("ignore SIGHUP")
    }
}

/// The only code in slot that turns descriptor numbers into owned
/// descriptors. Called before the process opens anything else.
#[allow(unsafe_code)]
mod fd {
    use std::os::fd::{FromRawFd as _, OwnedFd};

    use anyhow::{Result, bail};

    /// What `/proc/self/fd/<n>` links to.
    fn target(n: i32) -> Option<String> {
        std::fs::read_link(format!("/proc/self/fd/{n}"))
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// Takes ownership of inherited descriptor `n` and sets `FD_CLOEXEC` on
    /// it again. Only sockets, pipes and named FIFOs are taken.
    pub fn adopt(n: i32) -> Result<OwnedFd> {
        if n <= 2 {
            bail!("refusing standard descriptor {n}");
        }
        let Some(t) = target(n) else {
            bail!("not open");
        };
        let fifo = t.starts_with('/')
            && std::fs::metadata(&t)
                .is_ok_and(|m| std::os::unix::fs::FileTypeExt::is_fifo(&m.file_type()));
        if !(t.starts_with("socket:") || t.starts_with("pipe:") || fifo) {
            bail!("unexpected kind {t}");
        }
        // SAFETY: `n` is open (its /proc link exists), was inherited across
        // exec and is listed once in the state; nothing in this process has
        // taken it yet, since this runs before any other descriptor is
        // opened and each number is adopted at most once.
        let fd = unsafe { OwnedFd::from_raw_fd(n) };
        nix::fcntl::fcntl(
            &fd,
            nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC),
        )?;
        Ok(fd)
    }

    /// Closes every open descriptor above 2 except `keep`.
    pub fn close_others(keep: &[i32]) {
        let mut open = Vec::new();
        if let Ok(dir) = std::fs::read_dir("/proc/self/fd") {
            for e in dir.flatten() {
                if let Some(n) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) {
                    open.push(n);
                }
            }
        }
        for n in open {
            // The directory iterator's own descriptor is closed by now.
            if n <= 2 || keep.contains(&n) || target(n).is_none() {
                continue;
            }
            // SAFETY: inherited across exec and owned by nothing in this
            // process, which has opened no descriptor yet.
            drop(unsafe { OwnedFd::from_raw_fd(n) });
        }
    }
}

/// Descriptor number of `f`, for the state file.
pub fn raw(f: &impl AsFd) -> i32 {
    use std::os::fd::AsRawFd as _;
    f.as_fd().as_raw_fd()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("slot 0.7.0\n"), Some((0, 7, 0)));
        assert_eq!(parse_version("slot 1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("slot"), None);
        assert!(parse_version("slot 0.6.9").unwrap() < SINCE);
    }

    #[test]
    fn state_round_trips_and_a_newer_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let s = State {
            version: VERSION,
            listener: 7,
            next_id: 41,
            conns: vec![Conn {
                fd: 9,
                uid: Some(1000),
                pending: b"{\"t\":\"out\",\"b\":\"x\"}\n".to_vec(),
            }],
            ..State::default()
        };
        let p = write(dir.path(), &s).unwrap();
        let mode =
            std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&p).unwrap().permissions());
        assert_eq!(mode & 0o777, 0o600);
        let back = read(&p).unwrap();
        assert!(!p.exists(), "read deletes the file");
        assert_eq!(back.fds(), vec![7, 0, 9]);
        assert_eq!(back.conns[0].pending, s.conns[0].pending);
        let newer = State {
            version: VERSION + 1,
            ..State::default()
        };
        let p = write(dir.path(), &newer).unwrap();
        assert!(read(&p).is_err());
    }
}
