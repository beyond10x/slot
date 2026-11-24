//! The gate: builds in a sandbox join the pool through FIFOs.
//!
//! A sandbox that refuses `socket(AF_UNIX)` and has no network cannot reach
//! the daemon, so its builds ran outside the pool. It can still open FIFOs
//! in a directory it shares with the host. `slot gate --dir <dir>` serves
//! such a directory on the host (`docs/design.md` § 19):
//!
//! - `<dir>/jobserver.fifo` is a hard link to the pool's FIFO, so cargo,
//!   rustc and make in the sandbox draw tokens from the same pool;
//! - `<dir>/requests.fifo` takes one line per job: the id of a request
//!   file `<dir>/<id>.json` the client wrote;
//! - for each request the gate connects to the daemon like any client,
//!   with `argv` = `slot hold <dir>/<id>.hold` and `hold_for` = the
//!   client's command, and copies the daemon's frames to `<dir>/<id>.reply`.
//!
//! The daemon queues the job, takes its slots and starts the hold process,
//! which writes the job's environment to `<dir>/<id>.env` and waits on the
//! hold FIFO. The client then runs its command in the sandbox with that
//! environment, the jobserver pointed at the linked FIFO. When the command
//! exits the client writes `exit <code>` to the hold FIFO and closes it;
//! the hold process exits with that code and the daemon frees the slots. If
//! the client dies, the hold FIFO loses its last writer and the hold
//! process exits too. Nothing of the client's command runs on the host.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::proto::{Frame, Request};

/// The client's variable naming the gate directory.
pub const GATE_ENV: &str = "SLOT_GATE";
/// Request ids, one per line.
pub const REQUESTS: &str = "requests.fifo";
/// Hard link to the pool's jobserver FIFO.
pub const JOBSERVER: &str = "jobserver.fifo";
/// How long a client waits for the gate to take its request, and for the
/// hold process to write the job's environment.
const TAKE_WAIT: Duration = Duration::from_secs(10);
/// How often the gate checks the pool FIFO link and whether clients left.
const TICK: Duration = Duration::from_secs(1);
/// Variables that carry `--jobserver-auth`.
const AUTH_VARS: &[&str] = &["MAKEFLAGS", "MFLAGS", "CARGO_MAKEFLAGS"];

/// The gate directory from `SLOT_GATE`, if set.
pub fn dir_from_env() -> Option<PathBuf> {
    std::env::var_os(GATE_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Whether `id` is a request id the gate accepts: a file name of letters,
/// digits, `-`, `_` and `.`, not starting with `.`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn mkfifo(path: &Path) -> Result<()> {
    nix::unistd::mkfifo(path, nix::sys::stat::Mode::from_bits_truncate(0o600))
        .with_context(|| format!("mkfifo {}", path.display()))
}

fn is_fifo(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_fifo())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// Points `<dir>/jobserver.fifo` at the pool FIFO `pool`. Returns whether
/// it changed the link.
pub fn link_pool(dir: &Path, pool: &Path) -> Result<bool> {
    let link = dir.join(JOBSERVER);
    if same_file(&link, pool) {
        return Ok(false);
    }
    if !is_fifo(pool) {
        bail!("{} is not the pool's FIFO", pool.display());
    }
    let tmp = dir.join(format!(".{JOBSERVER}.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::fs::hard_link(pool, &tmp).with_context(|| {
        format!(
            "link {} to {} (they must be on one filesystem)",
            tmp.display(),
            pool.display()
        )
    })?;
    std::fs::rename(&tmp, &link).with_context(|| format!("rename to {}", link.display()))?;
    Ok(true)
}

/// Makes `dir`, links the pool FIFO into it and opens its request FIFO for
/// reading and writing, so it never reads end-of-file.
pub fn prepare(dir: &Path, pool: &Path) -> Result<File> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    link_pool(dir, pool)?;
    let req = dir.join(REQUESTS);
    if !is_fifo(&req) {
        let _ = std::fs::remove_file(&req);
        mkfifo(&req)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&req)
        .with_context(|| format!("open {}", req.display()))
}

/// `slot gate`: serves every directory in `dirs` until killed. `exe` is
/// the `slot` binary the daemon starts as the hold process.
pub fn serve(dirs: &[PathBuf], socket: &Path, pool: &Path, exe: &Path) -> Result<()> {
    if dirs.is_empty() {
        bail!("no --dir to serve");
    }
    for dir in dirs {
        let file = prepare(dir, pool)?;
        eprintln!(
            "slot gate: serving {} for the pool at {}",
            dir.display(),
            pool.display()
        );
        let (dir, socket, exe) = (dir.clone(), socket.to_path_buf(), exe.to_path_buf());
        std::thread::spawn(move || listen(&dir, file, &socket, &exe));
    }
    // The daemon recreates the FIFO only at a full restart; follow it.
    loop {
        std::thread::sleep(TICK * 5);
        for dir in dirs {
            match link_pool(dir, pool) {
                Ok(true) => eprintln!("slot gate: {} relinked to the pool", dir.display()),
                Ok(false) => {}
                Err(e) => eprintln!("slot gate: {}: {e:#}", dir.display()),
            }
        }
    }
}

fn listen(dir: &Path, file: File, socket: &Path, exe: &Path) {
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let id = line.trim().to_string();
        if !valid_id(&id) {
            eprintln!("slot gate: {}: ignored request {id:?}", dir.display());
            continue;
        }
        let (dir, socket, exe) = (dir.to_path_buf(), socket.to_path_buf(), exe.to_path_buf());
        std::thread::spawn(move || {
            if let Err(e) = relay(&dir, &id, &socket, &exe) {
                eprintln!("slot gate: {}: {id}: {e:#}", dir.display());
            }
        });
    }
}

/// The request the gate sends the daemon for a client's request.
pub fn daemon_request(dir: &Path, id: &str, client: Request, exe: &Path) -> Request {
    let hold = dir.join(format!("{id}.hold"));
    let cwd = if Path::new(&client.cwd).is_absolute() && Path::new(&client.cwd).is_dir() {
        client.cwd
    } else {
        dir.to_string_lossy().into_owned()
    };
    Request {
        argv: vec![
            exe.to_string_lossy().into_owned(),
            "hold".into(),
            hold.to_string_lossy().into_owned(),
        ],
        hold_for: Some(client.argv),
        cwd,
        env: client.env,
        workspace: None,
        session: client.session,
        repo: client.repo,
        checkout: client.checkout,
        client: client.client,
        cancel: None,
        tokens: client.tokens,
        priority: client.priority,
        timeout_s: client.timeout_s,
        // The command runs in the sandbox with the sandbox's limits; the hold
        // process the daemon starts needs none.
        nofile: None,
        fetch_once: false,
        local_reason: client.local_reason,
    }
}

/// Whether the reader of a FIFO we write to has gone.
fn reader_gone(f: &File) -> bool {
    let mut fds = [PollFd::new(f.as_fd(), PollFlags::empty())];
    matches!(poll(&mut fds, PollTimeout::ZERO), Ok(n) if n > 0)
        && fds[0]
            .revents()
            .is_some_and(|r| r.intersects(PollFlags::POLLERR | PollFlags::POLLHUP))
}

fn relay(dir: &Path, id: &str, socket: &Path, exe: &Path) -> Result<()> {
    let json = dir.join(format!("{id}.json"));
    let body = std::fs::read(&json).with_context(|| format!("read {}", json.display()))?;
    let _ = std::fs::remove_file(&json);
    let client: Request = serde_json::from_slice(&body).context("parse request")?;
    let reply_path = dir.join(format!("{id}.reply"));
    let mut reply = match OpenOptions::new()
        .write(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(&reply_path)
    {
        Ok(f) => f,
        // ENXIO: no reader, the client is gone.
        Err(e) if e.raw_os_error() == Some(nix::libc::ENXIO) => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("open {}", reply_path.display())),
    };
    set_blocking(&reply)?;
    if client.argv.is_empty() {
        send(
            &mut reply,
            &Frame::Info {
                msg: "slot: empty command".into(),
            },
        )?;
        return send(&mut reply, &Frame::Exit { code: 2 });
    }
    let req = daemon_request(dir, id, client, exe);
    let conn = match UnixStream::connect(socket) {
        Ok(c) => c,
        Err(e) => {
            // Closing without a start frame: the client runs the command
            // in its sandbox, outside the pool, and says so.
            return send(
                &mut reply,
                &Frame::Info {
                    msg: format!("slot: daemon unreachable from the gate ({e})"),
                },
            );
        }
    };
    let mut line = serde_json::to_vec(&req)?;
    line.push(b'\n');
    (&conn).write_all(&line)?;
    conn.set_read_timeout(Some(TICK))?;
    let mut rd = BufReader::new(conn.try_clone()?);
    let mut buf = String::new();
    loop {
        match rd.read_line(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                if !buf.ends_with('\n') {
                    continue;
                }
                let exit = matches!(serde_json::from_str::<Frame>(&buf), Ok(Frame::Exit { .. }));
                if reply.write_all(buf.as_bytes()).is_err() {
                    // The client left: closing the connection cancels the job.
                    let _ = conn.shutdown(std::net::Shutdown::Both);
                    return Ok(());
                }
                buf.clear();
                if exit {
                    return Ok(());
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if reader_gone(&reply) {
                    let _ = conn.shutdown(std::net::Shutdown::Both);
                    for ext in ["reply", "hold", "env"] {
                        let _ = std::fs::remove_file(dir.join(format!("{id}.{ext}")));
                    }
                    return Ok(());
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn set_blocking(f: &File) -> Result<()> {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    let flags = OFlag::from_bits_truncate(fcntl(f, FcntlArg::F_GETFL)?);
    fcntl(f, FcntlArg::F_SETFL(flags - OFlag::O_NONBLOCK))?;
    Ok(())
}

fn send(w: &mut File, f: &Frame) -> Result<()> {
    let mut b = serde_json::to_vec(f)?;
    b.push(b'\n');
    w.write_all(&b)?;
    Ok(())
}

/// `slot hold <fifo>`: the process the daemon runs for a gated job. Writes
/// its environment (the job's: jobserver, `SLOT_JOB`, toolchain settings)
/// to `<id>.env` beside the FIFO, then waits for the client to close the
/// FIFO and exits with the code it wrote there (130 when it wrote none).
pub fn hold(fifo: &Path) -> i32 {
    let Some(stem) = fifo
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".hold"))
        .filter(|s| valid_id(s))
    else {
        eprintln!("slot hold: {} is not a gate hold FIFO", fifo.display());
        return 2;
    };
    if !is_fifo(fifo) {
        eprintln!("slot hold: {} is not a FIFO", fifo.display());
        return 2;
    }
    let dir = fifo.parent().unwrap_or_else(|| Path::new("."));
    // Opened before the environment is published: a client that already
    // left leaves no writer, and the read below ends at once.
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(fifo)
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("slot hold: open {}: {e}", fifo.display());
            return 2;
        }
    };
    let env: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    let out = dir.join(format!("{stem}.env"));
    let tmp = dir.join(format!(".{stem}.env"));
    let wrote = serde_json::to_vec(&env)
        .map_err(std::io::Error::other)
        .and_then(|b| {
            let mut f = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&b)
        })
        .and_then(|()| std::fs::rename(&tmp, &out));
    if let Err(e) = wrote {
        eprintln!("slot hold: write {}: {e}", out.display());
        return 2;
    }
    if set_blocking(&file).is_err() {
        return 2;
    }
    let mut text = String::new();
    let _ = BufReader::new(file).read_to_string(&mut text);
    exit_code(&text).unwrap_or_else(|| {
        eprintln!("slot hold: the client left without an exit code");
        130
    })
}

/// The code of the last `exit <code>` line in `text`.
pub fn exit_code(text: &str) -> Option<i32> {
    text.lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("exit ")?.trim().parse().ok())
}

/// `env` with every `--jobserver-auth` pointed at `fifo`.
pub fn rewrite_auth(env: Vec<String>, fifo: &Path) -> Vec<String> {
    let auth = format!("--jobserver-auth=fifo:{}", fifo.display());
    env.into_iter()
        .map(|e| {
            let Some((k, v)) = e.split_once('=') else {
                return e;
            };
            if !AUTH_VARS.contains(&k) {
                return e;
            }
            let v: Vec<String> = v
                .split(' ')
                .map(|w| {
                    if w.starts_with("--jobserver-auth=") || w.starts_with("--jobserver-fds=") {
                        auth.clone()
                    } else {
                        w.to_string()
                    }
                })
                .collect();
            format!("{k}={}", v.join(" "))
        })
        .collect()
}

/// Files of one client request; removed when the client ends.
struct Files {
    paths: Vec<PathBuf>,
}

impl Drop for Files {
    fn drop(&mut self) {
        for p in &self.paths {
            let _ = std::fs::remove_file(p);
        }
    }
}

enum Event {
    Frame(Frame),
    Closed,
}

/// Runs `req` (argv, cwd, env filled in) through the gate at `dir`: queued
/// in the pool, then run here. Falls back to running here outside the pool,
/// with one stderr line, when the gate does not serve `dir`.
pub fn submit(dir: &Path, req: &Request) -> i32 {
    match submit_inner(dir, req) {
        Ok(code) => code,
        Err(Fallback(why)) => {
            eprintln!("slot: {why}; ran outside the pool");
            crate::client::run_local(&req.argv)
        }
    }
}

struct Fallback(String);

impl<E: std::fmt::Display> From<E> for Fallback {
    fn from(e: E) -> Self {
        Self(e.to_string())
    }
}

fn request_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("c{}-{nanos}", std::process::id())
}

#[allow(clippy::too_many_lines)]
fn submit_inner(dir: &Path, req: &Request) -> std::result::Result<i32, Fallback> {
    let id = request_id();
    let reply_path = dir.join(format!("{id}.reply"));
    let hold_path = dir.join(format!("{id}.hold"));
    let json = dir.join(format!("{id}.json"));
    let env_path = dir.join(format!("{id}.env"));
    let _files = Files {
        paths: vec![
            reply_path.clone(),
            hold_path.clone(),
            json.clone(),
            env_path.clone(),
        ],
    };
    mkfifo(&reply_path).map_err(|e| Fallback(format!("gate {}: {e:#}", dir.display())))?;
    mkfifo(&hold_path).map_err(|e| Fallback(format!("gate {}: {e:#}", dir.display())))?;
    // Read-write: this process is the hold FIFO's writer from now on, so
    // the hold process sees end-of-file exactly when this process ends.
    let mut hold = OpenOptions::new().read(true).write(true).open(&hold_path)?;
    let reply = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(&reply_path)?;
    let body = serde_json::to_vec(req)?;
    let tmp = dir.join(format!(".{id}.json"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &json)?;
    let mut requests = match OpenOptions::new()
        .write(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(dir.join(REQUESTS))
    {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(nix::libc::ENXIO) => {
            return Err(Fallback(format!("gate {} not served", dir.display())));
        }
        Err(e) => return Err(Fallback(format!("gate {}: {e}", dir.display()))),
    };
    requests.write_all(format!("{id}\n").as_bytes())?;
    drop(requests);

    // Frames arrive on a thread; the main thread waits for them and for
    // the command.
    let (tx, rx) = mpsc::channel::<Event>();
    std::thread::spawn(move || {
        let mut r = reply;
        let mut pending = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let hup = {
                let mut fds = [PollFd::new(r.as_fd(), PollFlags::POLLIN)];
                if poll(&mut fds, PollTimeout::NONE).is_err() {
                    continue;
                }
                fds[0]
                    .revents()
                    .is_some_and(|f| f.contains(PollFlags::POLLHUP))
            };
            match r.read(&mut buf) {
                Ok(0) => {
                    // No writer yet reads as end-of-file too; poll above
                    // reports a hang-up only after a first writer came.
                    if hup {
                        let _ = tx.send(Event::Closed);
                        return;
                    }
                }
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    while let Some(i) = pending.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = pending.drain(..=i).collect();
                        if let Ok(f) = serde_json::from_slice::<Frame>(&line)
                            && tx.send(Event::Frame(f)).is_err()
                        {
                            return;
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => {
                    let _ = tx.send(Event::Closed);
                    return;
                }
            }
        }
    });

    let sent = Instant::now();
    let job = loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Frame(Frame::Start { id, .. })) => break id,
            Ok(Event::Frame(Frame::Info { msg })) => eprintln!("{msg}"),
            Ok(Event::Frame(Frame::Exit { code })) => return Ok(code),
            Ok(Event::Frame(Frame::Out { b })) => print!("{b}"),
            Ok(Event::Frame(Frame::Err { b })) => eprint!("{b}"),
            Ok(Event::Frame(Frame::Fallback { msg })) => return Err(Fallback(msg)),
            Ok(Event::Closed) => {
                return Err(Fallback(format!(
                    "gate {} closed before the job started",
                    dir.display()
                )));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if json.exists() && sent.elapsed() > TAKE_WAIT {
                    return Err(Fallback(format!(
                        "gate {} did not take the request in {}s",
                        dir.display(),
                        TAKE_WAIT.as_secs()
                    )));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Fallback("gate reply lost".into()));
            }
        }
    };

    // The hold process writes the job's environment as it starts.
    let deadline = Instant::now() + TAKE_WAIT;
    let env: Vec<String> = loop {
        if let Ok(b) = std::fs::read(&env_path)
            && let Ok(v) = serde_json::from_slice(&b)
        {
            break v;
        }
        if Instant::now() > deadline {
            let _ = hold.write_all(b"exit 125\n");
            eprintln!("slot: job {job}: the hold process wrote no environment; not run");
            return Ok(125);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let env = rewrite_auth(env, &dir.join(JOBSERVER));
    let mut cmd = Command::new(&req.argv[0]);
    cmd.args(&req.argv[1..])
        .env_clear()
        .envs(env.iter().filter_map(|e| e.split_once('=')));
    if !req.cwd.is_empty() {
        cmd.current_dir(&req.cwd);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("slot: {}: {e}", req.argv[0]);
            let _ = hold.write_all(b"exit 127\n");
            return Ok(127);
        }
    };
    let mut cancelled_at: Option<Instant> = None;
    // What this client exits with when the daemon ends the job: 130 for a
    // cancel, 124 for a job ended at its run-time limit.
    let mut stopped_code = 130;
    let mut gate_gone = false;
    let code = loop {
        if let Ok(Some(st)) = child.try_wait() {
            break st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0));
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Frame(Frame::Exit { code })) if cancelled_at.is_none() => {
                if code == crate::run_limit::EXIT {
                    // The daemon's `info` frame said why.
                    stopped_code = code;
                } else {
                    eprintln!("slot: job {job} cancelled");
                }
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(child.id() as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
                cancelled_at = Some(Instant::now());
            }
            Ok(Event::Frame(Frame::Info { msg })) => eprintln!("{msg}"),
            Ok(Event::Closed) if !gate_gone => {
                gate_gone = true;
                eprintln!(
                    "slot: job {job}: the gate went away; the build goes on outside the pool"
                );
            }
            _ => {}
        }
        if cancelled_at.is_some_and(|t| t.elapsed() > Duration::from_secs(5)) {
            let _ = child.kill();
        }
    };
    if cancelled_at.is_some() {
        return Ok(stopped_code);
    }
    let _ = hold.write_all(format!("exit {code}\n").as_bytes());
    drop(hold);
    // Wait for the daemon's exit frame so the job is booked before we go.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !gate_gone && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Frame(Frame::Exit { .. }) | Event::Closed)
            | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Event::Frame(Frame::Info { msg })) => eprintln!("{msg}"),
            _ => {}
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_plain_file_names() {
        assert!(valid_id("c12-1700000000"));
        assert!(!valid_id(""));
        assert!(!valid_id("../x"));
        assert!(!valid_id(".hidden"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(&"x".repeat(129)));
    }

    #[test]
    fn the_jobserver_is_pointed_at_the_gate_link() {
        let env = vec![
            "CARGO_MAKEFLAGS=-j --jobserver-auth=fifo:/host/state/jobserver.fifo".to_string(),
            "MAKEFLAGS=-j --jobserver-auth=fifo:/host/state/jobserver.fifo".to_string(),
            "OTHER=--jobserver-auth=fifo:/x".to_string(),
            "SLOT_JOB=7".to_string(),
        ];
        let out = rewrite_auth(env, Path::new("/box/gate/jobserver.fifo"));
        assert_eq!(
            out,
            vec![
                "CARGO_MAKEFLAGS=-j --jobserver-auth=fifo:/box/gate/jobserver.fifo",
                "MAKEFLAGS=-j --jobserver-auth=fifo:/box/gate/jobserver.fifo",
                "OTHER=--jobserver-auth=fifo:/x",
                "SLOT_JOB=7",
            ]
        );
    }

    #[test]
    fn the_last_exit_line_wins() {
        assert_eq!(exit_code("exit 3\nexit 0\n"), Some(0));
        assert_eq!(exit_code("exit 101\n"), Some(101));
        assert_eq!(exit_code(""), None);
        assert_eq!(exit_code("garbage\n"), None);
    }

    #[test]
    fn the_daemon_starts_only_the_hold_process() {
        let client = Request {
            argv: vec!["cargo".into(), "build".into()],
            cwd: "/does/not/exist/on/the/host".into(),
            env: vec!["A=1".into()],
            tokens: Some(3),
            timeout_s: Some(1800),
            nofile: Some(crate::proto::FileLimit {
                soft: 4096,
                hard: 8192,
            }),
            checkout: Some(crate::types::Checkout::Worktree {
                name: "feature".into(),
            }),
            ..Request::default()
        };
        let r = daemon_request(Path::new("/g"), "c1-2", client, Path::new("/bin/slot"));
        assert_eq!(r.argv, vec!["/bin/slot", "hold", "/g/c1-2.hold"]);
        assert_eq!(r.hold_for, Some(vec!["cargo".into(), "build".into()]));
        assert_eq!(r.cwd, "/g");
        assert_eq!(r.tokens, Some(3));
        assert_eq!(r.timeout_s, Some(1800));
        assert_eq!(
            r.checkout,
            Some(crate::types::Checkout::Worktree {
                name: "feature".into()
            })
        );
        assert_eq!(r.env, vec!["A=1".to_string()]);
        assert_eq!(
            r.nofile, None,
            "the hold process runs with the daemon's limit"
        );
    }

    #[test]
    fn the_pool_fifo_is_linked_and_relinked() {
        let t = tempfile::tempdir().unwrap();
        let pool = t.path().join("pool.fifo");
        mkfifo(&pool).unwrap();
        let dir = t.path().join("gate");
        let _f = prepare(&dir, &pool).unwrap();
        assert!(same_file(&dir.join(JOBSERVER), &pool));
        assert!(is_fifo(&dir.join(REQUESTS)));
        assert!(!link_pool(&dir, &pool).unwrap());
        std::fs::remove_file(&pool).unwrap();
        mkfifo(&pool).unwrap();
        assert!(link_pool(&dir, &pool).unwrap());
        assert!(same_file(&dir.join(JOBSERVER), &pool));
    }
}
