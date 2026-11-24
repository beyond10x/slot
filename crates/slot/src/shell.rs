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
//! - a fetch: `rsync --server --sender … . <path>` under the workspace root
//!   or the cache root's `target/`;
//! - `slot proxy`, `slot cancel <id>` and `slot __siblings <ws-root> <id>
//!   <key>`, run by this binary whatever path the client named; the
//!   workspace root must be this server's.
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
    },
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
/// does), `ws_root` the daemon's workspace root and `cache_root` its cache
/// root.
pub fn plan(line: &str, home: &Path, ws_root: &Path, cache_root: &Path) -> Result<Plan, String> {
    let words = split(line)?;
    let Some((prog, args)) = words.split_first() else {
        return Err("no command: this key runs only slot's sync and build steps".into());
    };
    let base = Path::new(prog)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match base.as_str() {
        "rsync" => rsync(args, home, ws_root, cache_root).map(Plan::Rsync),
        "slot" => match args {
            [p] if p == "proxy" => Ok(Plan::Proxy),
            [c, id] if c == "cancel" => id
                .parse()
                .map(Plan::Cancel)
                .map_err(|_| format!("bad job id `{id}`")),
            [c, root, id, key] if c == "__siblings" => {
                if absolute(home, root)? != absolute(home, &ws_root.to_string_lossy())? {
                    return Err(format!("`{root}` is not this server's workspace root"));
                }
                if !crate::mirror::valid_id(id) || !crate::mirror::valid_id(key) {
                    return Err("bad workspace id or repository key".into());
                }
                Ok(Plan::Siblings {
                    id: id.clone(),
                    key: key.clone(),
                })
            }
            _ => Err(format!("`slot {}` is not a remote step", args.join(" "))),
        },
        _ => Err(format!("`{prog}` is not a remote step")),
    }
}

fn rsync(
    args: &[String],
    home: &Path,
    ws_root: &Path,
    cache_root: &Path,
) -> Result<Vec<String>, String> {
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
            let sibling = value
                .and_then(|v| v.strip_prefix("../"))
                .map(|v| v.strip_suffix('/').unwrap_or(v));
            if sender || !sibling.is_some_and(crate::mirror::valid_id) {
                return Err(format!(
                    "rsync `--copy-dest` takes only `../<id>` of a sync, not `{}`",
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
    let [path] = paths.as_slice() else {
        return Err("rsync must name one path".into());
    };
    let p = absolute(home, path)?;
    let ws = absolute(home, &ws_root.to_string_lossy())?;
    let target = absolute(home, &cache_root.join("target").to_string_lossy())?;
    let under = |root: &Path| p.starts_with(root) && p != root;
    if !(under(&ws) || (sender && under(&target))) {
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
        plan(
            line,
            Path::new("/srv/u"),
            Path::new("/srv/u/.local/state/slot/ws"),
            Path::new("/srv/u/.local/state/slot/cache"),
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
