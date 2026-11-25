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
//! - `slot proxy`, `slot cancel <id>` and `slot __siblings <ws-root> <id>
//!   <key> [<tree>]`, run by this binary whatever path the client named; the
//!   workspace root must be this server's;
//! - `slot __cargo-git <entry>:<rev>…`, read-only: which entries of this
//!   account's cargo git database lack the revision (entry names only, no
//!   path; no git runs); and the copy of one such entry,
//!   `rsync --server … . ~/.cargo/git/db/<entry>/`, the git dependencies of
//!   a job (`git_deps.rs`). The database is always the one under the
//!   account's home, never the session's `CARGO_HOME`. That copy takes no
//!   option that keeps, follows or writes through a symbolic link
//!   (`-l -L -k -K -R --copy-unsafe-links --safe-links`), deletes
//!   (`--delete…`, `--force`) or writes in place (`--inplace`), and an
//!   entry that is a symbolic link is refused.
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
    /// Print the cargo git database and the wanted entries that lack their
    /// revision (`git_deps::node_answer`).
    CargoGit(Vec<crate::git_deps::Wanted>),
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
    // What a copy into the cargo git database may not carry.
    let mut shorts = String::new();
    let mut longs: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if dot {
            paths.push(a.as_str());
        } else if a == "." {
            dot = true;
        } else if a == "--copy-dest" || a.starts_with("--copy-dest=") {
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

/// rsync short options a copy into the cargo git database may not carry:
/// the ones that keep, follow or write through symbolic links, and
/// `--relative`.
const DB_SHORT_REFUSED: &str = "lLkKR";

/// rsync long options a copy into the cargo git database may not carry:
/// it deletes nothing, writes no file in place, and takes no link.
const DB_LONG_REFUSED: &[&str] = &[
    "--delete",
    "--delete-before",
    "--delete-during",
    "--delete-after",
    "--delete-delay",
    "--inplace",
    "--force",
    "--copy-unsafe-links",
    "--safe-links",
];

/// Whether a copy into the database entry `entry` with these options may
/// run: none of [`DB_SHORT_REFUSED`] or [`DB_LONG_REFUSED`], and the entry
/// is not a symbolic link.
fn db_copy(entry: &Path, shorts: &str, longs: &[String]) -> Result<(), String> {
    if let Some(c) = shorts.chars().find(|c| DB_SHORT_REFUSED.contains(*c)) {
        return Err(format!(
            "rsync option `-{c}` is not allowed into the cargo git database"
        ));
    }
    if let Some(l) = longs.iter().find(|l| DB_LONG_REFUSED.contains(&l.as_str())) {
        return Err(format!(
            "rsync option `{l}` is not allowed into the cargo git database"
        ));
    }
    if std::fs::symlink_metadata(entry).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("`{}` is a symbolic link", entry.display()));
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
