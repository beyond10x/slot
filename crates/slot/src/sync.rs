//! The sync of remote mode: the repository to its mirror on the build
//! server, by rsync over ssh.
//!
//! Before rsync runs, the client asks the server for sibling mirrors of the
//! same repository (`mirror.rs`); a server that answers gets them as
//! `--copy-dest`, and `--stats` for the count of bytes sent. A server whose
//! slot does not know the step gets the plain sync of older clients.
//!
//! rsync compares files by content (`--checksum`), never by size and time
//! alone, and the sync line names the commit and the count of changed files
//! it sent, so a job is not run on content the caller did not have.
//!
//! The mirror keeps the server's modification times (`--no-times`): a file
//! whose content changed gets the time rsync wrote it, one whose content did
//! not keeps the time it has. cargo decides by modification time. With the
//! caller's times, a change made before a build the server ran on the old
//! content, and synced after it, was older than that build: cargo took the
//! old build as fresh. A `touch` on the server did not help, since the next
//! sync set the caller's time again.
//!
//! Symlinks stay links (`-a`): a link whose target lies outside the tree is
//! sent as the link, never as the file it points to, and the sync line counts
//! such links. Before any byte is sent, the tree is listed with the same
//! rsync (`preflight`); when that rsync would send such a target in place of
//! the link (a wrapper on `PATH` that adds `--copy-unsafe-links`), the sync is
//! refused and the job runs on this machine.
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

use crate::{mirror, same_path};

/// Default stall window: a sync that moves no byte for this long is killed.
pub const DEFAULT_STALL: Duration = Duration::from_secs(60);

/// Default bound on a whole sync.
pub const DEFAULT_LIMIT: Duration = Duration::from_secs(300);

/// rsync options after `-a` of a sync of sources: compare by content, and
/// leave the mirror its own modification times. Without `--checksum` every
/// file would differ in time and be sent on every sync.
pub const BY_CONTENT: [&str; 2] = ["--checksum", "--no-times"];

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
    /// Mirror the checkout at its own absolute path on the server
    /// (`SLOT_REMOTE_SAME_PATH`), not under the workspace root.
    pub same_path: bool,
}

impl Remote<'_> {
    /// Where the sync of the checkout at `top`, mirror `id`, goes.
    pub fn dest(&self, top: &Path, id: &str) -> String {
        if self.same_path {
            top.display().to_string()
        } else {
            format!("{}/{id}", self.ws_root)
        }
    }

    /// How rsync names sibling mirror `id` for `--copy-dest`: relative to
    /// the destination, `../<id>`; for a same-path mirror, whose parent is
    /// not the workspace root, `<ws-root>/<id>` made absolute with this
    /// machine's home, which the server's account shares.
    pub fn copy_dest(&self, id: &str) -> Option<String> {
        if !self.same_path {
            return Some(format!("../{id}"));
        }
        let ws = Path::new(self.ws_root);
        let ws = if ws.is_absolute() {
            ws.to_path_buf()
        } else {
            Path::new(&std::env::var_os("HOME")?).join(ws)
        };
        Some(format!("{}/{id}", ws.display()))
    }
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
    /// How long it waited for another sync of the same worktree.
    pub waited: Duration,
    /// What the checkout held when it was synced.
    pub sources: Sources,
    /// Symlinks synced whose target lies outside the tree: they go as
    /// links, never as the file they point to.
    pub outside_links: usize,
}

/// What the checkout held when it was synced, as git sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sources {
    /// `HEAD`'s short commit (`None` before the first commit) and the number
    /// of lines of `git status --porcelain`: changed tracked files and
    /// untracked files git does not ignore.
    Git {
        /// Short commit of `HEAD`.
        head: Option<String>,
        /// Changed files.
        changed: usize,
    },
    /// git could not be asked; why.
    Unknown(String),
}

impl Sources {
    /// Asks git about the checkout at `top`, without taking its index lock.
    pub fn of(top: &Path) -> Self {
        let git = |args: &[&str]| {
            Command::new("git")
                .env("LC_ALL", "C")
                .arg("--no-optional-locks")
                .arg("-C")
                .arg(top)
                .args(args)
                .stdin(Stdio::null())
                .output()
        };
        let status = match git(&["status", "--porcelain"]) {
            Ok(o) if o.status.success() => o.stdout,
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let why = err.lines().next().unwrap_or("").trim();
                return Self::Unknown(format!("git status exited with {}: {why}", o.status));
            }
            Err(e) => return Self::Unknown(format!("git: {e}")),
        };
        let head = git(&["rev-parse", "--short", "HEAD"])
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        Self::Git {
            head,
            changed: status
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .count(),
        }
    }

    fn words(&self) -> String {
        match self {
            Self::Git { head, changed } => format!(
                "at {} + {changed} changed file{}",
                head.as_deref().unwrap_or("no commit"),
                if *changed == 1 { "" } else { "s" }
            ),
            Self::Unknown(why) => format!("sources not named, git unavailable ({why})"),
        }
    }
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
        let waited = if self.waited >= Duration::from_millis(100) {
            format!(
                ", after {:.1} s waiting for another sync of this worktree",
                self.waited.as_secs_f64()
            )
        } else {
            String::new()
        };
        let links = match self.outside_links {
            0 => String::new(),
            1 => ", 1 symlink leaves the tree (sent as a link)".to_string(),
            n => format!(", {n} symlinks leave the tree (sent as links)"),
        };
        format!(
            "slot: synced to {host} in {:.1} s, {bytes}{sib}{waited}{links}, {}",
            self.elapsed.as_secs_f64(),
            self.sources.words()
        )
    }
}

/// How often a sync that waits for another one of its worktree tries again.
const LOCK_POLL: Duration = Duration::from_millis(200);

/// The right to sync one worktree to one server; dropping it lets the next
/// client of that worktree sync.
#[derive(Debug)]
pub struct SyncLock {
    _held: nix::fcntl::Flock<std::fs::File>,
}

/// Waits until no other client on this machine syncs mirror `id` to `host`,
/// for at most `wait`, and holds that until the returned lock is dropped.
/// Two rsyncs into one mirror at once ended one of them with exit 23
/// (partial transfer). The lock is a `flock` on a file in `dir`.
pub fn lock_workspace(dir: &Path, host: &str, id: &str, wait: Duration) -> Result<SyncLock> {
    use nix::errno::Errno;
    use nix::fcntl::{Flock, FlockArg};
    std::fs::create_dir_all(dir).map_err(|e| anyhow!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("slot-sync-{id}-{}.lock", mirror::digest16(host)));
    let deadline = Instant::now() + wait;
    loop {
        let f = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| anyhow!("{}: {e}", path.display()))?;
        match Flock::lock(f, FlockArg::LockExclusiveNonblock) {
            Ok(held) => return Ok(SyncLock { _held: held }),
            Err((_, Errno::EWOULDBLOCK)) if Instant::now() < deadline => {
                std::thread::sleep(
                    LOCK_POLL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err((_, Errno::EWOULDBLOCK)) => {
                bail!(
                    "another sync of this worktree ran longer than {} s",
                    wait.as_secs()
                )
            }
            Err((_, e)) => bail!("lock {}: {e}", path.display()),
        }
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
fn ask_siblings(
    r: &Remote<'_>,
    top: &Path,
    id: &str,
    key: &str,
    wait: Duration,
) -> Option<Vec<String>> {
    let (ssh, opts) = r.ssh.split_first()?;
    let mut child = Command::new(ssh)
        .args(opts)
        .arg(r.host)
        .arg(r.remote_slot)
        .args(["__siblings", r.ws_root, id, key])
        .args(r.same_path.then_some(top.as_os_str()))
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

/// Runs `rsync` with its stdout read, killed at a stall or at `deadline`;
/// its stdout.
fn run_watched(rsync: &mut Command, b: &Bounds, deadline: Instant) -> Result<String> {
    rsync
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
    Ok(out)
}

/// Syncs `e` to its own path on the server, for a same-path job, within
/// `b`. A git directory is shared by every worktree of its repository, and
/// a sibling tree a `.slot.toml` declares by every job that declares it:
/// one sync of a path runs at a time on this machine.
pub fn sync_extra(e: &same_path::Extra, r: &Remote<'_>, b: &Bounds) -> Result<()> {
    let kind = match e.kind {
        same_path::Kind::Git => "git",
        same_path::Kind::Tree => "tree",
    };
    let _lock = crate::paths::runtime_dir()
        .map(|dir| {
            let id = format!("{kind}{}", mirror::digest16(&e.path.to_string_lossy()));
            lock_workspace(&dir, r.host, &id, b.limit)
        })
        .transpose()?;
    let deadline = Instant::now() + b.limit;
    let filters: Vec<String> = same_path::filters(e.kind)
        .iter()
        .map(|f| (*f).to_string())
        .collect();
    preflight(&e.path, &filters, b, deadline)?;
    let path = e.path.display();
    let mut rsync = Command::new("rsync");
    rsync.arg("-a");
    // A tree holds sources cargo may read; a git directory does not.
    if e.kind == same_path::Kind::Tree {
        rsync.args(BY_CONTENT);
    }
    rsync
        .args(["--delete", "--mkpath"])
        .args(&filters)
        .arg("-e")
        .arg(r.ssh.join(" "))
        .arg(format!("{path}/"))
        .arg(format!("{}:{path}/", r.host));
    run_watched(&mut rsync, b, deadline).map(drop)
}

/// The rsync filter of a git database copy: its objects, refs and `HEAD`,
/// nothing else (no `config` with a login or a header, no hook, no
/// `info/`, no `FETCH_HEAD`, no lock file of a running git, no
/// `objects/info/alternates` or `http-alternates`, which name a path or
/// URL of this machine).
pub const DB_ENTRY_FILTER: [&str; 8] = [
    "--exclude=*.lock",
    "--exclude=/objects/info/alternates",
    "--exclude=/objects/info/http-alternates",
    "--include=/HEAD",
    "--include=/packed-refs",
    "--include=/objects/***",
    "--include=/refs/***",
    "--exclude=*",
];

/// Copies the git database `src` to entry `entry` of the server account's
/// cargo git database (`git_deps::NODE_DB`, relative to its home), within
/// `b`. Only [`DB_ENTRY_FILTER`] crosses; `-rt` without `-l`: a symbolic
/// link is not sent. Adds to what the entry holds (no `--delete`).
pub fn copy_db_entry(src: &Path, entry: &str, r: &Remote<'_>, b: &Bounds) -> Result<()> {
    let deadline = Instant::now() + b.limit;
    let mut rsync = Command::new("rsync");
    rsync
        .args(["-rt", "--mkpath"])
        .args(DB_ENTRY_FILTER)
        .arg("-e")
        .arg(r.ssh.join(" "))
        .arg(format!("{}/", src.display()))
        .arg(format!("{}:{}/{entry}/", r.host, crate::git_deps::NODE_DB));
    run_watched(&mut rsync, b, deadline).map(drop)
}

/// The fields of one line of `rsync --list-only`: the type letter of its
/// mode (`d`, `-`, `l`, …) and what follows the time: the path relative to
/// the listed directory, and for a link ` -> ` and its target.
fn list_entry(line: &str) -> Option<(char, &str)> {
    let kind = line.chars().next()?;
    let mut rest = line;
    for _ in 0..4 {
        rest = rest.trim_start_matches(' ');
        rest = &rest[rest.find(' ')?..];
    }
    Some((kind, rest.strip_prefix(' ')?))
}

/// The path and target of a link entry `rest` of the listing of `root`:
/// split at the ` -> ` after which the path is a symlink on disk, else at
/// the first.
fn split_link<'a>(root: &Path, rest: &'a str) -> Option<(&'a str, &'a str)> {
    let splits: Vec<(&str, &str)> = rest
        .match_indices(" -> ")
        .map(|(i, _)| (&rest[..i], &rest[i + 4..]))
        .collect();
    splits
        .iter()
        .find(|(name, _)| {
            root.join(name)
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
        })
        .or_else(|| splits.first())
        .copied()
}

/// Whether symlink `name` (relative to `root`) with target `target` points
/// outside `root`: an absolute target not under it, or a relative one whose
/// `..` climb above it.
pub fn leaves_tree(root: &Path, name: &Path, target: &Path) -> bool {
    use std::path::Component;
    if target.is_absolute() {
        let t = crate::never_sync::lexical(target);
        return !(t.starts_with(root) || root.canonicalize().is_ok_and(|r| t.starts_with(r)));
    }
    let mut depth = name.parent().map_or(0, |p| {
        p.components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .count()
    });
    for c in target.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return true,
        }
    }
    false
}

/// Lists what a sync of `src` with `filters` would send, with the rsync it
/// runs (`--list-only`, nothing is copied), and counts the symlinks whose
/// target lies outside `src`. Refuses, before any byte is sent, when that
/// rsync would send the file such a link points to in place of the link (a
/// wrapper on `PATH` that adds `--copy-links` or `--copy-unsafe-links`).
pub fn preflight(src: &Path, filters: &[String], b: &Bounds, deadline: Instant) -> Result<usize> {
    let mut rsync = Command::new("rsync");
    rsync
        .args(["-a", "-8", "--list-only"])
        .args(filters)
        .arg(format!("{}/", src.display()));
    let out = run_watched(&mut rsync, b, deadline)?;
    let mut outside = 0;
    let mut followed = Vec::new();
    for line in out.lines() {
        let Some((kind, rest)) = list_entry(line) else {
            continue;
        };
        if kind == 'l' {
            if let Some((name, target)) = split_link(src, rest)
                && leaves_tree(src, Path::new(name), Path::new(target))
            {
                outside += 1;
            }
            continue;
        }
        if rest == "." {
            continue;
        }
        let p = src.join(rest);
        if p.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
            && std::fs::read_link(&p).is_ok_and(|t| leaves_tree(src, Path::new(rest), &t))
        {
            followed.push(rest.to_string());
        }
    }
    if let Some(first) = followed.first() {
        bail!(
            "the rsync on PATH would send what {} symlink{} outside the tree point{} to \
             ({first}{}): it adds --copy-links or --copy-unsafe-links; nothing was synced",
            followed.len(),
            if followed.len() == 1 { "" } else { "s" },
            if followed.len() == 1 { "s" } else { "" },
            if followed.len() > 1 { ", …" } else { "" },
        );
    }
    Ok(outside)
}

/// Syncs the checkout at `top` to mirror `id` on the server, with the
/// toolchain's `excludes`, within `b`. rsync compares every file by content
/// (`--checksum`), not by size and time: a file changed at the same size and
/// time, or a sibling's file of that size and time, would otherwise be taken
/// as it is and the job would build the old content. The mirror keeps its
/// own modification times ([`BY_CONTENT`]).
pub fn sync(top: &Path, id: &str, excludes: &[&str], r: &Remote<'_>, b: &Bounds) -> Result<Synced> {
    let sources = Sources::of(top);
    let asked = Instant::now();
    let _lock = crate::paths::runtime_dir()
        .map(|dir| lock_workspace(&dir, r.host, id, b.limit))
        .transpose()?;
    let t0 = Instant::now();
    let deadline = t0 + b.limit;
    // A same-path mirror keeps a linked worktree's `.git` file; the git
    // directories follow in syncs of their own (`sync_extra`).
    let mut filters = vec![
        "--filter=:- .gitignore".to_string(),
        if r.same_path {
            "--exclude=/.git/"
        } else {
            "--exclude=/.git"
        }
        .to_string(),
    ];
    filters.extend(excludes.iter().map(|x| format!("--exclude={x}")));
    let outside_links = preflight(top, &filters, b, deadline)?;
    let key = mirror::repo_key(top);
    let siblings = ask_siblings(r, top, id, &key, b.stall.min(b.limit));
    let mut rsync = Command::new("rsync");
    // `-a` keeps every symlink a link: a link that leaves the tree is never
    // sent as the file it points to.
    rsync
        .arg("-a")
        .args(BY_CONTENT)
        .args(["--delete", "--mkpath"])
        .args(&filters);
    if let Some(sibs) = &siblings {
        rsync.arg("--stats");
        for s in sibs {
            if let Some(d) = r.copy_dest(s) {
                rsync.arg(format!("--copy-dest={d}"));
            }
        }
    }
    rsync
        .arg("-e")
        .arg(r.ssh.join(" "))
        .arg(format!("{}/", top.display()))
        .arg(format!("{}:{}/", r.host, r.dest(top, id)));
    let out = run_watched(&mut rsync, b, deadline)?;
    Ok(Synced {
        bytes: siblings.as_ref().and_then(|_| stats_bytes_sent(&out)),
        elapsed: t0.elapsed(),
        siblings: siblings.map_or(0, |s| s.len()),
        waited: t0.duration_since(asked),
        sources,
        outside_links,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A node reached through a fake ssh that runs the server's command in
    /// `<root>/node` (its home) and writes that command line to
    /// `<root>/server-line`: `(home, ssh)`.
    fn fake_node(root: &Path) -> (PathBuf, Vec<String>) {
        let home = root.join("node");
        std::fs::create_dir_all(&home).unwrap();
        let script = root.join("ssh.sh");
        std::fs::write(
            &script,
            "home=$1; shift 2; cd \"$home\" || exit 1\n\
             printf '%s\\n' \"$*\" > \"$home/../server-line\"\nexec \"$@\"\n",
        )
        .unwrap();
        let ssh = vec![
            "sh".to_string(),
            script.display().to_string(),
            home.display().to_string(),
        ];
        (home, ssh)
    }

    /// Copies `src` to entry `entry` of the fake node at `root`.
    fn copy_to_node(root: &Path, src: &Path, entry: &str) -> PathBuf {
        let (home, ssh) = fake_node(root);
        let r = Remote {
            host: "node",
            ssh: &ssh,
            ws_root: ".local/state/slot/ws",
            remote_slot: "slot",
            same_path: false,
        };
        copy_db_entry(src, entry, &r, &Bounds::default()).unwrap();
        home
    }

    #[test]
    fn no_alternates_of_a_git_database_reach_the_node() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let src = root.join("db-here");
        std::fs::create_dir_all(src.join("objects/info")).unwrap();
        std::fs::create_dir_all(src.join("objects/ab")).unwrap();
        std::fs::write(src.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(src.join("objects/ab/cd"), "x").unwrap();
        std::fs::write(src.join("objects/info/alternates"), "/srv/here/objects\n").unwrap();
        std::fs::write(src.join("objects/info/http-alternates"), "https://h/o\n").unwrap();
        let home = copy_to_node(&root, &src, "dep-00ff00ff00ff00ff");
        let entry = home
            .join(crate::git_deps::NODE_DB)
            .join("dep-00ff00ff00ff00ff");
        assert!(entry.join("objects/ab/cd").is_file(), "the objects cross");
        assert!(entry.join("HEAD").is_file());
        for f in ["objects/info/alternates", "objects/info/http-alternates"] {
            assert!(!entry.join(f).exists(), "{f} crossed");
        }
    }

    #[test]
    fn a_link_inside_a_node_entry_is_replaced_not_written_through() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let src = root.join("db-here");
        std::fs::create_dir_all(src.join("objects/ab")).unwrap();
        std::fs::create_dir_all(src.join("refs/heads")).unwrap();
        std::fs::write(src.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(src.join("objects/ab/cd"), "x").unwrap();
        std::fs::write(src.join("refs/heads/main"), "0123\n").unwrap();
        // The node's entry already holds links out of it, where the copy
        // writes a file and two directories.
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("file"), "keep\n").unwrap();
        let entry = root
            .join("node")
            .join(crate::git_deps::NODE_DB)
            .join("dep-00ff00ff00ff00ff");
        std::fs::create_dir_all(&entry).unwrap();
        std::os::unix::fs::symlink(outside.join("file"), entry.join("HEAD")).unwrap();
        std::os::unix::fs::symlink(&outside, entry.join("objects")).unwrap();
        std::os::unix::fs::symlink(&outside, entry.join("refs")).unwrap();
        let home = copy_to_node(&root, &src, "dep-00ff00ff00ff00ff");
        let line = std::fs::read_to_string(root.join("server-line")).unwrap();
        let roots = crate::shell::Roots {
            ws: &home.join(".local/state/slot/ws"),
            cache: &home.join(".local/state/slot/cache"),
            same_path: &[],
            cargo_git_db: &home.join(crate::git_deps::NODE_DB),
        };
        assert!(
            matches!(
                crate::shell::plan(line.trim(), &home, &roots),
                Ok(crate::shell::Plan::Rsync(_))
            ),
            "the node admits the client's own copy: {line}"
        );
        assert_eq!(
            std::fs::read_dir(&outside).unwrap().count(),
            1,
            "nothing written outside the entry"
        );
        assert_eq!(
            std::fs::read_to_string(outside.join("file")).unwrap(),
            "keep\n"
        );
        for (p, dir) in [("HEAD", false), ("objects", true), ("refs", true)] {
            let m = std::fs::symlink_metadata(entry.join(p)).unwrap();
            assert!(!m.file_type().is_symlink(), "{p} is still a link");
            assert_eq!(m.is_dir(), dir, "{p}");
        }
        assert!(entry.join("objects/ab/cd").is_file());
    }

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
            waited: Duration::ZERO,
            sources: Sources::Git {
                head: Some("1a2b3c4".into()),
                changed: 3,
            },
            outside_links: 0,
        };
        assert_eq!(
            s.line("node"),
            "slot: synced to node in 1.2 s, 4096 bytes sent, 2 sibling mirrors as base, \
             at 1a2b3c4 + 3 changed files"
        );
        let after = Synced {
            waited: Duration::from_millis(3400),
            ..s.clone()
        };
        assert!(
            after.line("node").ends_with(
                ", after 3.4 s waiting for another sync of this worktree, at 1a2b3c4 + 3 changed files"
            ),
            "{}",
            after.line("node")
        );
        let fresh = Synced {
            sources: Sources::Git {
                head: None,
                changed: 1,
            },
            ..s.clone()
        };
        assert!(
            fresh
                .line("node")
                .ends_with(", at no commit + 1 changed file"),
            "{}",
            fresh.line("node")
        );
        let nogit = Synced {
            sources: Sources::Unknown("git: No such file or directory (os error 2)".into()),
            ..s.clone()
        };
        assert!(
            nogit.line("node").ends_with(
                ", sources not named, git unavailable (git: No such file or directory (os error 2))"
            ),
            "{}",
            nogit.line("node")
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
    fn a_link_leaves_the_tree_by_an_absolute_target_or_a_climb_above_it() {
        let root = Path::new("/w/tree");
        let leaves =
            |name: &str, target: &str| leaves_tree(root, Path::new(name), Path::new(target));
        assert!(leaves("a", "/etc/passwd"));
        assert!(leaves("a", "/w/treeish/x"));
        assert!(!leaves("a", "/w/tree/b/c"));
        assert!(leaves("a", "../x"));
        assert!(leaves("sub/a", "../../x"));
        assert!(leaves("sub/a", "./../b/../../x"));
        assert!(!leaves("sub/a", "../b"));
        assert!(!leaves("sub/deeper/a", "../../b/./c"));
        assert!(!leaves("a", "b/c"));
    }

    #[test]
    fn a_listing_line_gives_its_type_and_path() {
        assert_eq!(
            list_entry("drwxr-xr-x          4,096 2026/11/24 12:34:41 ."),
            Some(('d', "."))
        );
        assert_eq!(
            list_entry("lrwxrwxrwx             15 2026/11/24 12:34:41 sub/up -> ../../f"),
            Some(('l', "sub/up -> ../../f"))
        );
        assert_eq!(
            list_entry("-rw-r--r--              2 2026/11/24 12:34:41  lead space"),
            Some(('-', " lead space"))
        );
        assert_eq!(list_entry("total size is 0"), None);
        let t = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("y", t.path().join("a -> b")).unwrap();
        assert_eq!(split_link(t.path(), "a -> b -> y"), Some(("a -> b", "y")));
        assert_eq!(split_link(t.path(), "c -> d"), Some(("c", "d")));
    }

    #[test]
    fn the_preflight_counts_links_that_leave_the_tree_and_copies_nothing() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let tree = root.join("tree");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::create_dir_all(tree.join("out")).unwrap();
        std::fs::write(root.join("secret"), "s").unwrap();
        std::fs::write(tree.join("sub/x"), "x").unwrap();
        std::fs::write(tree.join(".gitignore"), "out/\n").unwrap();
        std::os::unix::fs::symlink(root.join("secret"), tree.join("abs")).unwrap();
        std::os::unix::fs::symlink("../../secret", tree.join("sub/up")).unwrap();
        std::os::unix::fs::symlink("sub/x", tree.join("inside")).unwrap();
        std::os::unix::fs::symlink("../../secret", tree.join("out/ignored")).unwrap();
        let filters = ["--filter=:- .gitignore".to_string()];
        let b = Bounds::default();
        let n = preflight(&tree, &filters, &b, Instant::now() + b.limit).unwrap();
        assert_eq!(n, 2, "abs and sub/up; inside stays inside, out/ is ignored");
    }

    #[test]
    fn the_sources_are_the_head_and_the_changed_files_git_does_not_ignore() {
        let d = tempfile::tempdir().unwrap();
        let top = d.path().join("r");
        std::fs::create_dir(&top).unwrap();
        let git = |args: &[&str]| {
            let st = Command::new("git")
                .arg("-C")
                .arg(&top)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
                .args([
                    "-c",
                    "commit.gpgSign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(top.join("a"), "1").unwrap();
        assert_eq!(
            Sources::of(&top),
            Sources::Git {
                head: None,
                changed: 1
            }
        );
        std::fs::write(top.join(".gitignore"), "out/\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "c"]);
        std::fs::write(top.join("a"), "2").unwrap();
        std::fs::write(top.join("new"), "n").unwrap();
        std::fs::create_dir(top.join("out")).unwrap();
        std::fs::write(top.join("out/x"), "ignored").unwrap();
        let Sources::Git { head, changed } = Sources::of(&top) else {
            panic!("{:?}", Sources::of(&top));
        };
        assert_eq!(changed, 2);
        assert!(head.is_some_and(|h| h.len() >= 7 && h.chars().all(|c| c.is_ascii_hexdigit())));
        assert!(matches!(
            Sources::of(&d.path().join("missing")),
            Sources::Unknown(_)
        ));
    }

    #[test]
    fn one_sync_of_a_worktree_to_a_server_runs_at_a_time() {
        let d = tempfile::tempdir().unwrap();
        let held = lock_workspace(d.path(), "node", "00aa", Duration::from_secs(1)).unwrap();
        let t0 = Instant::now();
        let second = lock_workspace(d.path(), "node", "00aa", Duration::from_millis(300));
        assert!(second.is_err(), "a second sync ran beside the first");
        assert!(t0.elapsed() >= Duration::from_millis(300), "it waited");
        assert!(lock_workspace(d.path(), "other", "00aa", Duration::ZERO).is_ok());
        assert!(lock_workspace(d.path(), "node", "00bb", Duration::ZERO).is_ok());
        let waiter = {
            let dir = d.path().to_path_buf();
            std::thread::spawn(move || {
                let t = Instant::now();
                lock_workspace(&dir, "node", "00aa", Duration::from_secs(10)).map(|_| t.elapsed())
            })
        };
        std::thread::sleep(Duration::from_millis(400));
        drop(held);
        let waited = waiter.join().unwrap().unwrap();
        assert!(waited >= Duration::from_millis(300), "{waited:?}");
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
