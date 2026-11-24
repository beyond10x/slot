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
//! - `slot proxy`, `slot cancel <id>` and `slot __siblings <ws-root> <id>
//!   <key> [<tree>]`, run by this binary whatever path the client named; the
//!   workspace root must be this server's.
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
    /// Record mirror `id` under repository `key` and print its siblings.
    Siblings {
        /// Workspace id.
        id: String,
        /// Repository key.
        key: String,
        /// The tree of a same-path mirror: `<ws-root>/<id>` links to it.
        path: Option<PathBuf>,
    },
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
const RSYNC_LONG_VALUE: &[&str] = &["--compress-level", "--compress-choice", "--checksum-choice"];

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
    for path in paths {
        let p = absolute(home, path)?;
        let under = |root: &Path| p.starts_with(root) && p != root;
        let same = Path::new(path).is_absolute() && under_any(&p, home, roots.same_path)?;
        if !(under(&ws) || same || (sender && under(&target))) {
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
            },
        )
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
        assert!(p("slot cancel x").is_err());
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
