//! `slot shell`: the ssh forced command for a build server.
//!
//! In `~/.ssh/authorized_keys` on the server:
//!
//! ```text
//! command="/path/to/slot shell",restrict ssh-ed25519 AAAA… workstation
//! ```
//!
//! ssh then runs `slot shell` for every connection with that key and puts
//! what the client asked for in `SSH_ORIGINAL_COMMAND`. `slot shell` runs
//! it only if it is one of the steps of remote mode:
//!
//! - the sync: `rsync --server … . <ws-root>/<id>/`;
//! - a fetch: `rsync --server --sender … . <path>…`, each path under the
//!   workspace root or the cache root's `target/` (a cargo job's products
//!   come back in one fetch of several paths);
//! - `slot state`, read-only: the node daemon's own row and its jobs, for
//!   the endpoint's poll;
//! - `slot output <id> [<from>]`, read-only: the kept output of one job,
//!   for the endpoint's page;
//! - `slot forget <path>`, read-write: removes one tree that reached this
//!   node, a workspace mirror `<ws-root>/<id>` or a tree below a
//!   `--same-path-root`, with the workspace root's records of it, and
//!   prints what it removed and the bytes freed. Refused: `..`, a root
//!   itself, any other path, a symbolic link at or below the root, and a
//!   tree a queued or running job of the daemon (read from `--http`) uses;
//!   `slot nodes forget` runs it;
//! - `slot proxy`, `slot cancel <id>` and `slot __siblings <ws-root> <id>
//!   <key> [<tree>]`, run by this binary whatever path the client named; the
//!   workspace root must be this server's;
//! - `slot __cargo-git <entry>:<rev>…`, read-only: which entries of this
//!   account's cargo git database lack the revision (entry names only, no
//!   path; no git runs); and the copy of one such entry,
//!   `rsync --server … . ~/.cargo/git/db/<entry>/`, the git dependencies of
//!   a job (`git_deps.rs`). The database is always the one under the
//!   account's home, never the session's `CARGO_HOME`. That copy takes
//!   exactly the client's options (`sync::copy_db_entry`): the short flags
//!   `r` and `t` and the long options `--server` and `--mkpath`. Every
//!   other one is refused, `-a` (which keeps links and special files) and
//!   `--copy-dest` among them, and so is an entry that is a symbolic link.
//!
//! With `--same-path-root DIR`, a sync, a fetch and the sibling step may
//! also name an absolute path below DIR: a tree mirrored at the client's own
//! path (`SLOT_REMOTE_SAME_PATH`). Its sync may then take `--copy-dest` as
//! `<ws-root>/<id>`, absolute.
//!
//! Anything else is refused with exit 126. rsync options are checked
//! against a list that leaves out the ones that name other files or
//! programs (`--rsync-path`, `-e`, `--log-file`, `--files-from`,
//! `--write-batch` and the like). The one that names a directory,
//! `--copy-dest`, is taken only as `../<id>` of a sync, a sibling mirror
//! under the workspace root.
//!
//! This narrows the key to slot's protocol: no shell, no port or agent
//! forwarding (`restrict`), no file outside the two roots. It is not a
//! sandbox: a job is any command, so whoever holds the key can still run
//! code on the server as this user through `slot proxy`.

use std::path::{Component, Path, PathBuf};

/// The start of the one line the proxy step prints when the daemon's
/// socket is missing or refuses: the daemon drains, restarts or died.
pub const DAEMON_AWAY: &str = "slot shell: daemon not answering:";

/// The exit status of the proxy step in that case (sysexits'
/// `EX_TEMPFAIL`): not 255, which ssh keeps for a failure of its own, and
/// not 3, which the step keeps for any other failure.
pub const DAEMON_AWAY_EXIT: i32 = 75;

/// Whether a failed connect to the daemon's socket means the daemon is
/// away for a while: the socket is missing (a draining daemon removed it,
/// a restarting one has not bound it yet) or nothing listens on it.
pub fn daemon_away(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

/// What `slot shell` runs for one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Exec `rsync` with these arguments (without the program name).
    Rsync(Vec<String>),
    /// Bridge stdin and stdout to the daemon.
    Proxy,
    /// Cancel job `id`.
    Cancel(i64),
    /// Print the node daemon's own node row and its jobs as one JSON
    /// object, read from its web address: the read-only `state` step an
    /// endpoint polls.
    State,
    /// Print the kept output of job `id` from its chunk `from` on as one
    /// JSON object, read from the node daemon's web address: the read-only
    /// `output` step an endpoint relays to its page.
    Output {
        /// Job id on this node.
        id: i64,
        /// The first chunk wanted.
        from: u64,
    },
    /// Record mirror `id` under repository `key` and print its siblings.
    Siblings {
        /// Workspace id.
        id: String,
        /// Repository key.
        key: String,
        /// The tree of a same-path mirror: `<ws-root>/<id>` links to it.
        path: Option<PathBuf>,
    },
    /// Print the wanted entries of the account's cargo git database that
    /// lack their revision, by name only (`git_deps::node_answer`).
    CargoGit(Vec<crate::git_deps::Wanted>),
    /// Remove a tree that reached this node: a workspace mirror
    /// `<ws-root>/<id>` or a tree below a same-path root ([`forget`]).
    Forget(PathBuf),
}

/// The directories a client's paths may name.
#[derive(Debug, Clone, Copy)]
pub struct Roots<'a> {
    /// The daemon's workspace root.
    pub ws: &'a Path,
    /// The daemon's cache root.
    pub cache: &'a Path,
    /// `--same-path-root`: where a tree may be mirrored at its own path.
    pub same_path: &'a [PathBuf],
    /// The account's cargo git database: a sync may write one entry
    /// directly below it.
    pub cargo_git_db: &'a Path,
}

/// rsync long options a sync or fetch may carry.
const RSYNC_LONG: &[&str] = &[
    "--server",
    "--sender",
    "--delete",
    "--delete-before",
    "--delete-during",
    "--delete-after",
    "--delete-delay",
    "--mkpath",
    "--partial",
    "--numeric-ids",
    "--safe-links",
    "--copy-unsafe-links",
    "--no-implied-dirs",
    "--checksum",
    "--compress",
    "--ignore-errors",
    "--force",
    "--inplace",
    "--stats",
];

/// rsync long options that take a value and may carry one.
const RSYNC_LONG_VALUE: &[&str] = &[
    "--bwlimit",
    "--compress-level",
    "--compress-choice",
    "--checksum-choice",
];

/// Short-option letters rsync's client sends in server mode that open no
/// other file or program. `e` is the protocol capability string after `.`
/// (`-logDtpre.iLsfxCIvu`), checked separately.
const RSYNC_SHORT: &str = "aAbcCdDgHiIJkKlLmnNoOpPqrRStuUvWxXyz";

/// Splits a command line the way a POSIX shell splits words, without
/// expansion: single quotes, double quotes and backslash escapes. Refuses
/// unquoted shell operators, so nothing is taken for a separate command.
pub fn split(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(x @ ('"' | '\\' | '$' | '`')) => cur.push(x),
                            Some(x) => {
                                cur.push('\\');
                                cur.push(x);
                            }
                            None => return Err("unterminated double quote".into()),
                        },
                        Some('$' | '`') => return Err("expansion in a quoted word".into()),
                        Some(x) => cur.push(x),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(x) => cur.push(x),
                    None => return Err("trailing backslash".into()),
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            ';' | '&' | '|' | '<' | '>' | '$' | '`' | '(' | ')' | '*' | '?' | '[' | '~' => {
                return Err(format!("shell character `{c}`"));
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// Decides what to run for `SSH_ORIGINAL_COMMAND` = `line`. `home` is the
/// account's home directory (relative paths are taken from it, as ssh
/// does), `roots` the directories its paths may name.
pub fn plan(line: &str, home: &Path, roots: &Roots<'_>) -> Result<Plan, String> {
    let ws_root = roots.ws;
    let words = split(line)?;
    let Some((prog, args)) = words.split_first() else {
        return Err("no command: this key runs only slot's sync and build steps".into());
    };
    let base = Path::new(prog)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match base.as_str() {
        "rsync" => rsync(args, home, roots).map(Plan::Rsync),
        "slot" => match args {
            [p] if p == "proxy" => Ok(Plan::Proxy),
            [s] if s == "state" => Ok(Plan::State),
            [o, id, rest @ ..] if o == "output" && rest.len() <= 1 => Ok(Plan::Output {
                id: id.parse().map_err(|_| format!("bad job id `{id}`"))?,
                from: match rest {
                    [f] => f.parse().map_err(|_| format!("bad chunk number `{f}`"))?,
                    _ => 0,
                },
            }),
            [c, id] if c == "cancel" => id
                .parse()
                .map(Plan::Cancel)
                .map_err(|_| format!("bad job id `{id}`")),
            [c, root, id, key, rest @ ..] if c == "__siblings" && rest.len() <= 1 => {
                if absolute(home, root)? != absolute(home, &ws_root.to_string_lossy())? {
                    return Err(format!("`{root}` is not this server's workspace root"));
                }
                if !crate::mirror::valid_id(id) || !crate::mirror::valid_id(key) {
                    return Err("bad workspace id or repository key".into());
                }
                let path = match rest {
                    [p] => Some(same_path(home, p, roots.same_path)?),
                    _ => None,
                };
                Ok(Plan::Siblings {
                    id: id.clone(),
                    key: key.clone(),
                    path,
                })
            }
            [f, path] if f == "forget" => forget_path(home, path, roots).map(Plan::Forget),
            [c, words @ ..] if c == "__cargo-git" && !words.is_empty() => {
                crate::git_deps::parse_words(words)
                    .map(Plan::CargoGit)
                    .ok_or_else(|| "`slot __cargo-git` takes `<entry>:<rev>` words".into())
            }
            _ => Err(format!("`slot {}` is not a remote step", args.join(" "))),
        },
        _ => Err(format!("`{prog}` is not a remote step")),
    }
}

/// The tree the `forget` step may remove: `path` made absolute against
/// `home`, either a workspace mirror (`<ws-root>/<id>`) or a path below a
/// same-path root, written absolute. Refused: `..`, a root itself, any
/// other path, and a path with a symbolic link at or below its root.
fn forget_path(home: &Path, path: &str, roots: &Roots<'_>) -> Result<PathBuf, String> {
    let p = absolute(home, path)?;
    let ws = absolute(home, &roots.ws.to_string_lossy())?;
    let mut same_roots = Vec::new();
    for r in roots.same_path {
        same_roots.push(absolute(home, &r.to_string_lossy())?);
    }
    if p == ws || same_roots.contains(&p) {
        return Err(format!("`{path}` is a root itself: name one tree below it"));
    }
    let mirror = p.parent() == Some(ws.as_path())
        && p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(crate::mirror::valid_id);
    let base = if mirror {
        Some(ws.clone())
    } else if Path::new(path).is_absolute() {
        same_roots.into_iter().find(|r| p.starts_with(r))
    } else {
        None
    };
    let Some(base) = base else {
        return Err(format!(
            "`{path}` is outside the roots: only a mirror `{}/<id>` or a tree below a --same-path-root is removed",
            ws.display()
        ));
    };
    for a in p.ancestors().take_while(|a| *a != base) {
        if std::fs::symlink_metadata(a).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("`{}` is a symbolic link", a.display()));
        }
    }
    Ok(p)
}

/// What a queued or running job of the node daemon works in: its id and
/// its directories (its working directory and its declared trees).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    /// Job id on this node.
    pub id: i64,
    /// Its directories.
    pub dirs: Vec<PathBuf>,
}

/// The queued and running jobs of `/api/state` (`state`), this daemon's
/// own: their working directories and declared trees.
pub fn busy_of(state: &str) -> Result<Vec<Busy>, String> {
    let v: serde_json::Value =
        serde_json::from_str(state).map_err(|e| format!("/api/state: {e}"))?;
    let active = v
        .get("active")
        .and_then(serde_json::Value::as_array)
        .ok_or("/api/state has no `active`")?;
    Ok(active
        .iter()
        .filter(|j| {
            !j.get("from_node")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .map(|j| {
            let id = j.get("id").and_then(serde_json::Value::as_i64).unwrap_or(0);
            let cwd = j.get("cwd").and_then(serde_json::Value::as_str);
            let extra = j
                .get("extra_trees")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str);
            let dirs = cwd
                .into_iter()
                .chain(extra)
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .collect();
            Busy { id, dirs }
        })
        .collect())
}

/// Removes the tree at `path` (checked by the `forget` step's plan) unless
/// a job of `busy` uses it: a directory of the job, the tree of the
/// workspace mirror it works in, or the same-path tree the workspace root
/// links to for it, is `path`, lies under it or holds it. For a mirror its
/// records in the workspace root `ws` go too; for a same-path tree, every
/// link of the workspace root to it or below it. Returns one line for each
/// thing removed, the tree's with the bytes freed.
pub fn forget(path: &Path, ws: &Path, busy: &[Busy]) -> Result<Vec<String>, String> {
    let path = crate::never_sync::lexical(path);
    let ws = &crate::never_sync::lexical(ws);
    for job in busy {
        for d in &job.dirs {
            let d = crate::never_sync::lexical(d);
            // The synced tree the directory lies in: its mirror, or the
            // same-path tree the workspace root links to.
            let mut trees = Vec::new();
            if let Ok(rel) = d.strip_prefix(ws)
                && let Some(first) = rel.components().next()
            {
                trees.push(ws.join(first));
            }
            if let Some(id) = crate::mirror::linked_id(ws, &d)
                && let Ok(to) = std::fs::read_link(ws.join(id))
            {
                trees.push(crate::never_sync::lexical(&to));
            }
            // A directory above the path (a local job in the account's
            // home) does not hold it; a synced tree above it does.
            if d.starts_with(&path)
                || trees
                    .iter()
                    .any(|t| t.starts_with(&path) || path.starts_with(t))
            {
                return Err(format!(
                    "{} is used by job {}; nothing removed",
                    path.display(),
                    job.id
                ));
            }
        }
    }
    let m = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !m.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    let bytes = crate::evict::dir_size(&path);
    std::fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut lines = vec![format!("removed {} ({bytes} bytes freed)", path.display())];
    let forget_id = |id: &str| {
        crate::mirror::forget(ws, id);
        crate::cargo_fetch::forget(ws, id);
    };
    if path.parent() == Some(ws.as_path()) {
        if let Some(id) = path.file_name().and_then(|n| n.to_str()) {
            forget_id(id);
        }
        return Ok(lines);
    }
    for e in std::fs::read_dir(ws).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(to) = std::fs::read_link(e.path()) else {
            continue;
        };
        if crate::mirror::valid_id(&name)
            && crate::never_sync::lexical(&to).starts_with(&path)
            && std::fs::remove_file(e.path()).is_ok()
        {
            forget_id(&name);
            lines.push(format!("removed {} (its link)", e.path().display()));
        }
    }
    Ok(lines)
}

/// The `forget` step: the node daemon's queued and running jobs, read from
/// its web address `http`, then [`forget`]. A daemon that does not answer
/// is an error: which trees its jobs use is not known.
pub fn forget_step(http: &str, path: &Path, ws: &Path) -> Result<Vec<String>, String> {
    let state = crate::status::get(http, "/api/state").map_err(|e| {
        format!("which trees jobs use is not known, the daemon at {http} does not answer ({e:#}); nothing removed")
    })?;
    forget(path, ws, &busy_of(&state)?)
}

/// `path`, absolute and under one of `roots`, the tree of a same-path
/// mirror.
fn same_path(home: &Path, path: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    if !Path::new(path).is_absolute() {
        return Err(format!("same-path mirror `{path}` is not absolute"));
    }
    let p = absolute(home, path)?;
    if !under_any(&p, home, roots)? {
        return Err(format!(
            "same-path mirror `{path}` is not under a --same-path-root"
        ));
    }
    Ok(p)
}

/// Whether `p` is below one of `roots` (not a root itself).
fn under_any(p: &Path, home: &Path, roots: &[PathBuf]) -> Result<bool, String> {
    for r in roots {
        let r = absolute(home, &r.to_string_lossy())?;
        if p.starts_with(&r) && p != r {
            return Ok(true);
        }
    }
    Ok(false)
}

fn rsync(args: &[String], home: &Path, roots: &Roots<'_>) -> Result<Vec<String>, String> {
    let (ws_root, cache_root) = (roots.ws, roots.cache);
    let ws = absolute(home, &ws_root.to_string_lossy())?;
    if args.first().map(String::as_str) != Some("--server") {
        return Err("rsync runs only in server mode".into());
    }
    let sender = args.iter().any(|a| a == "--sender");
    let mut paths = Vec::new();
    let mut dot = false;
    // The options, checked again for a copy into the cargo git database.
    let mut shorts = String::new();
    let mut longs: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if dot {
            paths.push(a.as_str());
        } else if a == "." {
            dot = true;
        } else if a == "--copy-dest" || a.starts_with("--copy-dest=") {
            longs.push("--copy-dest".into());
            let value = match a.split_once('=') {
                Some((_, v)) => Some(v),
                None => it.next().map(String::as_str),
            };
            // `<ws-root>/<id>`, absolute: the sibling of a same-path mirror,
            // whose parent is not the workspace root.
            let in_ws = value
                .filter(|v| Path::new(v).is_absolute())
                .and_then(|v| absolute(home, v).ok())
                .and_then(|p| {
                    p.strip_prefix(&ws)
                        .ok()
                        .map(|r| r.to_string_lossy().into_owned())
                });
            let sibling = value
                .and_then(|v| v.strip_prefix("../"))
                .map(|v| v.strip_suffix('/').unwrap_or(v));
            if sender
                || !(sibling.is_some_and(crate::mirror::valid_id)
                    || in_ws.as_deref().is_some_and(crate::mirror::valid_id))
            {
                return Err(format!(
                    "rsync `--copy-dest` takes only `../<id>` or `<ws-root>/<id>` of a sync, not `{}`",
                    value.unwrap_or("")
                ));
            }
        } else if let Some(long) = a.strip_prefix("--") {
            let name = long.split_once('=').map_or(long, |(n, _)| n);
            let full = format!("--{name}");
            longs.push(full.clone());
            let ok = if long.contains('=') {
                RSYNC_LONG_VALUE.contains(&full.as_str())
            } else {
                RSYNC_LONG.contains(&full.as_str())
            };
            if !ok {
                return Err(format!("rsync option `{a}` is not allowed"));
            }
        } else if let Some(short) = a.strip_prefix('-') {
            // `-logDtpre.iLsfxCIvu`: the `e` right before `.` marks the
            // protocol capabilities that follow, not `-e COMMAND`.
            let flags = short
                .split_once('.')
                .map_or(short, |(f, _)| f.strip_suffix('e').unwrap_or(f));
            shorts.push_str(flags);
            if let Some(c) = flags.chars().find(|c| !RSYNC_SHORT.contains(*c)) {
                return Err(format!("rsync option `-{c}` is not allowed"));
            }
        } else {
            return Err(format!("rsync argument `{a}` before `.`"));
        }
    }
    // A sync names its mirror; a fetch names one path, or, for the products
    // of a cargo job, several (`artifacts.rs`).
    if paths.is_empty() || (!sender && paths.len() > 1) {
        return Err("rsync must name one path".into());
    }
    let target = absolute(home, &cache_root.join("target").to_string_lossy())?;
    let git_db = absolute(home, &roots.cargo_git_db.to_string_lossy())?;
    for path in paths {
        let p = absolute(home, path)?;
        let under = |root: &Path| p.starts_with(root) && p != root;
        let same = Path::new(path).is_absolute() && under_any(&p, home, roots.same_path)?;
        // One entry of the cargo git database, written, never read back.
        let db_entry = !sender
            && p.parent() == Some(git_db.as_path())
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(crate::git_deps::valid_entry);
        if db_entry {
            db_copy(&p, &shorts, &longs)?;
        }
        if !(under(&ws) || same || (sender && under(&target)) || db_entry) {
            return Err(format!(
                "rsync path `{path}` is outside {}{}",
                ws.display(),
                if sender {
                    format!(" and {}", target.display())
                } else {
                    String::new()
                }
            ));
        }
    }
    Ok(args.to_vec())
}

/// rsync short options a copy into the cargo git database may carry: the
/// client's `-rt` (`sync::copy_db_entry`), and nothing else. Every other
/// flag is refused, `-a` (`-rlptgoD`: links, devices, special files,
/// owners) included.
const DB_SHORT: &str = "rt";

/// rsync long options a copy into the cargo git database may carry: the
/// client's, and nothing else (no `--copy-dest`, no delete, no link).
const DB_LONG: &[&str] = &["--server", "--mkpath"];

/// Whether a copy into the database entry `entry` with these options may
/// run: only [`DB_SHORT`] and [`DB_LONG`], and neither the entry nor the
/// database, `git/` or the cargo home above it is a symbolic
/// link.
fn db_copy(entry: &Path, shorts: &str, longs: &[String]) -> Result<(), String> {
    if let Some(c) = shorts.chars().find(|c| !DB_SHORT.contains(*c)) {
        return Err(format!(
            "rsync option `-{c}` is not allowed into the cargo git database"
        ));
    }
    if let Some(l) = longs.iter().find(|l| !DB_LONG.contains(&l.as_str())) {
        return Err(format!(
            "rsync option `{l}` is not allowed into the cargo git database"
        ));
    }
    // The entry, the database, `git/` and the cargo home: a link at any of
    // them would send the copy elsewhere.
    for p in entry.ancestors().take(4) {
        if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("`{}` is a symbolic link", p.display()));
        }
    }
    Ok(())
}

/// `path` made absolute against `home`, with `.` removed; `..` is refused.
fn absolute(home: &Path, path: &str) -> Result<PathBuf, String> {
    let mut out = if Path::new(path).is_absolute() {
        PathBuf::from("/")
    } else {
        home.to_path_buf()
    };
    for c in Path::new(path).components() {
        match c {
            Component::Normal(x) => out.push(x),
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err(format!("`..` in path `{path}`"));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: &str) -> Result<Plan, String> {
        plan_with(line, &[])
    }

    fn plan_with(line: &str, same_path: &[PathBuf]) -> Result<Plan, String> {
        plan(
            line,
            Path::new("/srv/u"),
            &Roots {
                ws: Path::new("/srv/u/.local/state/slot/ws"),
                cache: Path::new("/srv/u/.local/state/slot/cache"),
                same_path,
                cargo_git_db: Path::new("/srv/u/.cargo/git/db"),
            },
        )
    }

    #[test]
    fn a_git_dependency_copy_follows_no_link_and_deletes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let db = home.join(".cargo/git/db");
        std::fs::create_dir_all(&db).unwrap();
        let roots = Roots {
            ws: &home.join("ws"),
            cache: &home.join("cache"),
            same_path: &[],
            cargo_git_db: &db,
        };
        let p = |l: &str| plan(l, &home, &roots);
        let ok = "rsync --server -rte.iLsfxCIvu --mkpath . .cargo/git/db/dep-00ff00ff00ff00ff/";
        assert!(matches!(p(ok), Ok(Plan::Rsync(_))), "{:?}", p(ok));
        for flags in [
            "-logDtpre.iLsfxCIvu",
            "-rtle.iLsfxCIvu",
            "-rtLe.iLsfxCIvu",
            "-rtKe.iLsfxCIvu",
            "-rtke.iLsfxCIvu",
            "-rtRe.iLsfxCIvu",
        ] {
            let l = format!("rsync --server {flags} . .cargo/git/db/dep-00ff00ff00ff00ff/");
            assert!(p(&l).is_err(), "accepted: {l}");
        }
        for opt in [
            "--delete",
            "--delete-after",
            "--inplace",
            "--copy-unsafe-links",
            "--safe-links",
        ] {
            let l = format!(
                "rsync --server -rte.iLsfxCIvu {opt} . .cargo/git/db/dep-00ff00ff00ff00ff/"
            );
            assert!(p(&l).is_err(), "accepted: {l}");
        }
        std::fs::create_dir_all(home.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(home.join("elsewhere"), db.join("dep-11ee11ee11ee11ee"))
            .unwrap();
        let link = "rsync --server -rte.iLsfxCIvu . .cargo/git/db/dep-11ee11ee11ee11ee/";
        assert!(p(link).is_err(), "an entry that is a link");
        let sync = "rsync --server -logDtpre.iLsfxCIvu --delete --mkpath . ws/0123abcd/";
        assert!(
            matches!(p(sync), Ok(Plan::Rsync(_))),
            "the sync keeps its flags"
        );
    }

    /// A symbolic link at the database, at `git/` or at the cargo home
    /// would send the copy outside the database: refused.
    #[test]
    fn a_link_above_the_database_entry_is_refused() {
        for linked in [".cargo/git/db", ".cargo/git", ".cargo"] {
            let t = tempfile::tempdir().unwrap();
            let home = t.path().canonicalize().unwrap();
            let elsewhere = home.join("elsewhere");
            let real = elsewhere.join(".cargo/git/db");
            std::fs::create_dir_all(&real).unwrap();
            let link = home.join(linked);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(elsewhere.join(linked), &link).unwrap();
            let db = home.join(".cargo/git/db");
            let roots = Roots {
                ws: &home.join("ws"),
                cache: &home.join("cache"),
                same_path: &[],
                cargo_git_db: &db,
            };
            let l = "rsync --server -rte.iLsfxCIvu --mkpath . .cargo/git/db/dep-00ff00ff00ff00ff/";
            let got = plan(l, &home, &roots);
            assert!(
                got.as_ref()
                    .is_err_and(|e| e.contains("is a symbolic link")),
                "{linked}: {got:?}"
            );
        }
    }
    #[test]
    fn a_copy_into_the_cargo_git_database_takes_only_the_clients_flags() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let db = home.join(".cargo/git/db");
        std::fs::create_dir_all(home.join("ws/00aa")).unwrap();
        let roots = Roots {
            ws: &home.join("ws"),
            cache: &home.join("cache"),
            same_path: &[],
            cargo_git_db: &db,
        };
        let p = |l: &str| plan(l, &home, &roots);
        let entry = ".cargo/git/db/dep-00ff00ff00ff00ff/";
        for ok in [
            "-rte.iLsfxCIvu --mkpath",
            "-tre.iLsfxCIvu --mkpath",
            "-rte.iLsfxCIvu",
            "-rt --mkpath",
        ] {
            let l = format!("rsync --server {ok} . {entry}");
            assert!(matches!(p(&l), Ok(Plan::Rsync(_))), "{l}: {:?}", p(&l));
        }
        for bad in [
            "-ae.iLsfxCIvu --mkpath",
            "-rtae.iLsfxCIvu",
            "-rtDe.iLsfxCIvu",
            "-rtle.iLsfxCIvu",
            "-rtpe.iLsfxCIvu",
            "-rtoge.iLsfxCIvu",
            "-rtHe.iLsfxCIvu",
            "-rtxe.iLsfxCIvu",
            "-rtbe.iLsfxCIvu",
            "-rte.iLsfxCIvu --copy-dest=../00aa",
            "-rte.iLsfxCIvu --copy-dest ../00aa",
            "-rte.iLsfxCIvu --partial",
            "-rte.iLsfxCIvu --numeric-ids",
            "-rte.iLsfxCIvu --no-implied-dirs",
            "-rte.iLsfxCIvu --bwlimit=100",
            "-rte.iLsfxCIvu --compress",
            "-rte.iLsfxCIvu --stats",
        ] {
            let l = format!("rsync --server {bad} . {entry}");
            assert!(p(&l).is_err(), "accepted: {l}");
        }
    }

    #[test]
    fn a_git_dependency_is_asked_for_and_copied_into_the_cargo_git_database_only() {
        let rev = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            p(&format!("slot __cargo-git dep-00ff00ff00ff00ff:{rev}")),
            Ok(Plan::CargoGit(vec![crate::git_deps::Wanted {
                entry: "dep-00ff00ff00ff00ff".into(),
                rev: rev.into(),
            }]))
        );
        for bad in [
            "slot __cargo-git".to_string(),
            format!("slot __cargo-git ../x-1:{rev}"),
            "slot __cargo-git dep-1:HEAD".to_string(),
        ] {
            assert!(p(&bad).is_err(), "accepted: {bad}");
        }
        for ok in [
            "rsync --server -rte.iLsfxCIvu --mkpath . /srv/u/.cargo/git/db/dep-00ff00ff00ff00ff/",
            "rsync --server -rte.iLsfxCIvu --mkpath . .cargo/git/db/dep-00ff00ff00ff00ff",
        ] {
            assert!(matches!(p(ok), Ok(Plan::Rsync(_))), "{ok}: {:?}", p(ok));
        }
        for bad in [
            "rsync --server -rte.iLsfxCIvu . /srv/u/.cargo/git/db/",
            "rsync --server -rte.iLsfxCIvu . /srv/u/.cargo/git/db/dep-1/objects",
            "rsync --server -rte.iLsfxCIvu . /srv/u/.cargo/git/db/.hidden-1",
            "rsync --server -rte.iLsfxCIvu . /srv/u/.cargo/git/db/../../.ssh",
            "rsync --server -rte.iLsfxCIvu . /srv/u/.cargo/credentials-1",
            "rsync --server --sender -rte.iLsfxCIvu . /srv/u/.cargo/git/db/dep-1",
        ] {
            assert!(p(bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn sync_and_fetch_pass() {
        let sync =
            "rsync --server -logDtpre.iLsfxCIvu --delete --mkpath . .local/state/slot/ws/0123abcd/";
        assert!(matches!(p(sync), Ok(Plan::Rsync(_))), "{:?}", p(sync));
        let fetch = "rsync --server --sender -logDtpre.iLsfxCIvu . /srv/u/.local/state/slot/cache/target/0123abcd/debug/ess";
        assert!(matches!(p(fetch), Ok(Plan::Rsync(_))), "{:?}", p(fetch));
        let mirror =
            "rsync --server --sender -logDtpre.iLsfxCIvu . .local/state/slot/ws/0123abcd/bin/tool";
        assert!(matches!(p(mirror), Ok(Plan::Rsync(_))));
    }

    #[test]
    fn a_sync_with_a_bandwidth_cap_passes() {
        let capped = "rsync --server -logDtpre.iLsfxCIvu --bwlimit=2700 --delete --mkpath . .local/state/slot/ws/0123abcd/";
        assert!(matches!(p(capped), Ok(Plan::Rsync(_))), "{:?}", p(capped));
    }

    #[test]
    fn a_fetch_of_products_names_several_paths_under_the_target_dir() {
        let t = "/srv/u/.local/state/slot/cache/target/0123abcd/./debug";
        let products =
            format!("rsync --server --sender -logDtprRe.iLsfxCIvu . {t}/tiny {t}/deps/it-1f");
        assert!(
            matches!(p(&products), Ok(Plan::Rsync(_))),
            "{:?}",
            p(&products)
        );
        for bad in [
            format!("rsync --server --sender -logDtprRe.iLsfxCIvu . {t}/tiny /srv/u/.ssh/id"),
            format!("rsync --server --sender -logDtprRe.iLsfxCIvu . {t}/tiny {t}/../../../../x"),
            "rsync --server -logDtprRe.iLsfxCIvu . .local/state/slot/ws/a/ .local/state/slot/ws/b/"
                .to_string(),
            "rsync --server --sender -logDtprRe.iLsfxCIvu .".to_string(),
        ] {
            assert!(p(&bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn slot_steps_pass_whatever_the_path() {
        assert_eq!(p("/srv/u/.local/bin/slot proxy"), Ok(Plan::Proxy));
        assert_eq!(p("slot proxy"), Ok(Plan::Proxy));
        assert_eq!(p("slot cancel 42"), Ok(Plan::Cancel(42)));
        assert_eq!(p("/srv/u/.local/bin/slot state"), Ok(Plan::State));
        for bad in [
            "slot state x",
            "slot state --http 0:1",
            "slot state; id",
            "slot states",
        ] {
            assert!(p(bad).is_err(), "accepted: {bad}");
        }
        assert!(p("slot cancel x").is_err());
        assert_eq!(p("slot output 7"), Ok(Plan::Output { id: 7, from: 0 }));
        assert_eq!(
            p("/srv/u/.local/bin/slot output 7 12"),
            Ok(Plan::Output { id: 7, from: 12 })
        );
        for bad in [
            "slot output",
            "slot output x",
            "slot output 7 -1",
            "slot output 7 1 2",
            "slot output 7; id",
        ] {
            assert!(p(bad).is_err(), "accepted: {bad}");
        }
        assert!(p("slot serve").is_err());
        assert!(p("slot cargo build").is_err());
    }

    #[test]
    fn anything_else_is_refused() {
        for bad in [
            "",
            "bash",
            "sh -c id",
            "slot proxy; id",
            "slot proxy && id",
            "slot proxy | nc x 1",
            "slot proxy $(id)",
            "rsync --server -e sh . .local/state/slot/ws/x/",
            "rsync --server -le . .local/state/slot/ws/x/",
            "rsync --server -T /tmp . .local/state/slot/ws/x/",
            "rsync --server --rsync-path=sh . .local/state/slot/ws/x/",
            "rsync --server --log-file=/srv/u/.bashrc . .local/state/slot/ws/x/",
            "rsync --server --files-from=/etc/passwd . .local/state/slot/ws/x/",
            "rsync --server . .ssh/",
            "rsync --server . .local/state/slot/ws/../../../.ssh/",
            "rsync --server . .local/state/slot/ws",
            "rsync --server --sender . .ssh/id_ed25519",
            "rsync --server --sender . /etc/passwd",
            "rsync --server . .local/state/slot/cache/target/x/",
            "rsync -logDtpre.iLsfxCIvu . .local/state/slot/ws/x/",
            "rsync --server . a b",
        ] {
            assert!(p(bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn a_sync_may_copy_from_sibling_mirrors_and_count_its_bytes() {
        let sync = "rsync --server -logDtpre.iLsfxCIvu --delete --stats --copy-dest ../00aa --copy-dest=../00bb/ --mkpath . .local/state/slot/ws/0123abcd/";
        assert!(matches!(p(sync), Ok(Plan::Rsync(_))), "{:?}", p(sync));
        // What the client's `rsync -a --checksum` sends: `c` among the flags.
        let checksum = "rsync --server -logDtprce.iLsfxCIvu --delete --stats --copy-dest=../00aa --mkpath . .local/state/slot/ws/0123abcd/";
        assert!(
            matches!(p(checksum), Ok(Plan::Rsync(_))),
            "{:?}",
            p(checksum)
        );
        for bad in [
            "rsync --server --copy-dest ../../.ssh . .local/state/slot/ws/x/",
            "rsync --server --copy-dest /etc . .local/state/slot/ws/x/",
            "rsync --server --copy-dest=00aa . .local/state/slot/ws/x/",
            "rsync --server --copy-dest ../00aa/sub . .local/state/slot/ws/x/",
            "rsync --server --copy-dest . .local/state/slot/ws/x/",
            "rsync --server --sender --copy-dest ../00aa . .local/state/slot/ws/x/",
            "rsync --server --link-dest ../00aa . .local/state/slot/ws/x/",
            "rsync --server --compare-dest=../00aa . .local/state/slot/ws/x/",
        ] {
            assert!(p(bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn the_sibling_step_runs_only_on_this_servers_workspace_root() {
        let ok = Plan::Siblings {
            id: "0123abcd".into(),
            key: "00ff".into(),
            path: None,
        };
        assert_eq!(
            p("/srv/u/.local/bin/slot __siblings .local/state/slot/ws 0123abcd 00ff"),
            Ok(ok.clone())
        );
        assert_eq!(
            p("slot __siblings /srv/u/.local/state/slot/ws 0123abcd 00ff"),
            Ok(ok)
        );
        for bad in [
            "slot __siblings .ssh 0123abcd 00ff",
            "slot __siblings .local/state/slot/ws ../x 00ff",
            "slot __siblings .local/state/slot/ws 0123abcd ../00ff",
            "slot __siblings .local/state/slot/ws 0123abcd",
            "slot __siblings .local/state/slot/ws/../ws 0123abcd 00ff",
        ] {
            assert!(p(bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn a_same_path_mirror_is_admitted_only_under_a_same_path_root() {
        let roots = [PathBuf::from("/srv/u/trees")];
        let same = |l: &str| plan_with(l, &roots);
        let sync = "rsync --server -logDtpre.iLsfxCIvu --delete --stats --copy-dest=/srv/u/.local/state/slot/ws/00aa/ --copy-dest ../00bb --mkpath . /srv/u/trees/r/a/";
        assert!(matches!(same(sync), Ok(Plan::Rsync(_))), "{:?}", same(sync));
        let fetch = "rsync --server --sender -logDtpre.iLsfxCIvu . /srv/u/trees/r/a/bin/tool";
        assert!(
            matches!(same(fetch), Ok(Plan::Rsync(_))),
            "{:?}",
            same(fetch)
        );
        assert_eq!(
            same("slot __siblings .local/state/slot/ws 0123abcd 00ff /srv/u/trees/r/a"),
            Ok(Plan::Siblings {
                id: "0123abcd".into(),
                key: "00ff".into(),
                path: Some(PathBuf::from("/srv/u/trees/r/a")),
            })
        );
        assert!(
            p(sync).is_err() && p(fetch).is_err(),
            "without a root nothing outside the workspace root"
        );
        for bad in [
            "rsync --server --delete . /srv/u/trees/",
            "rsync --server --delete . /srv/u/trees",
            "rsync --server --delete . /srv/u/.ssh/",
            "rsync --server --delete . /srv/u/trees/../.ssh/",
            "rsync --server --delete . trees/r/a/",
            "rsync --server --delete . /srv/u/treesx/a/",
            "rsync --server --sender . /etc/passwd",
            "rsync --server --copy-dest=/srv/u/.ssh . /srv/u/trees/r/a/",
            "rsync --server --copy-dest=/srv/u/.local/state/slot/ws/00aa/sub . /srv/u/trees/r/a/",
            "rsync --server --copy-dest=/srv/u/.local/state/slot/ws/../../../.ssh . /srv/u/trees/r/a/",
            "rsync --server --copy-dest=/srv/u/trees/r/b . /srv/u/trees/r/a/",
            "slot __siblings .local/state/slot/ws 0123abcd 00ff /srv/u/.ssh",
            "slot __siblings .local/state/slot/ws 0123abcd 00ff trees/r/a",
            "slot __siblings .local/state/slot/ws 0123abcd 00ff /srv/u/trees",
            "slot __siblings .local/state/slot/ws 0123abcd 00ff /srv/u/trees/../x",
            "slot __siblings .local/state/slot/ws 0123abcd 00ff /srv/u/trees/a /srv/u/trees/b",
        ] {
            assert!(same(bad).is_err(), "accepted: {bad}");
        }
        assert!(
            p("slot __siblings .local/state/slot/ws 0123abcd 00ff /srv/u/trees/r/a").is_err(),
            "no root: no same-path sibling step"
        );
    }

    #[test]
    fn the_forget_step_takes_a_mirror_or_a_same_path_tree_only() {
        let roots = [PathBuf::from("/srv/u/trees")];
        let same = |l: &str| plan_with(l, &roots);
        let mirror = PathBuf::from("/srv/u/.local/state/slot/ws/0123abcd");
        assert_eq!(
            same("slot forget .local/state/slot/ws/0123abcd"),
            Ok(Plan::Forget(mirror.clone()))
        );
        assert_eq!(
            same(&format!("slot forget {}", mirror.display())),
            Ok(Plan::Forget(mirror))
        );
        assert_eq!(
            same("/srv/u/bin/slot forget '/srv/u/trees/r/a b'"),
            Ok(Plan::Forget(PathBuf::from("/srv/u/trees/r/a b")))
        );
        for (bad, why) in [
            ("slot forget /srv/u/trees/r/../x", "`..`"),
            ("slot forget /srv/u/trees", "root itself"),
            ("slot forget /srv/u/trees/", "root itself"),
            ("slot forget .local/state/slot/ws", "root itself"),
            ("slot forget .local/state/slot/ws/0123abcd/src", "outside"),
            ("slot forget .local/state/slot/ws/.repo-keys", "outside"),
            ("slot forget trees/r/a", "outside"),
            ("slot forget /srv/u/.ssh", "outside"),
            ("slot forget /", "outside"),
            ("slot forget", "not a remote step"),
            (
                "slot forget /srv/u/trees/a /srv/u/trees/b",
                "not a remote step",
            ),
        ] {
            let got = same(bad);
            assert!(
                got.as_ref().is_err_and(|e| e.contains(why)),
                "{bad}: {got:?}"
            );
        }
        assert!(
            p("slot forget /srv/u/trees/r/a").is_err(),
            "no root: no same-path tree"
        );
    }

    #[test]
    fn the_forget_step_refuses_a_link_at_or_below_the_root() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let trees = home.join("trees");
        let ws = home.join("ws");
        std::fs::create_dir_all(trees.join("real/sub")).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        std::os::unix::fs::symlink(trees.join("real"), trees.join("link")).unwrap();
        std::os::unix::fs::symlink(trees.join("real"), ws.join("00aa")).unwrap();
        let same = [trees.clone()];
        let roots = Roots {
            ws: &ws,
            cache: &home.join("cache"),
            same_path: &same,
            cargo_git_db: &home.join(".cargo/git/db"),
        };
        let p = |l: &str| plan(l, &home, &roots);
        for bad in [trees.join("link"), trees.join("link/sub"), ws.join("00aa")] {
            let got = p(&format!("slot forget {}", bad.display()));
            assert!(
                got.as_ref().is_err_and(|e| e.contains("symbolic link")),
                "{}: {got:?}",
                bad.display()
            );
        }
        let real = trees.join("real/sub");
        assert_eq!(
            p(&format!("slot forget {}", real.display())),
            Ok(Plan::Forget(real))
        );
    }

    fn job(id: i64, dirs: &[&Path]) -> Busy {
        Busy {
            id,
            dirs: dirs.iter().map(|d| d.to_path_buf()).collect(),
        }
    }

    #[test]
    fn forget_removes_a_tree_no_job_uses_and_says_what_it_freed() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let ws = root.join("ws");
        let mirror = ws.join("00aa");
        std::fs::create_dir_all(mirror.join("src")).unwrap();
        std::fs::write(mirror.join("src/lib.rs"), vec![b'x'; 8192]).unwrap();
        let tree = root.join("trees/r/a");
        std::fs::create_dir_all(tree.join("crates/x")).unwrap();
        std::os::unix::fs::symlink(&tree, ws.join("00bb")).unwrap();

        let busy = [job(7, &[&mirror.join("src")])];
        let e = forget(&mirror, &ws, &busy).unwrap_err();
        assert!(e.contains("used by job 7"), "{e}");
        let busy = [job(8, &[&tree.join("crates/x")])];
        for path in [tree.clone(), tree.join("crates"), root.join("trees")] {
            let e = forget(&path, &ws, &busy).unwrap_err();
            assert!(e.contains("used by job 8"), "{}: {e}", path.display());
        }
        let declared = [job(9, &[&root.join("elsewhere"), &tree])];
        assert!(forget(&tree, &ws, &declared).is_err(), "a declared tree");
        let home_job = [job(10, &[&root])];
        assert!(
            forget(&mirror, &ws, &home_job).is_ok(),
            "a job above the tree does not hold it"
        );
        assert!(!mirror.exists());

        let lines = forget(&tree, &ws, &[]).unwrap();
        assert!(!tree.exists() && !ws.join("00bb").is_symlink());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with(&format!("removed {} (", tree.display())));
        assert!(lines[1].contains("00bb (its link)"), "{lines:?}");
        assert!(forget(&tree, &ws, &[]).is_err(), "nothing there");
    }

    #[test]
    fn the_jobs_a_state_answer_names_are_its_own_queued_and_running_ones() {
        let state = r#"{"active":[
            {"id":3,"cwd":"/w/a","extra_trees":["/w/specs"]},
            {"id":4,"cwd":"/x","from_node":true},
            {"id":5,"cwd":""}
        ],"done":[{"id":1,"cwd":"/w/old"}]}"#;
        assert_eq!(
            busy_of(state).unwrap(),
            [
                Busy {
                    id: 3,
                    dirs: vec![PathBuf::from("/w/a"), PathBuf::from("/w/specs")],
                },
                Busy {
                    id: 5,
                    dirs: vec![],
                },
            ]
        );
        assert!(busy_of("{}").is_err());
        assert!(busy_of("not json").is_err());
    }

    #[test]
    fn split_handles_quotes_and_refuses_operators() {
        assert_eq!(
            split(r#"rsync --server . 'a b/' "c d" e\ f"#).unwrap(),
            vec!["rsync", "--server", ".", "a b/", "c d", "e f"]
        );
        assert!(split("a 'b").is_err());
        assert!(split("a; b").is_err());
        assert!(split("a \"$HOME\"").is_err());
    }
}
