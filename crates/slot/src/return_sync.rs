//! The return sync of remote mode: after a node job, what the job wrote in
//! its tree comes back to the tree on this machine, and what it deleted
//! there is deleted here when this machine's copy is still the one the node
//! had.
//!
//! The node's tree is listed before the job (after the sync) and after it,
//! each time by an rsync with no destination, which `slot shell` admits as
//! a fetch: the times are the node's own, compared only with each other.
//! A path whose entry is new or changed comes back with
//! `rsync -a --checksum --update`, chosen by include rules the client sends
//! inside rsync's protocol (the node refuses `--files-from`); a file here
//! that is newer than the node's stays. Nothing under `.git`, `target` or
//! `node_modules` is listed, copied or deleted, and no path that a symbolic
//! link of this tree leads to, or that is one, is written.
//!
//! A regular file in the listing before the job and missing after it is
//! deleted here when this machine's file still has the size and
//! modification time it had when the sync sent it ([`Sent`]), or has the
//! size and modification time (to the second) of the node's file before the
//! job: a file that came back from the node. Any other such file is kept and
//! named on a line of its own. A directory the job removed is removed here
//! when it is empty after that.
//!
//! On by default; `SLOT_RETURN_SYNC=0` turns it off. A host wrapper in
//! front of `slot` that did its own return sync exports `SLOT_SHIM_PID`:
//! then the client leaves the return to it unless `SLOT_RETURN_SYNC=1`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::artifacts::Source;
use crate::sync::Bounds;

/// Turns the return sync off (`0`) or on (any other value).
pub const ENV: &str = "SLOT_RETURN_SYNC";

/// The mark of a host wrapper that does its own return sync.
pub const WRAPPER_ENV: &str = "SLOT_SHIM_PID";

/// What the listings and the copy leave out.
pub const EXCLUDES: [&str; 3] = [
    "--exclude=/.git/",
    "--exclude=target/",
    "--exclude=node_modules/",
];

/// Whether the client does the return sync: [`ENV`] set and not empty
/// decides (`0` off); unset, it is on unless [`WRAPPER_ENV`] marks a host
/// wrapper that does it.
pub fn wanted(switch: Option<&str>, wrapper: Option<&str>) -> bool {
    match switch.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => v != "0",
        None => wrapper.is_none_or(str::is_empty),
    }
}

/// A regular file's size and modification time on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    secs: i64,
    nanos: i64,
}

impl Stamp {
    /// The stamp of `m`, when it is a regular file's.
    pub fn of(m: &std::fs::Metadata) -> Option<Self> {
        m.file_type().is_file().then(|| Self {
            len: m.len(),
            secs: m.mtime(),
            nanos: m.mtime_nsec(),
        })
    }
}

/// The regular files a sync sent, by path relative to the tree, each with
/// its stamp when the sync listed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sent(HashMap<String, Stamp>);

impl Sent {
    /// Stamps each of `files` (relative to `top`) as it is now.
    pub fn record(top: &Path, files: impl IntoIterator<Item = String>) -> Self {
        Self(
            files
                .into_iter()
                .filter_map(|f| {
                    let m = top.join(&f).symlink_metadata().ok()?;
                    Some((f, Stamp::of(&m)?))
                })
                .collect(),
        )
    }

    /// How many files were stamped.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no file was stamped.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One entry of the node's tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Item {
    /// Path relative to the tree.
    pub path: String,
    /// The type letter of its mode: `-` a regular file, `d`, `l`, …
    pub kind: char,
    /// Bytes.
    pub size: u64,
    /// Modification time, seconds since the epoch.
    pub secs: i64,
    /// A link's target.
    pub link: Option<String>,
}

/// One line of a listing made with `TZ=UTC`:
/// `-rw-r--r--  1,234 2026/11/27 21:17:06 path`, a link with ` -> target`.
fn item(line: &str) -> Option<Item> {
    let mut rest = line;
    let mut field = || {
        let (f, r) = rest.trim_start_matches(' ').split_once(' ')?;
        rest = r;
        Some(f)
    };
    let (mode, size, date, time) = (field()?, field()?, field()?, field()?);
    let kind = mode.chars().next()?;
    let size = size
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    let secs = epoch(date, time)?;
    let (path, link) = match rest.split_once(" -> ") {
        Some((p, t)) if kind == 'l' => (p, Some(t.to_string())),
        _ => (rest, None),
    };
    (!path.is_empty()).then(|| Item {
        path: path.to_string(),
        kind,
        size,
        secs,
        link,
    })
}

/// `2026/11/27` and `21:17:06`, in UTC, as seconds since the epoch.
fn epoch(date: &str, time: &str) -> Option<i64> {
    let num = |s: &str| s.parse::<u16>().ok();
    let d: Vec<_> = date.split('/').map(num).collect::<Option<_>>()?;
    let t: Vec<_> = time.split(':').map(num).collect::<Option<_>>()?;
    let ([y, mo, da], [h, mi, se]) = (d.as_slice(), t.as_slice()) else {
        return None;
    };
    let month = time::Month::try_from(u8::try_from(*mo).ok()?).ok()?;
    let date =
        time::Date::from_calendar_date(i32::from(*y), month, u8::try_from(*da).ok()?).ok()?;
    let time = time::Time::from_hms(
        u8::try_from(*h).ok()?,
        u8::try_from(*mi).ok()?,
        u8::try_from(*se).ok()?,
    )
    .ok()?;
    Some(
        time::PrimitiveDateTime::new(date, time)
            .assume_utc()
            .unix_timestamp(),
    )
}

/// The entries of a listing, without the tree's own `.`; `None` when a
/// line does not read as one: a listing read in part would make a file
/// look deleted.
pub fn items(out: &str) -> Option<Vec<Item>> {
    out.lines()
        .filter(|l| !l.is_empty())
        .map(item)
        .filter(|i| i.as_ref().is_none_or(|i| i.path != "."))
        .collect()
}

/// rsync's exit status when a file vanished while it was listed.
const VANISHED: i32 = 24;

/// Lists the job's tree on the node, `node.target`, without the
/// [`EXCLUDES`], bounded like a sync; why it failed. rsync's own error
/// lines are not shown: a node that refuses the step costs the caller's
/// one line.
pub fn list(node: &Source<'_>, b: &Bounds) -> Result<Vec<Item>, String> {
    let mut rsync = Command::new("rsync");
    rsync
        .args(["-rl", "-8"])
        .args(EXCLUDES)
        .arg("-e")
        .arg(node.ssh.join(" "))
        .arg(format!(
            "{}:{}/",
            node.host,
            node.target.trim_end_matches('/')
        ))
        .env("TZ", "UTC")
        .stderr(Stdio::null());
    match crate::artifacts::bounded(rsync, b) {
        Ok((st, out)) if st.success() || st.code() == Some(VANISHED) => {
            items(&out).ok_or_else(|| "a line of the listing did not read".into())
        }
        Ok((st, _)) => Err(format!("rsync exited with {st}")),
        Err(e) => Err(format!("{e:#}")),
    }
}

/// What a job changed in its tree, from the listing `before` it and
/// `after` it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Entries new or changed after the job, each with whether it is a
    /// directory.
    pub copy: Vec<(String, bool)>,
    /// Regular files before the job that are missing after it.
    pub gone: Vec<Item>,
    /// Directories before the job that are missing after it, the deepest
    /// first.
    pub gone_dirs: Vec<String>,
}

/// The changes between two listings of a tree.
pub fn changes(before: &[Item], after: &[Item]) -> Changes {
    let had: HashSet<&Item> = before.iter().collect();
    let now: HashSet<&str> = after.iter().map(|i| i.path.as_str()).collect();
    let copy = after
        .iter()
        .filter(|i| !had.contains(i))
        .map(|i| (i.path.clone(), i.kind == 'd'))
        .collect();
    let gone = before
        .iter()
        .filter(|i| i.kind == '-' && !now.contains(i.path.as_str()))
        .cloned()
        .collect();
    let mut gone_dirs: Vec<String> = before
        .iter()
        .filter(|i| i.kind == 'd' && !now.contains(i.path.as_str()))
        .map(|i| i.path.clone())
        .collect();
    gone_dirs.sort_unstable_by(|a, b| b.cmp(a));
    Changes {
        copy,
        gone,
        gone_dirs,
    }
}

/// `p` as an rsync pattern that matches only itself: a backslash escapes a
/// wildcard character only when the pattern holds one.
fn literal(p: &str) -> String {
    if !p.contains(['[', '*', '?']) {
        return p.to_string();
    }
    let mut s = String::with_capacity(p.len() + 4);
    for c in p.chars() {
        if matches!(c, '[' | '*' | '?' | '\\') {
            s.push('\\');
        }
        s.push(c);
    }
    s
}

/// The filter rules that let exactly `paths` through, with the directories
/// on their way, and nothing else. A name with a line break cannot be said
/// in a rule and is left out.
pub fn rules(paths: &[(String, bool)]) -> String {
    let mut lines = BTreeSet::new();
    for (p, dir) in paths {
        if p.contains(['\n', '\r']) {
            continue;
        }
        let mut at = 0;
        while let Some(i) = p[at..].find('/') {
            at += i;
            lines.insert(format!("+ /{}/", literal(&p[..at])));
            at += 1;
        }
        lines.insert(format!("+ /{}{}", literal(p), if *dir { "/" } else { "" }));
    }
    let mut s: String = lines.into_iter().flat_map(|l| [l, "\n".into()]).collect();
    s.push_str("- *\n");
    s
}

/// Whether `rel` names a path strictly inside `top` that no symbolic link
/// of this machine leads to: only plain names, and no directory on the way
/// is a link.
pub fn inside(top: &Path, rel: &str) -> bool {
    let p = Path::new(rel);
    let parts: Vec<Component<'_>> = p.components().collect();
    if parts.is_empty() || !parts.iter().all(|c| matches!(c, Component::Normal(_))) {
        return false;
    }
    let mut cur = top.to_path_buf();
    for c in &parts[..parts.len() - 1] {
        cur.push(c);
        if cur
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return false;
        }
    }
    true
}

/// Whether `top/rel` is a symbolic link here.
fn is_link(top: &Path, rel: &str) -> bool {
    top.join(rel)
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
}

/// Copies what `rules` lets through from the tree on the node into `top`;
/// the count of regular files received.
fn copy(node: &Source<'_>, top: &Path, rules: &str, b: &Bounds) -> Result<usize, String> {
    let mut rsync = Command::new("rsync");
    rsync
        .args([
            "-a",
            "--checksum",
            "--update",
            "--omit-dir-times",
            "-8",
            "--out-format=%i|%n",
        ])
        .args(EXCLUDES)
        .arg("--filter=merge -")
        .arg("-e")
        .arg(node.ssh.join(" "))
        .arg(format!(
            "{}:{}/",
            node.host,
            node.target.trim_end_matches('/')
        ))
        .arg(format!("{}/", top.display()))
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = rsync.spawn().map_err(|e| format!("rsync: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("rsync stdin")?;
    let rules = rules.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(rules.as_bytes());
    });
    let read = |r: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut r) = r {
                let _ = r.read_to_string(&mut s);
            }
            s
        })
    };
    let out = read(child.stdout.take().map(|o| Box::new(o) as _));
    let err = read(child.stderr.take().map(|o| Box::new(o) as _));
    let st = crate::sync::watch(&mut child, Some(b.stall), Instant::now() + b.limit, b.limit);
    let _ = writer.join();
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    let st = st.map_err(|stop| format!("rsync killed: {stop}"))?;
    let received = out.lines().filter(|l| l.starts_with(">f")).count();
    if st.success() || st.code() == Some(VANISHED) {
        Ok(received)
    } else {
        let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        Err(format!("rsync exited with {st}: {first}"))
    }
}

/// What the deletions did: the count deleted, and each path kept with why.
fn delete(top: &Path, ch: &Changes, sent: &Sent) -> (usize, Vec<(String, String)>) {
    let mut deleted = 0;
    let mut kept = Vec::new();
    for item in &ch.gone {
        let rel = item.path.as_str();
        if !inside(top, rel) {
            kept.push((rel.to_string(), "a symbolic link here leads to it".into()));
            continue;
        }
        let path = top.join(rel);
        let Ok(m) = path.symlink_metadata() else {
            continue;
        };
        let Some(now) = Stamp::of(&m) else {
            kept.push((rel.to_string(), "not a regular file here".into()));
            continue;
        };
        let recorded = sent.0.get(rel);
        let was_sent = recorded == Some(&now);
        let came_back = m.len() == item.size && m.mtime() == item.secs;
        if was_sent || came_back {
            match std::fs::remove_file(&path) {
                Ok(()) => deleted += 1,
                Err(e) => kept.push((rel.to_string(), format!("could not delete it: {e}"))),
            }
        } else if recorded.is_some() {
            kept.push((rel.to_string(), "changed here since the sync".into()));
        } else {
            kept.push((
                rel.to_string(),
                "not sent from here, and not the node's copy".into(),
            ));
        }
    }
    for rel in &ch.gone_dirs {
        if !inside(top, rel) {
            continue;
        }
        let path = top.join(rel);
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_dir())
            && std::fs::remove_dir(&path).is_ok()
        {
            deleted += 1;
        }
    }
    (deleted, kept)
}

/// The return sync after a job on `node.host` whose tree there is
/// `node.target` and here `top`: `before` is the listing made before the
/// job, `sent` what the sync sent. Prints a line per file kept and one
/// line of counts, or one line saying why it did not run.
pub fn back(
    node: &Source<'_>,
    top: &Path,
    before: &Result<Vec<Item>, String>,
    sent: &Sent,
    b: &Bounds,
) {
    let host = node.host;
    let before = match before {
        Ok(v) => v,
        Err(why) => {
            eprintln!(
                "slot: return sync skipped: could not list the tree on {host} before the job ({why})"
            );
            return;
        }
    };
    let after = match list(node, b) {
        Ok(v) => v,
        Err(why) => {
            eprintln!(
                "slot: return sync skipped: could not list the tree on {host} after the job ({why})"
            );
            return;
        }
    };
    let ch = changes(before, &after);
    let wanted: Vec<(String, bool)> = ch
        .copy
        .iter()
        .filter(|(p, _)| inside(top, p) && !is_link(top, p))
        .cloned()
        .collect();
    let back = if wanted.is_empty() {
        0
    } else {
        match copy(node, top, &rules(&wanted), b) {
            Ok(n) => n,
            Err(why) => {
                eprintln!("slot: return sync: the copy from {host} failed: {why}");
                0
            }
        }
    };
    let (deleted, kept) = delete(top, &ch, sent);
    for (path, why) in &kept {
        eprintln!("slot: return sync: kept {path}: {why}");
    }
    eprintln!(
        "slot: return sync from {host}: {back} back, {deleted} deleted, {} kept",
        kept.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(path: &str, kind: char, size: u64, secs: i64) -> Item {
        Item {
            path: path.into(),
            kind,
            size,
            secs,
            link: None,
        }
    }

    #[test]
    fn the_switch_decides_and_a_wrapper_takes_the_default() {
        assert!(wanted(None, None));
        assert!(wanted(Some(""), Some("")));
        assert!(!wanted(Some("0"), None));
        assert!(!wanted(None, Some("4242")));
        assert!(wanted(Some("1"), Some("4242")));
        assert!(!wanted(Some("0"), Some("4242")));
    }

    #[test]
    fn a_listing_line_reads_as_an_entry_in_utc() {
        let l = "-rw-r--r--          1,234 2026/11/27 21:17:06 out/a b.txt";
        assert_eq!(item(l), Some(it("out/a b.txt", '-', 1234, 1_795_814_226)));
        let l = "lrwxrwxrwx              5 1970/01/01 00:00:01 link -> a.txt";
        let i = item(l).unwrap();
        assert_eq!((i.path.as_str(), i.kind, i.secs), ("link", 'l', 1));
        assert_eq!(i.link.as_deref(), Some("a.txt"));
        assert_eq!(item("not a listing line"), None);
        let out = "drwxr-xr-x  4,096 2026/11/27 21:17:06 .\n\
                   drwxr-xr-x  4,096 2026/11/27 21:17:06 sub\n";
        assert_eq!(items(out).unwrap().len(), 1);
        assert_eq!(items("drwx 4 2026/11/27 21:17:06 .\ngarbage\n"), None);
    }

    #[test]
    fn new_and_changed_entries_come_back_and_missing_ones_are_gone() {
        let before = [
            it("a", '-', 1, 10),
            it("b", '-', 1, 10),
            it("d", 'd', 0, 10),
            it("d/e", 'd', 0, 10),
            it("d/e/f", '-', 1, 10),
            it("l", 'l', 1, 10),
        ];
        let after = [
            it("a", '-', 2, 10),
            it("n", 'd', 0, 11),
            it("n/x", '-', 3, 11),
        ];
        let ch = changes(&before, &after);
        assert_eq!(
            ch.copy,
            [
                ("a".to_string(), false),
                ("n".to_string(), true),
                ("n/x".to_string(), false)
            ]
        );
        let gone: Vec<&str> = ch.gone.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(gone, ["b", "d/e/f"]);
        assert_eq!(ch.gone_dirs, ["d/e", "d"]);
    }

    #[test]
    fn the_rules_let_through_each_path_and_its_directories_only() {
        let r = rules(&[
            ("a/b/c.txt".into(), false),
            ("a/new".into(), true),
            ("x [1]*.txt".into(), false),
            ("back\\slash".into(), false),
            ("line\nbreak".into(), false),
        ]);
        assert_eq!(
            r,
            "+ /a/\n+ /a/b/\n+ /a/b/c.txt\n+ /a/new/\n+ /back\\slash\n+ /x \\[1]\\*.txt\n- *\n"
        );
    }

    #[test]
    fn a_path_through_a_link_or_out_of_the_tree_is_not_inside() {
        let t = tempfile::tempdir().unwrap();
        let top = t.path();
        std::fs::create_dir(top.join("real")).unwrap();
        std::os::unix::fs::symlink("real", top.join("via")).unwrap();
        assert!(inside(top, "real/f"));
        assert!(inside(top, "via"));
        assert!(!inside(top, "via/f"));
        assert!(!inside(top, "../f"));
        assert!(!inside(top, "/etc/passwd"));
        assert!(!inside(top, "a/../../f"));
        assert!(!inside(top, ""));
    }

    #[test]
    fn a_file_is_deleted_only_as_it_was_sent_or_came_back() {
        let t = tempfile::tempdir().unwrap();
        let top = t.path();
        for f in ["sent", "changed", "came", "other", "real/f"] {
            let p = top.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "1234").unwrap();
        }
        std::os::unix::fs::symlink("real", top.join("via")).unwrap();
        std::fs::create_dir(top.join("emptied")).unwrap();
        std::fs::create_dir(top.join("full")).unwrap();
        std::fs::write(top.join("full/mine"), "x").unwrap();
        let sent = Sent::record(top, ["sent".into(), "changed".into()]);
        assert_eq!(sent.len(), 2);
        std::fs::write(top.join("changed"), "12345").unwrap();
        let m = top.join("came").symlink_metadata().unwrap();
        let ch = Changes {
            copy: Vec::new(),
            gone: vec![
                it("sent", '-', 99, 0),
                it("changed", '-', 99, 0),
                it("came", '-', m.len(), m.mtime()),
                it("other", '-', 99, 0),
                it("via/f", '-', 4, 0),
                it("absent", '-', 4, 0),
            ],
            gone_dirs: vec!["full".into(), "emptied".into()],
        };
        let (deleted, kept) = delete(top, &ch, &sent);
        assert_eq!(deleted, 3);
        assert!(!top.join("sent").exists());
        assert!(!top.join("came").exists());
        assert!(!top.join("emptied").exists());
        assert!(top.join("full/mine").exists());
        assert!(top.join("real/f").exists());
        let kept: Vec<&str> = kept.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(kept, ["changed", "other", "via/f"]);
    }
}
