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
use std::time::Duration;

use std::fmt::Write as _;

use anyhow::{Context as _, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

use crate::paths;
use crate::proto::{Frame, Request, Workspace};
use crate::toolchain;

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
    let conn = if let Some(host) = &s.host {
        remote(host, top.as_deref(), &cwd, &mut req, s)
    } else {
        req.cwd = cwd.to_string_lossy().into_owned();
        req.env = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        local(&s.socket)
    };
    match conn {
        Ok(c) => talk(c, &req),
        Err(e) => {
            eprintln!("slot: daemon unreachable ({e:#}), ran locally");
            run_local(argv)
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
        if tc.client_env(&k) || s.extra_env.contains(&k) {
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

fn talk(conn: Conn, req: &Request) -> i32 {
    let Conn {
        reader,
        mut writer,
        mut closer,
    } = conn;
    let mut line = match serde_json::to_vec(req) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("slot: {e}");
            return 3;
        }
    };
    line.push(b'\n');
    if let Err(e) = writer.write_all(&line).and_then(|()| writer.flush()) {
        eprintln!("slot: send: {e}");
        return 3;
    }
    // The writer stays open until the job ends: closing it tells the daemon
    // the client is gone, and the daemon cancels the job.
    if let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP]) {
        std::thread::spawn(move || {
            if let Some(sig) = signals.forever().next() {
                closer();
                std::process::exit(128 + sig);
            }
        });
    }
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    for l in BufReader::new(reader).lines() {
        let Ok(l) = l else { break };
        match serde_json::from_str::<Frame>(&l) {
            Ok(Frame::Out { b }) => {
                let _ = stdout.write_all(b.as_bytes());
                let _ = stdout.flush();
            }
            Ok(Frame::Err { b }) => {
                let _ = stderr.write_all(b.as_bytes());
            }
            Ok(Frame::Info { msg }) => eprintln!("{msg}"),
            Ok(Frame::Exit { code }) => {
                drop(writer);
                return code;
            }
            Err(_) => {}
        }
    }
    eprintln!("slot: connection to daemon lost");
    3
}
