//! The client side of `slot <command…>`.
//!
//! Local mode talks to the daemon on this machine through its unix socket.
//! Remote mode (`SLOT_HOST` set) rsyncs the repository to the server and
//! talks to the daemon through `ssh $SLOT_HOST slot proxy`. When the daemon
//! cannot be reached the command runs locally and slot says so on stderr.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use std::fmt::Write as _;

use anyhow::{Context as _, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

use crate::paths;
use crate::proto::{Frame, Request, Workspace};
use crate::repo_config::{self, RepoConfig};
use crate::toolchain;

/// How long a queued job waits for an upgrading daemon to come back.
pub const UPGRADE_WAIT: Duration = Duration::from_secs(600);

/// Interval between connection attempts while the daemon upgrades.
const UPGRADE_POLL: Duration = Duration::from_millis(500);

/// The signal that ended this client (SIGINT, SIGTERM or SIGHUP), 0 while none.
static SIGNALLED: AtomicI32 = AtomicI32::new(0);

/// Closes the current connection to the daemon, so it cancels the job.
type Closer = Box<dyn FnMut() + Send>;
static CLOSER: Mutex<Option<Closer>> = Mutex::new(None);

/// The signal that ended this client, if one did.
fn signalled() -> Option<i32> {
    match SIGNALLED.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// Installs, once per process, the thread that handles SIGINT, SIGTERM and
/// SIGHUP: it records the signal, then closes the current connection (the
/// daemon cancels the job), then exits with 128 + signal. The record comes
/// first, so the main thread, which sees the connection close, never takes
/// that for a daemon upgrade and never sends the job again.
fn handle_signals() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP]) else {
            return;
        };
        std::thread::spawn(move || {
            if let Some(sig) = signals.forever().next() {
                SIGNALLED.store(sig, Ordering::SeqCst);
                if let Some(close) = CLOSER.lock().ok().as_mut().and_then(|c| c.as_mut()) {
                    close();
                }
                std::process::exit(128 + sig);
            }
        });
    });
}

/// Client settings, read from `SLOT_*` variables.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Remote ssh host; `None` = local daemon.
    pub host: Option<String>,
    /// ssh command and options.
    pub ssh: Vec<String>,
    /// Remote workspace root.
    pub ws_root: String,
    /// slot executable on the remote host.
    pub remote_slot: String,
    /// Extra variable names forwarded to a remote host.
    pub extra_env: HashSet<String>,
    /// Local daemon socket.
    pub socket: PathBuf,
}

impl Settings {
    /// Reads `SLOT_HOST`, `SLOT_SSH`, `SLOT_WSROOT`, `SLOT_REMOTE`,
    /// `SLOT_ENV` and `SLOT_SOCKET`.
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let ssh = var("SLOT_SSH").map_or_else(
            || {
                let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| ".".into());
                [
                    "ssh",
                    "-o",
                    "ControlMaster=auto",
                    "-o",
                    "ControlPersist=10m",
                    "-o",
                    &format!("ControlPath={runtime}/slot-ssh-%C"),
                    "-o",
                    "ConnectTimeout=3",
                ]
                .iter()
                .map(|s| (*s).to_string())
                .collect()
            },
            |s| s.split_whitespace().map(String::from).collect(),
        );
        Self {
            host: var("SLOT_HOST"),
            ssh,
            ws_root: var("SLOT_WSROOT").unwrap_or_else(|| paths::DEFAULT_REMOTE_WSROOT.into()),
            remote_slot: var("SLOT_REMOTE").unwrap_or_else(|| "slot".into()),
            extra_env: var("SLOT_ENV")
                .map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            socket: var("SLOT_SOCKET").map_or_else(paths::default_socket, PathBuf::from),
        }
    }
}

/// Runs `argv` through the daemon and returns the exit code to use.
pub fn run(argv: &[String], s: &Settings) -> i32 {
    if nested() {
        return run_local(argv);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (top, repo) = git_info(&cwd);
    let mut req = Request {
        argv: argv.to_vec(),
        session: std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default(),
        client: client_id(),
        repo: repo.unwrap_or_else(|| {
            cwd.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        }),
        ..Request::default()
    };
    if let Some(host) = s
        .host
        .as_ref()
        .filter(|_| !listed_local(top.as_deref(), argv))
    {
        return match remote(host, top.as_deref(), &cwd, &mut req, s) {
            Ok(c) => talk(c, &req).code(),
            Err(e) => {
                eprintln!("slot: daemon unreachable ({e:#}), ran locally");
                run_local(argv)
            }
        };
    }
    req.cwd = cwd.to_string_lossy().into_owned();
    req.env = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    submit_local(&s.socket, &req, UPGRADE_WAIT)
}

/// Whether the repository's `.slot.toml` lists `argv` under `local`, so it
/// runs on this machine's daemon although `SLOT_HOST` is set. Says so on
/// stderr; a `.slot.toml` that does not parse is reported and ignored.
fn listed_local(top: Option<&Path>, argv: &[String]) -> bool {
    let Some(top) = top else { return false };
    match RepoConfig::load(top) {
        Ok(c) => c.local_match(argv).is_some_and(|p| {
            eprintln!(
                "slot: {} lists `{p}` as local: running on this machine",
                repo_config::FILE
            );
            true
        }),
        Err(e) => {
            eprintln!("slot: {e}; ignored");
            false
        }
    }
}

/// `slot cancel <id>`: asks the daemon to cancel job `id`. With `SLOT_HOST`
/// set, runs `slot cancel` on that host over ssh.
pub fn cancel(id: i64, s: &Settings) -> i32 {
    if let Some(host) = &s.host {
        let Some((ssh, args)) = s.ssh.split_first() else {
            eprintln!("slot: SLOT_SSH is empty");
            return 2;
        };
        return match Command::new(ssh)
            .args(args)
            .arg(host)
            .arg(&s.remote_slot)
            .arg("cancel")
            .arg(id.to_string())
            .status()
        {
            Ok(st) => st.code().unwrap_or(1),
            Err(e) => {
                eprintln!("slot: {ssh}: {e}");
                3
            }
        };
    }
    let req = Request {
        cancel: Some(id),
        ..Request::default()
    };
    match local(&s.socket) {
        Ok(c) => talk(c, &req).code(),
        Err(e) => {
            eprintln!("slot: daemon unreachable ({e:#})");
            3
        }
    }
}

/// Runs `req` through the daemon at `socket`. An upgrading daemon drops the
/// connections of jobs it has not started and re-executes; such a job is
/// sent again, once, as soon as the socket is back, for up to `wait`.
/// A job whose process had started is not sent again, nor one whose client
/// got a signal: the signal cancelled it.
pub fn submit_local(socket: &Path, req: &Request, wait: Duration) -> i32 {
    let conn = match local(socket) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("slot: daemon unreachable ({e:#}), ran locally");
            return run_local(&req.argv);
        }
    };
    match talk(conn, req) {
        Ended::Lost { started: false } => {}
        ended => return ended.code(),
    }
    if let Some(sig) = signalled() {
        return 128 + sig;
    }
    eprintln!("slot: daemon upgrading, resubmitting job");
    let deadline = Instant::now() + wait;
    loop {
        if let Some(sig) = signalled() {
            return 128 + sig;
        }
        match local(socket) {
            Ok(c) => return talk(c, req).code(),
            Err(_) if Instant::now() < deadline => std::thread::sleep(UPGRADE_POLL),
            Err(e) => {
                eprintln!(
                    "slot: daemon not back after {}s ({e:#}), ran locally",
                    wait.as_secs()
                );
                return run_local(&req.argv);
            }
        }
    }
}

/// Whether this client runs inside a slot job. Such a call runs directly:
/// it inherits the outer job's jobserver, so it shares the pool without
/// queueing a second job that could wait on slots its parent holds.
pub fn nested() -> bool {
    std::env::var_os(crate::server::JOB_ENV).is_some_and(|v| !v.is_empty())
}

/// Runs `argv` here, without the daemon.
pub fn run_local(argv: &[String]) -> i32 {
    match Command::new(&argv[0]).args(&argv[1..]).status() {
        Ok(st) => st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0)),
        Err(e) => {
            eprintln!("slot: {}: {e}", argv[0]);
            127
        }
    }
}

fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default()
}

fn client_id() -> String {
    format!("{}:{}", hostname(), nix::unistd::getppid())
}

/// Repository root and name of the git checkout containing `cwd`. A linked
/// worktree reports its main repository's name.
pub fn git_info(cwd: &Path) -> (Option<PathBuf>, Option<String>) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "rev-parse",
            "--show-toplevel",
            "--path-format=absolute",
            "--git-common-dir",
        ])
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return (None, None) };
    if !out.status.success() {
        return (None, None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let (Some(top), Some(common)) = (lines.next(), lines.next()) else {
        return (None, None);
    };
    let common = Path::new(common.trim_end_matches('/'));
    let name = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().and_then(Path::file_name)
    } else {
        common.file_name()
    };
    let repo = name.map(|n| n.to_string_lossy().trim_end_matches(".git").to_string());
    (Some(PathBuf::from(top)), repo)
}

/// Workspace id for a repository root on this host.
pub fn workspace_id(top: &Path) -> String {
    let digest = Sha256::digest(format!("{}:{}", hostname(), top.display()).as_bytes());
    digest.iter().take(8).fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// A connection to the daemon, split into halves.
pub struct Conn {
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    closer: Box<dyn FnMut() + Send>,
}

fn local(socket: &Path) -> Result<Conn> {
    let c = UnixStream::connect(socket).with_context(|| format!("connect {}", socket.display()))?;
    c.set_write_timeout(Some(Duration::from_secs(2)))?;
    let reader = c.try_clone()?;
    let writer = c.try_clone()?;
    Ok(Conn {
        reader: Box::new(reader),
        writer: Box::new(writer),
        closer: Box::new(move || {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }),
    })
}

fn remote(
    host: &str,
    top: Option<&Path>,
    cwd: &Path,
    req: &mut Request,
    s: &Settings,
) -> Result<Conn> {
    let top = top.ok_or_else(|| anyhow!("remote mode needs a git repository"))?;
    let id = workspace_id(top);
    let rel = cwd
        .strip_prefix(top)
        .unwrap_or(Path::new(""))
        .to_string_lossy()
        .into_owned();
    let tc = toolchain::for_argv(&req.argv, 1);
    let mut rsync = Command::new("rsync");
    rsync.args([
        "-a",
        "--delete",
        "--mkpath",
        "--filter=:- .gitignore",
        "--exclude=/.git",
    ]);
    for x in tc.sync_excludes() {
        rsync.arg(format!("--exclude={x}"));
    }
    rsync
        .arg("-e")
        .arg(s.ssh.join(" "))
        .arg(format!("{}/", top.display()))
        .arg(format!("{host}:{}/{id}/", s.ws_root));
    let st = rsync.status().context("rsync")?;
    if !st.success() {
        bail!("rsync exited with {st}");
    }
    for (k, v) in std::env::vars() {
        if (tc.client_env(&k) && !toolchain::credential(&k)) || s.extra_env.contains(&k) {
            req.env.push(format!("{k}={v}"));
        }
    }
    req.workspace = Some(Workspace {
        id,
        rel_cwd: rel,
        client_root: top.to_string_lossy().into_owned(),
    });
    let mut child: Child = Command::new(&s.ssh[0])
        .args(&s.ssh[1..])
        .arg(host)
        .arg(&s.remote_slot)
        .arg("proxy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("ssh")?;
    let writer = child.stdin.take().ok_or_else(|| anyhow!("ssh stdin"))?;
    let reader = child.stdout.take().ok_or_else(|| anyhow!("ssh stdout"))?;
    Ok(Conn {
        reader: Box::new(reader),
        writer: Box::new(writer),
        closer: Box::new(move || {
            let _ = child.kill();
        }),
    })
}

/// How a conversation with the daemon ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The job's exit code, or 3 when the request could not be sent.
    Exit(i32),
    /// The connection closed without an exit frame. `started` is whether
    /// the job's process had started (a start, output or error frame came).
    Lost { started: bool },
    /// The client got this signal; the job was cancelled.
    Signalled(i32),
}

impl Ended {
    /// The exit code for the caller; a lost connection is reported here.
    fn code(self) -> i32 {
        match self {
            Self::Exit(code) => code,
            Self::Signalled(sig) => 128 + sig,
            Self::Lost { .. } => {
                eprintln!("slot: connection to daemon lost");
                3
            }
        }
    }
}

fn talk(conn: Conn, req: &Request) -> Ended {
    let Conn {
        reader,
        mut writer,
        closer,
    } = conn;
    let mut line = match serde_json::to_vec(req) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("slot: {e}");
            return Ended::Exit(3);
        }
    };
    line.push(b'\n');
    if let Err(e) = writer.write_all(&line).and_then(|()| writer.flush()) {
        eprintln!("slot: send: {e}");
        return Ended::Exit(3);
    }
    // The writer stays open until the job ends: closing it tells the daemon
    // the client is gone, and the daemon cancels the job.
    if let Ok(mut c) = CLOSER.lock() {
        *c = Some(closer);
    }
    handle_signals();
    if let Some(sig) = signalled() {
        return Ended::Signalled(sig);
    }
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let mut started = false;
    for l in BufReader::new(reader).lines() {
        let Ok(l) = l else { break };
        match serde_json::from_str::<Frame>(&l) {
            Ok(Frame::Out { b }) => {
                started = true;
                let _ = stdout.write_all(b.as_bytes());
                let _ = stdout.flush();
            }
            Ok(Frame::Err { b }) => {
                started = true;
                let _ = stderr.write_all(b.as_bytes());
            }
            Ok(Frame::Start { .. }) => started = true,
            Ok(Frame::Info { msg }) => eprintln!("{msg}"),
            Ok(Frame::Exit { code }) => {
                drop(writer);
                return Ended::Exit(code);
            }
            Err(_) => {}
        }
    }
    if let Some(sig) = signalled() {
        return Ended::Signalled(sig);
    }
    Ended::Lost { started }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    /// A fake daemon: for each script, accepts one connection, reads the
    /// request, reports it, writes the script's frames and closes. Between
    /// connections the socket is gone for `gap`, as during a re-exec.
    fn fake_daemon(
        socket: PathBuf,
        scripts: Vec<Vec<Frame>>,
        gap: Duration,
    ) -> mpsc::Receiver<Request> {
        let (tx, rx) = mpsc::channel();
        let l = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let mut l = Some(l);
            for (i, frames) in scripts.into_iter().enumerate() {
                if i > 0 {
                    std::thread::sleep(gap);
                    l = Some(UnixListener::bind(&socket).unwrap());
                }
                let (c, _) = l.as_ref().unwrap().accept().unwrap();
                let mut line = String::new();
                BufReader::new(&c).read_line(&mut line).unwrap();
                tx.send(serde_json::from_str(&line).unwrap()).unwrap();
                let mut w = &c;
                for f in frames {
                    let mut b = serde_json::to_vec(&f).unwrap();
                    b.push(b'\n');
                    w.write_all(&b).unwrap();
                }
                drop(l.take());
                let _ = std::fs::remove_file(&socket);
                drop(c);
            }
        });
        rx
    }

    fn request() -> Request {
        Request {
            argv: vec!["cargo".into(), "check".into()],
            cwd: "/w".into(),
            ..Request::default()
        }
    }

    #[test]
    fn a_job_dropped_while_queued_is_resubmitted_once() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let queued = Frame::Info {
            msg: "slot: job 1 queued".into(),
        };
        let rx = fake_daemon(
            socket.clone(),
            vec![
                vec![queued],
                vec![Frame::Start { id: 1 }, Frame::Exit { code: 7 }],
            ],
            Duration::from_millis(800),
        );
        let t0 = Instant::now();
        assert_eq!(
            submit_local(&socket, &request(), Duration::from_secs(10)),
            7
        );
        assert!(t0.elapsed() >= Duration::from_millis(800));
        assert_eq!(rx.recv().unwrap(), request());
        assert_eq!(rx.recv().unwrap(), request(), "the same request again");
    }

    #[test]
    fn a_started_job_is_not_resubmitted() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let rx = fake_daemon(
            socket.clone(),
            vec![vec![Frame::Start { id: 1 }], vec![Frame::Exit { code: 0 }]],
            Duration::from_millis(100),
        );
        assert_eq!(
            submit_local(&socket, &request(), Duration::from_secs(10)),
            3
        );
        assert_eq!(rx.recv().unwrap(), request());
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "no second submission"
        );
    }

    #[test]
    fn output_from_an_older_daemon_counts_as_started() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let rx = fake_daemon(
            socket.clone(),
            vec![vec![Frame::Out { b: String::new() }]],
            Duration::ZERO,
        );
        assert_eq!(
            submit_local(&socket, &request(), Duration::from_millis(600)),
            3
        );
        assert_eq!(rx.recv().unwrap(), request());
    }
}
