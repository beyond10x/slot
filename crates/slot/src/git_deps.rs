//! The git dependencies of a job's checkout reach the node with the job.
//!
//! A cargo command on a node fetches the `git+` sources its `Cargo.lock`
//! names from the node, which may have no way to them: a private forge it
//! holds no credential for, or no network to it. The node's fetch-once step
//! (`cargo_fetch.rs`) runs there too, and only for a cargo command the
//! client sends itself, not for a wrapper's inner cargo call.
//!
//! So before a job is placed on a node the client reads every `Cargo.lock`
//! of the checkout (the ones the sync sends) and takes the `git+` sources
//! and their locked revisions. `cargo metadata --offline` in the lock
//! file's directory names the database entry cargo keeps each source in
//! (`$CARGO_HOME/git/db/<name>-<hash>`, from its checkout's path); when one
//! is missing, or does not hold the revision, `cargo fetch --locked` runs
//! here first, with this machine's credentials and outside the pool. Only
//! that entry of each source counts: another entry of the same name (a fork
//! that holds the same commit) never does. The client asks the node which
//! of those entries lack their revision (`slot __cargo-git`), takes only
//! answers it asked for, and copies those entries' objects, refs and
//! `HEAD` into the node account's `~/.cargo/git/db`, over the same ssh and
//! rsync as the sync. cargo on the node then finds the locked revision and
//! does not fetch. No `config`, hook, `info/` or `FETCH_HEAD` crosses.
//!
//! A source on a remote deny list keeps the job on this machine with the
//! reason `git dependency denied: <source>`. The lists are the never-sync
//! file (`never_sync.rs`, `remote-deny`) and `remote-deny-origins` in
//! slot's configuration directory; either may hold either kind of line:
//! `/abs/path`, which denies a `git+file` source under that prefix, and
//! `origin:<pattern>`, a shell pattern (`case` semantics, without regard to
//! case) matched against the URL cargo fetches in every form the check
//! knows: as written, normalised (`normalise`), bare (`bare`: host and
//! path, without user, port, trailing `/` or `.git`), as git rewrites it
//! (`url.<base>.insteadOf`) and with an ssh host alias resolved. A line of
//! another kind, a list or lock file that cannot be read, a source the
//! check cannot take apart, a git or ssh lookup that fails or runs over
//! its bound, no absolute cargo home, or a cargo configuration that
//! replaces a git source keeps every job with a lock file here while a
//! list names anything. The copy checks the sources again before each
//! entry.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::io::Read as _;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::never_sync;
use crate::sync;

/// File name of the origin deny list in slot's configuration directory.
pub const ORIGINS_FILE: &str = "remote-deny-origins";

/// Start of the reason of a job a denied source keeps here.
pub const REASON_PREFIX: &str = "git dependency denied: ";

/// The node's cargo git database, relative to its account's home. Never
/// the session's `CARGO_HOME`: the caller must not pick where it writes.
pub const NODE_DB: &str = ".cargo/git/db";

/// Longest the fetch here, the probe and the copies may take together.
pub const LIMIT: Duration = Duration::from_secs(300);

/// Longest one git or ssh lookup of the deny check may take.
const LOOKUP: Duration = Duration::from_secs(10);

/// One `git+` source of a `Cargo.lock`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Source {
    /// The source without its revision: `git+https://host/org/repo?branch=x`.
    pub url: String,
    /// The locked commit id.
    pub rev: String,
    /// The name cargo gives its database entries: the URL's last path
    /// segment without `.git` (`<name>-<hash>`).
    pub name: String,
}

impl Source {
    /// The `source` value of a lock file's package, when it is `git+` with
    /// a URL and a full revision.
    pub fn parse(source: &str) -> Option<Self> {
        let rest = source.strip_prefix("git+")?;
        let (url, rev) = source.split_once('#')?;
        if !valid_rev(rev) {
            return None;
        }
        let (_, _, path) = url_parts(rest.split_once('#').map_or(rest, |(u, _)| u))?;
        let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        let last = last.strip_suffix(".git").unwrap_or(last);
        Some(Self {
            url: url.to_string(),
            rev: rev.to_string(),
            name: if last.is_empty() {
                "_empty".into()
            } else {
                last.to_string()
            },
        })
    }
}

/// A full commit id: 40 or 64 hex digits.
pub fn valid_rev(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A database entry name as cargo makes it: `<name>-<hash>`, letters,
/// digits, `.`, `_` and `-`, not starting with `.` or `-`.
pub fn valid_entry(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with(['.', '-'])
        && name.contains('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `(scheme, host, path)` of a URL without `git+`, query or fragment: the
/// host lowercased, without user and port; the path without its leading
/// `/`. A `file` URL has an empty host and an absolute path.
fn url_parts(url: &str) -> Option<(String, String, String)> {
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme == "file" {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        return path
            .starts_with('/')
            .then(|| (scheme, String::new(), path.to_string()));
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host.starts_with('[') {
        host.split_once(']')
            .map_or(host, |(h, _)| h)
            .trim_start_matches('[')
    } else {
        host.split_once(':').map_or(host, |(h, _)| h)
    };
    Some((scheme, host.to_ascii_lowercase(), path.to_string()))
}

/// The `git+` sources of a lock file's text, each once. An error when the
/// text does not parse, or a `git+` source has no URL, no `#` or no full
/// revision.
pub fn parse_lock(text: &str) -> Result<Vec<Source>, String> {
    let table = toml::from_str::<toml::Table>(text).map_err(|e| {
        format!(
            "does not parse ({})",
            e.to_string().lines().next().unwrap_or("").trim()
        )
    })?;
    let mut out = Vec::new();
    let packages = table
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten();
    for p in packages {
        let Some(s) = p.get("source").and_then(toml::Value::as_str) else {
            continue;
        };
        if s.starts_with("git+") {
            out.push(Source::parse(s).ok_or_else(|| {
                format!("git source `{s}` has no URL, no `#` or no full revision")
            })?);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// The lock files of the checkout at `top` that a sync sends (tracked, or
/// untracked and not ignored); without git, the nearest one from `cwd` up
/// to `top`.
pub fn lock_files(top: &Path, cwd: &Path) -> Vec<PathBuf> {
    let out = Command::new("git")
        .env("LC_ALL", "C")
        .arg("-C")
        .arg(top)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "Cargo.lock",
            "**/Cargo.lock",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success());
    let Some(out) = out else {
        return crate::cargo_fetch::lock_dir(cwd, top)
            .map(|d| d.join("Cargo.lock"))
            .into_iter()
            .collect();
    };
    let mut files: Vec<PathBuf> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| top.join(String::from_utf8_lossy(p).as_ref()))
        .filter(|p| p.exists())
        .collect();
    files.sort();
    files.dedup();
    files
}

/// What the lock files of a checkout name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Locks {
    /// The `git+` sources of each lock file, by the directory that holds
    /// it; files without one are left out.
    pub found: Vec<(PathBuf, Vec<Source>)>,
    /// A lock file that could not be read or parsed, or a git source that
    /// could not be taken apart, with why.
    pub problems: Vec<String>,
    /// The directory of every lock file read, with git sources or without.
    pub dirs: Vec<PathBuf>,
    /// The cargo configuration files cargo reads for those lock files:
    /// `.cargo/config.toml` and `.cargo/config` in each lock file's
    /// directory and every directory above it, and in any directory of the
    /// checkout the sync sends (`config_files`). `$CARGO_HOME`'s are added
    /// by the check.
    pub configs: Vec<PathBuf>,
}

/// Reads the lock files `files`.
pub fn locked(files: &[PathBuf]) -> Locks {
    let mut l = Locks::default();
    for f in files {
        let text = match std::fs::read_to_string(f) {
            Ok(t) => t,
            Err(e) => {
                l.problems
                    .push(format!("lock file {} unreadable ({e})", f.display()));
                continue;
            }
        };
        let dir = f.parent().map(Path::to_path_buf).unwrap_or_default();
        l.dirs.push(dir.clone());
        match parse_lock(&text) {
            Ok(sources) if sources.is_empty() => {}
            Ok(sources) => l.found.push((dir, sources)),
            Err(e) => l.problems.push(format!("lock file {}: {e}", f.display())),
        }
    }
    l
}

/// The lock files of the checkout at `top` (`lock_files`), read, with the
/// cargo configuration files that apply to them (`config_files`).
pub fn checkout(top: &Path, cwd: &Path) -> Locks {
    let mut l = locked(&lock_files(top, cwd));
    l.configs = config_files(top, &l.dirs);
    l
}

/// The cargo configuration files that exist for lock files in `dirs` of the
/// checkout at `top`: `.cargo/config.toml` and `.cargo/config` in each of
/// `dirs` and every directory above it, and those the sync sends from any
/// directory of the checkout (cargo reads them from where it runs, which a
/// wrapper may choose).
pub fn config_files(top: &Path, dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|d| d.ancestors())
        .flat_map(|a| [a.join(".cargo/config.toml"), a.join(".cargo/config")])
        .filter(|p| p.is_file())
        .collect();
    let listed = Command::new("git")
        .env("LC_ALL", "C")
        .arg("-C")
        .arg(top)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ".cargo/config.toml",
            ".cargo/config",
            "**/.cargo/config.toml",
            "**/.cargo/config",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success());
    if let Some(o) = listed {
        out.extend(
            o.stdout
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| top.join(String::from_utf8_lossy(p).as_ref()))
                .filter(|p| p.is_file()),
        );
    }
    out.sort();
    out.dedup();
    out
}

/// Why the cargo configuration files `files` keep a job here: one of them
/// does not parse, or a `[source.<name>]` table replaces a git source or
/// names a git source as the replacement (`replace-with`, the tables merged
/// over every file). With a git source replaced, `cargo metadata` names the
/// replacement's checkout for the source the lock file names, so the entry
/// copied would be another repository's.
pub fn git_source_replaced(files: &[PathBuf]) -> Option<String> {
    // Each source name: whether a file gives it a `git` key, and every
    // `replace-with` a file gives it.
    let mut sources: std::collections::BTreeMap<String, (bool, Vec<String>)> =
        std::collections::BTreeMap::new();
    for f in files {
        let text = match std::fs::read_to_string(f) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Some(format!("cargo config {} unreadable ({e})", f.display())),
        };
        let Ok(table) = toml::from_str::<toml::Table>(&text) else {
            return Some(format!("cargo config {} does not parse", f.display()));
        };
        if table.contains_key("include") {
            return Some(format!(
                "cargo config {} has `include`; the files it names are not read",
                f.display()
            ));
        }
        let Some(source) = table.get("source") else {
            continue;
        };
        let Some(source) = source.as_table() else {
            return Some(format!(
                "cargo config {}: `source` is no table",
                f.display()
            ));
        };
        for (name, v) in source {
            let e = sources.entry(name.clone()).or_default();
            let Some(t) = v.as_table() else {
                continue;
            };
            e.0 |= t.contains_key("git");
            e.1.extend(
                t.get("replace-with")
                    .and_then(toml::Value::as_str)
                    .map(str::to_string),
            );
        }
    }
    sources.iter().find_map(|(name, (git, with))| {
        with.iter().find_map(|w| {
            let target_git = sources.get(w).is_some_and(|(g, _)| *g);
            (*git || target_git)
                .then(|| format!("cargo config replaces source `{name}` with `{w}`, a git source"))
        })
    })
}

/// The URL cargo fetches for a source: without `git+`, its `?query` and
/// its `#fragment` (`git+https://h/g/r.git?branch=x#<rev>` is
/// `https://h/g/r.git`).
pub fn fetch_url(source: &str) -> &str {
    let url = source.strip_prefix("git+").unwrap_or(source);
    url.split(['?', '#']).next().unwrap_or(url)
}

/// `%XX` decoded; `None` for a broken escape or bytes that are no UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `path` with every run of `/` made one.
fn collapse(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if !(c == '/' && out.ends_with('/')) {
            out.push(c);
        }
    }
    out
}

/// A URL (without `git+`, query or fragment) in one form: scheme and host
/// lowercased, a host's trailing `.` and a scheme's default port dropped,
/// the path percent-decoded with runs of `/` made one. `None` when it
/// cannot be taken apart: no `scheme://`, no host, a port that is no
/// number, a broken escape.
pub fn normalise(url: &str) -> Option<String> {
    // git's scp-like form, `user@host:path` or `[user@host:port]:path`, as
    // the ssh URL it stands for.
    let scp;
    let bracket = url
        .strip_prefix('[')
        .and_then(|r| r.split_once("]:"))
        .filter(|(a, _)| !url.contains("://") && !a.is_empty() && !a.contains('/'));
    let url = match (bracket, url.split_once(':')) {
        (Some((a, p)), _) => {
            scp = format!("ssh://{a}/{}", p.trim_start_matches('/'));
            scp.as_str()
        }
        (None, Some((a, p))) if !url.contains("://") && !a.is_empty() && !a.contains('/') => {
            scp = format!("ssh://{a}/{}", p.trim_start_matches('/'));
            scp.as_str()
        }
        _ => url,
    };
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return None;
    }
    if scheme == "file" {
        let path = if rest.len() >= 9 && rest[..9].eq_ignore_ascii_case("localhost") {
            &rest[9..]
        } else {
            rest
        };
        if !path.starts_with('/') {
            return None;
        }
        return Some(format!("file://{}", collapse(&percent_decode(path)?)));
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (user, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
        let (h, after) = v6.split_once(']')?;
        (format!("[{h}]"), after.strip_prefix(':').unwrap_or(after))
    } else {
        let (h, p) = hostport.split_once(':').unwrap_or((hostport, ""));
        (h.to_string(), p)
    };
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    if host.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let default = match scheme.as_str() {
        "http" => "80",
        "https" => "443",
        "ssh" | "git+ssh" | "ssh+git" => "22",
        "git" => "9418",
        _ => "",
    };
    let port = port.trim_start_matches('0');
    let port = if port.is_empty() || port == default {
        String::new()
    } else {
        format!(":{port}")
    };
    let user = user.map(|u| format!("{u}@")).unwrap_or_default();
    let path = collapse(&format!("/{}", percent_decode(path)?));
    Some(format!("{scheme}://{user}{host}{port}{path}"))
}

/// Whether `text` as a whole matches the shell pattern `pattern`, as a
/// POSIX shell's `case` matches it: `*` any run of characters, `/`
/// included; `?` one character; `[...]` one character of a class, with
/// ranges (`a-z`) and `!` or `^` first for its complement; `\` takes the
/// next character as it is. A `[` without its `]` is itself.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    // The last `*`: the pattern index after it and the text index it took up to.
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        let step = match p.get(pi) {
            Some('*') => {
                star = Some((pi + 1, ti));
                pi += 1;
                continue;
            }
            Some('?') => Some(1),
            Some('[') => {
                let (hit, len) = class(&p[pi..], t[ti]);
                hit.then_some(len)
            }
            Some('\\') if pi + 1 < p.len() => (p[pi + 1] == t[ti]).then_some(2),
            Some(c) => (*c == t[ti]).then_some(1),
            None => None,
        };
        match step {
            Some(len) => {
                pi += len;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Matches `c` against the class that `p` starts with (`p[0]` is `[`):
/// `(matched, length of the class in p)`; a `[` without `]` is itself.
fn class(p: &[char], c: char) -> (bool, usize) {
    class_of(p, c).unwrap_or((c == '[', 1))
}

fn class_of(p: &[char], c: char) -> Option<(bool, usize)> {
    let mut i = 1;
    let negate = matches!(p.get(i), Some('!' | '^'));
    if negate {
        i += 1;
    }
    let start = i;
    let mut hit = false;
    loop {
        let x = *p.get(i)?;
        if x == ']' && i > start {
            return Some((hit != negate, i + 1));
        }
        if p.get(i + 1) == Some(&'-') && p.get(i + 2).is_some_and(|e| *e != ']') {
            let end = p[i + 2];
            hit |= x <= c && c <= end;
            i += 3;
        } else {
            hit |= x == c;
            i += 1;
        }
    }
}

/// One line of a deny list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    /// Empty, or a comment.
    Nothing,
    /// `/abs/path`.
    Path(String),
    /// `origin:<pattern>`.
    Origin(String),
    /// Anything else, with why.
    Bad(&'static str),
}

/// `line` without a comment: from a `#` that starts the line or follows
/// white space.
fn uncommented(line: &str) -> &str {
    let t = line.trim();
    if t.starts_with('#') {
        return "";
    }
    let cut = t
        .char_indices()
        .find(|&(i, c)| c == '#' && t[..i].ends_with(char::is_whitespace))
        .map_or(t.len(), |(i, _)| i);
    t[..cut].trim()
}

/// The pattern of an `origin` line (`origin:`, any case, white space
/// around the `:`), empty when it names none; `None` for another line.
pub fn origin_pattern(line: &str) -> Option<&str> {
    let (key, rest) = uncommented(line).split_once(':')?;
    key.trim()
        .eq_ignore_ascii_case("origin")
        .then(|| rest.trim())
}

fn rule(line: &str) -> Rule {
    let t = uncommented(line);
    if t.is_empty() {
        return Rule::Nothing;
    }
    if let Some(p) = origin_pattern(line) {
        return if p.is_empty() {
            Rule::Bad("an origin line without a pattern")
        } else {
            Rule::Origin(p.to_string())
        };
    }
    if t.starts_with('/') {
        Rule::Path(t.to_string())
    } else {
        Rule::Bad("neither `/abs/path` nor `origin:<pattern>`")
    }
}

/// The remote deny lists as the git dependencies read them. Each file
/// (`remote-deny`, `remote-deny-origins`) may hold either kind of line:
/// `/abs/path`, a path prefix, and `origin:<pattern>`, a shell pattern
/// matched against a remote URL. Any other line that is not empty or a
/// comment is a problem that keeps every job here.
#[derive(Debug, Clone, Default)]
pub struct DenyLists {
    paths: never_sync::List,
    origins: Vec<String>,
    problem: Option<String>,
}

impl DenyLists {
    /// Parses the texts of the list files.
    pub fn parse<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        Self::parse_named(texts.into_iter().map(|t| ("list", t)))
    }

    /// Parses `(name, text)` of each list file; a problem names the file
    /// and the line.
    fn parse_named<'a>(texts: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut paths = String::new();
        let mut origins = Vec::new();
        let mut problem = None;
        for (name, text) in texts {
            for (i, line) in text.lines().enumerate() {
                match rule(line) {
                    Rule::Nothing => {}
                    Rule::Path(p) => {
                        paths.push_str(&p);
                        paths.push('\n');
                    }
                    Rule::Origin(p) => origins.push(p),
                    Rule::Bad(why) => {
                        problem.get_or_insert_with(|| {
                            format!("{name} line {}: {why}: `{}`", i + 1, line.trim())
                        });
                    }
                }
            }
        }
        Self {
            paths: never_sync::List::parse(&paths).0,
            origins,
            problem,
        }
    }

    /// Reads the files at `files`: a missing file is an empty list.
    pub fn load(files: &[PathBuf]) -> Result<Self, String> {
        let mut texts = Vec::new();
        for f in files {
            match std::fs::read_to_string(f) {
                Ok(t) => texts.push((f.display().to_string(), t)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", f.display())),
            }
        }
        Ok(Self::parse_named(
            texts.iter().map(|(n, t)| (n.as_str(), t.as_str())),
        ))
    }

    /// Why the lists cannot be used, if they cannot.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Whether the lists name nothing and have no problem.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.origins.is_empty() && self.problem.is_none()
    }

    /// Whether one of `forms` of a URL is denied: an `origin:` pattern
    /// matches it (without regard to case), or it is a `file` URL whose
    /// path lies under a listed prefix.
    fn denies_forms(&self, forms: &[String]) -> bool {
        forms.iter().any(|f| {
            let lower = f.to_lowercase();
            self.origins
                .iter()
                .any(|p| glob_match(&p.to_lowercase(), &lower))
                || f.strip_prefix("file://")
                    .is_some_and(|p| self.paths.denies(Path::new(p)).is_some())
        })
    }

    /// Whether `s` is denied by the forms of its URL alone, without git's
    /// rewrites: also when the URL cannot be taken apart.
    pub fn denies(&self, s: &Source) -> bool {
        match forms(fetch_url(&s.url), &[]) {
            Some(f) => self.denies_forms(&f),
            None => true,
        }
    }
}

/// The forms of `url` the deny check compares: as written, normalised,
/// its bare forms (`bare`), and the same for each of `more` (the URL git
/// rewrites it to, an ssh alias resolved). `None` when `url` or one of
/// `more` cannot be normalised.
pub fn forms(url: &str, more: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for u in std::iter::once(url).chain(more.iter().map(String::as_str)) {
        out.push(u.to_string());
        let n = normalise(u)?;
        out.extend(bare(&n));
        out.push(n);
    }
    out.sort();
    out.dedup();
    Some(out)
}

/// The bare forms of the normalised URL `n`: without user and port, the
/// path without trailing `/` and `.git`, as `scheme://host/path` for its
/// own scheme and every network scheme git fetches over, and as
/// `host/path`; each also with `.git`, `/` and `.git/` after it, so a
/// pattern written with or without them matches.
fn bare(n: &str) -> Vec<String> {
    let Some((scheme, rest)) = n.split_once("://") else {
        return Vec::new();
    };
    let (host, path) = if scheme == "file" {
        ("", rest)
    } else {
        let (authority, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
        let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = if hostport.starts_with('[') {
            hostport.find(']').map_or(hostport, |i| &hostport[..=i])
        } else {
            hostport.split_once(':').map_or(hostport, |(h, _)| h)
        };
        (host, path)
    };
    let p = path.trim_end_matches('/');
    let p = p.strip_suffix(".git").unwrap_or(p).trim_end_matches('/');
    // A network URL also in every scheme git fetches over, so a pattern
    // that names one scheme denies the same repository over another.
    let schemes: &[&str] = if scheme == "file" {
        &["file"]
    } else {
        &["https", "http", "ssh", "git", "git+ssh", "ssh+git", scheme]
    };
    let mut out = Vec::with_capacity(4 * (schemes.len() + 1));
    let bases = schemes
        .iter()
        .map(|s| format!("{s}://{host}{p}"))
        .chain(std::iter::once(format!("{host}{p}")));
    for base in bases {
        for end in ["", ".git", "/", ".git/"] {
            out.push(format!("{base}{end}"));
        }
    }
    out
}

/// The programs and bounds the deny check runs, and the cargo home it
/// reads.
#[derive(Debug, Clone)]
pub struct Lookup {
    /// git and the options before its subcommand.
    pub git: Vec<OsString>,
    /// ssh and the options before `-G`.
    pub ssh: Vec<OsString>,
    /// Longest one lookup may take.
    pub limit: Duration,
    /// This machine's cargo home (`cargo_home`), or why it has none.
    pub cargo_home: Result<PathBuf, String>,
}

impl Lookup {
    /// `git` and `ssh` from `PATH`, [`LOOKUP`], and [`cargo_home`].
    pub fn here() -> Self {
        Self {
            git: vec!["git".into()],
            ssh: vec!["ssh".into()],
            limit: LOOKUP,
            cargo_home: cargo_home(),
        }
    }
}

fn program(argv: &[OsString]) -> Command {
    let (prog, pre) = argv
        .split_first()
        .map_or((OsStr::new("false"), &[][..]), |(p, a)| (p.as_os_str(), a));
    let mut c = Command::new(prog);
    c.args(pre);
    c
}

/// The URL git fetches for `url` after its `url.<base>.insteadOf` rules,
/// asked in `dir` (`git ls-remote --get-url`, which talks to no remote):
/// `Ok(None)` when git rewrites nothing, an error when git fails, runs
/// over `lk.limit` or prints nothing.
pub fn git_rewrite(url: &str, dir: &Path, lk: &Lookup) -> Result<Option<String>, String> {
    let mut c = program(&lk.git);
    // `--`: a URL that starts with `-` is no option.
    c.args(["ls-remote", "--get-url", "--", url])
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0");
    let (ok, out, tail) = run_bounded(&mut c, Instant::now() + lk.limit)
        .map_err(|e| format!("git ls-remote --get-url {url}: {e}"))?;
    let got = out.trim();
    if !ok || got.is_empty() {
        return Err(format!("git ls-remote --get-url {url} failed: {tail}"));
    }
    Ok((got != url).then(|| got.to_string()))
}

/// The host ssh connects to for `host` (`ssh -G`, which connects nowhere):
/// `Ok(None)` when it is `host` itself, an error when ssh fails, runs over
/// `lk.limit` or names no host, or `host` is empty or starts with `-`.
pub fn ssh_hostname(host: &str, lk: &Lookup) -> Result<Option<String>, String> {
    if host.is_empty() || host.starts_with('-') {
        return Err(format!("ssh host `{host}` is not looked up"));
    }
    let mut c = program(&lk.ssh);
    c.arg("-G").arg(host);
    let (ok, out, tail) = run_bounded(&mut c, Instant::now() + lk.limit)
        .map_err(|e| format!("ssh -G {host}: {e}"))?;
    if !ok {
        return Err(format!("ssh -G {host} failed: {tail}"));
    }
    let name = out
        .lines()
        .find_map(|l| l.strip_prefix("hostname "))
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| format!("ssh -G {host} names no host"))?;
    Ok((name != host.to_ascii_lowercase()).then_some(name))
}

/// `url` with its ssh host alias resolved: `Ok(None)` when it is no ssh
/// URL or ssh knows its host by no other name.
fn ssh_resolved(url: &str, lk: &Lookup) -> Result<Option<String>, String> {
    let Some(n) = normalise(url) else {
        return Ok(None);
    };
    let Some((scheme, rest)) = n.split_once("://") else {
        return Ok(None);
    };
    if !matches!(scheme, "ssh" | "git+ssh" | "ssh+git") {
        return Ok(None);
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (user, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (format!("{u}@"), h),
        None => (String::new(), authority),
    };
    let (host, port) = hostport
        .split_once(':')
        .map_or((hostport, String::new()), |(h, p)| (h, format!(":{p}")));
    Ok(ssh_hostname(host, lk)?.map(|real| format!("{scheme}://{user}{real}{port}/{path}")))
}

/// The forms of source `s` of the lock file in `dir`, with git's rewrite
/// and an ssh alias resolved; `None` when one cannot be taken apart, or a
/// lookup fails or runs over its bound: the origin is then undetermined.
pub fn resolved_forms(s: &Source, dir: &Path, lk: &Lookup) -> Option<Vec<String>> {
    let url = fetch_url(&s.url);
    let mut more: Vec<String> = git_rewrite(url, dir, lk).ok()?.into_iter().collect();
    let mut ssh = Vec::new();
    for u in std::iter::once(url).chain(more.iter().map(String::as_str)) {
        ssh.extend(ssh_resolved(u, lk).ok()?);
    }
    more.extend(ssh);
    forms(url, &more)
}

/// Why the sources of `locks` keep a job here under `lists`, with the
/// programs of this machine ([`Lookup::here`]).
pub fn check(locks: &Locks, lists: &DenyLists) -> Option<String> {
    check_with(locks, lists, &Lookup::here())
}

/// Why the sources of `locks` keep a job here under `lists`: a problem of
/// the lists or the lock files; no cargo home, or a cargo configuration
/// that replaces a git source (`git_source_replaced`, the checkout's and
/// the cargo home's); or the first source one of whose forms
/// (`resolved_forms`) is denied or cannot be determined. `None` when
/// `lists` name nothing.
pub fn check_with(locks: &Locks, lists: &DenyLists, lk: &Lookup) -> Option<String> {
    if let Some(p) = lists.problem() {
        return Some(format!("{REASON_PREFIX}{p}"));
    }
    if lists.is_empty() {
        return None;
    }
    if let Some(p) = locks.problems.first() {
        return Some(format!("{REASON_PREFIX}{p}"));
    }
    if locks.dirs.is_empty() && locks.found.is_empty() {
        return None;
    }
    let home = match &lk.cargo_home {
        Ok(h) => h,
        Err(e) => return Some(format!("{REASON_PREFIX}{e}")),
    };
    let mut configs = locks.configs.clone();
    configs.extend([home.join("config.toml"), home.join("config")]);
    if let Some(why) = git_source_replaced(&configs) {
        return Some(format!("{REASON_PREFIX}{why}"));
    }
    locks.found.iter().find_map(|(dir, sources)| {
        sources.iter().find_map(|s| {
            let denied = resolved_forms(s, dir, lk).is_none_or(|f| lists.denies_forms(&f));
            denied.then(|| format!("{REASON_PREFIX}{}", s.url))
        })
    })
}

/// Why `sources` keep a job here: the first one `lists` deny by the forms
/// of its URL alone (`DenyLists::denies`).
pub fn denied<'a>(
    sources: impl IntoIterator<Item = &'a Source>,
    lists: &DenyLists,
) -> Option<String> {
    sources
        .into_iter()
        .find(|s| lists.denies(s))
        .map(|s| format!("{REASON_PREFIX}{}", s.url))
}

/// The origin list's path: `remote-deny-origins` in slot's configuration
/// directory.
pub fn origins_path() -> PathBuf {
    crate::hostconfig::config_dir().join(ORIGINS_FILE)
}

/// The deny lists this machine configures: the never-sync list
/// (`never_sync::path_from`, with the host configuration's
/// `never_sync_file`) and `remote-deny-origins`. A list that cannot be
/// read, or a host configuration that does not parse, is an error.
pub fn lists() -> Result<DenyLists, String> {
    let paths = never_sync::path_from(crate::hostconfig::client_path().as_deref())?;
    DenyLists::load(&[paths, origins_path()])
}

/// Why the job in `cwd` of the checkout at `top` stays here for a git
/// dependency, against the lists this machine configures. Reads no lock
/// file when the lists name nothing. A list that cannot be read keeps the
/// job here.
pub fn of_job(top: Option<&Path>, cwd: &Path) -> Option<String> {
    let lists = match lists() {
        Ok(l) => l,
        Err(e) => return Some(format!("{REASON_PREFIX}list unreadable ({e})")),
    };
    if lists.is_empty() {
        return None;
    }
    let top = top.unwrap_or(cwd);
    check(&checkout(top, cwd), &lists)
}

/// This machine's cargo home, from `CARGO_HOME` and `HOME`
/// ([`cargo_home_from`]).
pub fn cargo_home() -> Result<PathBuf, String> {
    cargo_home_from(std::env::var_os("CARGO_HOME"), std::env::var_os("HOME"))
}

/// The cargo home `CARGO_HOME` = `cargo_home` and `HOME` = `home` name:
/// `CARGO_HOME`, else `$HOME/.cargo`. An error when neither is set or the
/// result is relative: it would be taken from wherever slot runs.
pub fn cargo_home_from(
    cargo_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, String> {
    let dir = match cargo_home.filter(|v| !v.is_empty()) {
        Some(c) => PathBuf::from(c),
        None => PathBuf::from(
            home.filter(|h| !h.is_empty())
                .ok_or("neither CARGO_HOME nor HOME is set")?,
        )
        .join(".cargo"),
    };
    if dir.is_absolute() {
        Ok(dir)
    } else {
        Err(format!("cargo home `{}` is not absolute", dir.display()))
    }
}

/// The cargo git database under `cargo_home`.
pub fn db_dir(cargo_home: &Path) -> PathBuf {
    cargo_home.join("git").join("db")
}

/// The node's cargo git database: [`NODE_DB`] under the account's home.
pub fn node_db(home: &Path) -> PathBuf {
    home.join(NODE_DB)
}

/// Whether the git database at `dir` holds object `rev`, read from its
/// loose objects and pack indexes (version 1 and 2) without running git:
/// no configuration of the database is read, nothing is fetched, no
/// alternate is followed.
pub fn has_object(dir: &Path, rev: &str) -> bool {
    if !valid_rev(rev) {
        return false;
    }
    let hex = rev.to_ascii_lowercase();
    let objects = dir.join("objects");
    if objects.join(&hex[..2]).join(&hex[2..]).is_file() {
        return true;
    }
    let want: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    std::fs::read_dir(objects.join("pack"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "idx"))
        .any(|p| std::fs::File::open(&p).is_ok_and(|f| idx_contains(&f, &want)))
}

/// Whether the pack index `f` names object `want`.
fn idx_contains(f: &std::fs::File, want: &[u8]) -> bool {
    let read = |at: u64, len: usize| -> Option<Vec<u8>> {
        let mut b = vec![0; len];
        f.read_exact_at(&mut b, at).ok()?;
        Some(b)
    };
    let Some(head) = read(0, 8) else {
        return false;
    };
    let n = want.len();
    // Version 2: magic, version, fan-out, names. Version 1: fan-out, then
    // a 4-byte offset before each name.
    let (fan_at, names_at, stride, skip) = if head[..4] == [0xff, b't', b'O', b'c'] {
        if head[4..8] != [0, 0, 0, 2] {
            return false;
        }
        (8u64, 8u64 + 1024, n as u64, 0u64)
    } else {
        (0, 1024, n as u64 + 4, 4)
    };
    let fan = |i: usize| -> Option<u64> {
        let b = read(fan_at + 4 * i as u64, 4)?;
        Some(u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])))
    };
    let first = usize::from(want[0]);
    let lo = if first == 0 { Some(0) } else { fan(first - 1) };
    let (Some(mut lo), Some(mut hi)) = (lo, fan(first)) else {
        return false;
    };
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let Some(name) = read(names_at + mid * stride + skip, n) else {
            return false;
        };
        match name.as_slice().cmp(want) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => return true,
        }
    }
    false
}

/// A database entry the node needs, with the revision it must hold.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Wanted {
    /// `<name>-<hash>`.
    pub entry: String,
    /// The locked commit id.
    pub rev: String,
}

/// Runs a command with stdout and stderr read, killed at `deadline`;
/// `(success, stdout, last stderr lines)`.
fn run_bounded(cmd: &mut Command, deadline: Instant) -> Result<(bool, String, String), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().ok_or("stdout")?;
    let mut err = child.stderr.take().ok_or("stderr")?;
    let r_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let r_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let left = deadline.saturating_duration_since(Instant::now());
    let st = sync::watch(&mut child, None, deadline, left);
    let out = r_out.join().unwrap_or_default();
    let err = r_err.join().unwrap_or_default();
    let lines: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(3)..].join(" / ");
    match st {
        Ok(st) => Ok((st.success(), out, tail)),
        Err(stop) => Err(stop.to_string()),
    }
}

/// A cargo command in `dir` outside the pool, with this machine's
/// environment and network.
fn cargo_cmd(cargo: &[String], dir: &Path, args: &[&str]) -> Command {
    let (prog, pre) = cargo
        .split_first()
        .map_or(("cargo", &[][..]), |(p, a)| (p.as_str(), a));
    let mut c = Command::new(prog);
    c.args(pre)
        .args(args)
        .current_dir(dir)
        .env("SLOT_SHIM_OFF", "1")
        .env_remove("SLOT_HOST")
        .env_remove("CARGO_NET_OFFLINE");
    c
}

/// The database entry cargo keeps each `git+` source of `cargo metadata`'s
/// output in: `(source, entry)`, the entry read from the package's
/// checkout path: the first component below `checkouts`, this machine's
/// `<cargo home>/git/checkouts` (the same name as its database). A
/// manifest outside `checkouts` names no entry, and a path inside a
/// checkout that itself holds `git/checkouts/` does not change the entry.
pub fn metadata_entries(json: &str, checkouts: &Path) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let canonical = checkouts.canonicalize().ok();
    let below = |manifest: &Path| -> Option<String> {
        let rest = manifest.strip_prefix(checkouts).ok().or_else(|| {
            canonical
                .as_deref()
                .and_then(|c| manifest.strip_prefix(c).ok())
        })?;
        match rest.components().next()? {
            std::path::Component::Normal(n) => n.to_str().map(str::to_string),
            _ => None,
        }
    };
    let mut out: Vec<(String, String)> = v
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let source = p.get("source")?.as_str()?;
            if !source.starts_with("git+") {
                return None;
            }
            let entry = below(Path::new(p.get("manifest_path")?.as_str()?))?;
            valid_entry(&entry).then(|| (source.to_string(), entry))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// What `cargo metadata --offline` in `dir` says about its git sources.
fn cargo_entries(
    cargo: &[String],
    dir: &Path,
    checkouts: &Path,
    deadline: Instant,
) -> Option<Vec<(String, String)>> {
    let mut c = cargo_cmd(
        cargo,
        dir,
        &["metadata", "--format-version", "1", "--locked", "--offline"],
    );
    match run_bounded(&mut c, deadline) {
        Ok((true, out, _)) => Some(metadata_entries(&out, checkouts)),
        _ => None,
    }
}

/// The database entries the lock files `locks` name, each the one cargo
/// keeps that source in and holding its revision in this machine's
/// database `db`. A lock file whose entries cargo cannot name offline, or
/// that lack a revision, gets one `cargo fetch --locked` here with `cargo`
/// (the job's cargo and toolchain), outside the pool and with this
/// machine's credentials. The second value says what could not be had.
pub fn prepare(
    locks: &[(PathBuf, Vec<Source>)],
    db: &Path,
    cargo: &[String],
    deadline: Instant,
) -> (Vec<Wanted>, Vec<String>) {
    let mut wanted = Vec::new();
    let mut notes = Vec::new();
    let checkouts = db.with_file_name("checkouts");
    for (dir, sources) in locks {
        let pick = |map: &[(String, String)]| -> Vec<Option<String>> {
            sources
                .iter()
                .map(|s| {
                    let key = format!("{}#{}", s.url, s.rev);
                    map.iter()
                        .find(|(src, _)| *src == key)
                        .map(|(_, e)| e.clone())
                        .filter(|e| has_object(&db.join(e), &s.rev))
                })
                .collect()
        };
        let mut have = cargo_entries(cargo, dir, &checkouts, deadline)
            .map_or_else(|| vec![None; sources.len()], |m| pick(&m));
        if have.iter().any(Option::is_none) {
            let mut c = cargo_cmd(cargo, dir, &["fetch", "--locked"]);
            match run_bounded(&mut c, deadline) {
                Ok((true, _, _)) => {
                    have = cargo_entries(cargo, dir, &checkouts, deadline)
                        .map_or_else(|| vec![None; sources.len()], |m| pick(&m));
                }
                Ok((false, _, tail)) => notes.push(format!(
                    "cargo fetch --locked in {} failed: {tail}",
                    dir.display()
                )),
                Err(e) => notes.push(format!(
                    "cargo fetch --locked in {} failed: {e}",
                    dir.display()
                )),
            }
        }
        for (s, e) in sources.iter().zip(have) {
            match e {
                Some(entry) => wanted.push(Wanted {
                    entry,
                    rev: s.rev.clone(),
                }),
                None => notes.push(format!(
                    "{}#{} has no entry in {}",
                    s.url,
                    s.rev,
                    db.display()
                )),
            }
        }
    }
    wanted.sort();
    wanted.dedup();
    (wanted, notes)
}

/// The words of the node's step: `<entry>:<rev>` each.
pub fn probe_words(wanted: &[Wanted]) -> Vec<String> {
    wanted
        .iter()
        .map(|w| format!("{}:{}", w.entry, w.rev))
        .collect()
}

/// Parses the words of `slot __cargo-git`; `None` when one is not
/// `<entry>:<rev>`.
pub fn parse_words(words: &[String]) -> Option<Vec<Wanted>> {
    words
        .iter()
        .map(|w| {
            let (entry, rev) = w.split_once(':')?;
            (valid_entry(entry) && valid_rev(rev)).then(|| Wanted {
                entry: entry.to_string(),
                rev: rev.to_string(),
            })
        })
        .collect()
}

/// What the node's step prints: `missing <entry>` for each wanted entry of
/// database `db` that does not hold its revision ([`has_object`]; an entry
/// that is a symbolic link counts as missing). Names only, never a path.
pub fn node_answer(db: &Path, wanted: &[Wanted]) -> String {
    let mut out = String::new();
    for w in wanted {
        let dir = db.join(&w.entry);
        let link = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink());
        if link || !has_object(&dir, &w.rev) {
            let _ = writeln!(out, "missing {}", w.entry);
        }
    }
    out
}

/// The entries the node's answer says it lacks, each one of `wanted`. An
/// error for any other line, or an entry the job did not ask for: the node
/// does not choose what this machine sends.
pub fn missing_of(text: &str, wanted: &[Wanted]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let entry = line
            .strip_prefix("missing ")
            .ok_or_else(|| format!("the node answered `{line}`"))?;
        if !wanted.iter().any(|w| w.entry == entry) {
            return Err(format!(
                "the node asked for {entry}, which the job does not name"
            ));
        }
        if !out.iter().any(|e| e == entry) {
            out.push(entry.to_string());
        }
    }
    Ok(out)
}

/// Why `carry` did not bring the git dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarryError {
    /// The job must not go to the node: a denied source, a list or lock
    /// file the check cannot read, an answer the job did not ask for.
    Refuse(String),
    /// Nothing was copied, and the job goes as before.
    Skip(String),
}

/// Asks the node which of `wanted` it lacks: `Skip` when it cannot answer
/// (an older slot), `Refuse` when it answers what the job did not ask.
fn ask_node(
    r: &sync::Remote<'_>,
    wanted: &[Wanted],
    deadline: Instant,
) -> Result<Vec<String>, CarryError> {
    let (ssh, opts) = r
        .ssh
        .split_first()
        .ok_or_else(|| CarryError::Skip("no ssh command".into()))?;
    let mut c = Command::new(ssh);
    c.args(opts)
        .arg(r.host)
        .arg(r.remote_slot)
        .arg("__cargo-git")
        .args(probe_words(wanted));
    let (ok, out, tail) = run_bounded(&mut c, deadline).map_err(CarryError::Skip)?;
    if !ok {
        return Err(CarryError::Skip(format!(
            "{} did not say which it holds (an older slot there?): {tail}",
            r.host
        )));
    }
    missing_of(&out, wanted).map_err(CarryError::Refuse)
}

/// Copies each of `missing` with `copy`, each after its own `recheck`: a
/// source denied while the entries are copied stops the copy before the
/// next entry.
fn copy_each(
    missing: &[String],
    mut recheck: impl FnMut() -> Result<(), CarryError>,
    mut copy: impl FnMut(&str) -> Result<(), CarryError>,
) -> Result<(), CarryError> {
    for entry in missing {
        recheck()?;
        copy(entry)?;
    }
    Ok(())
}

/// Brings the git dependencies of the checkout at `top` to the node of
/// `r`: the line to print when something was copied, `Ok(None)` when
/// nothing had to be. The sources it copies are checked against `lists`
/// again first. `argv` names the cargo whose fetch fills this machine's
/// database.
pub fn carry(
    top: &Path,
    cwd: &Path,
    argv: &[String],
    lists: &DenyLists,
    r: &sync::Remote<'_>,
    b: &sync::Bounds,
) -> Result<Option<String>, CarryError> {
    let locks = checkout(top, cwd);
    let lk = Lookup::here();
    if let Some(why) = check_with(&locks, lists, &lk) {
        return Err(CarryError::Refuse(why));
    }
    if locks.found.is_empty() {
        return Ok(None);
    }
    let t0 = Instant::now();
    let deadline = t0 + LIMIT;
    let db = db_dir(
        lk.cargo_home
            .as_ref()
            .map_err(|e| CarryError::Refuse(format!("{REASON_PREFIX}{e}")))?,
    );
    // The job's own cargo and `+toolchain` when it is a cargo command; a
    // wrapper's inner cargo is the one on `PATH`.
    let is_cargo = argv
        .first()
        .and_then(|a| Path::new(a).file_name())
        .is_some_and(|n| n == "cargo");
    let cargo = if is_cargo {
        crate::cargo_fetch::cargo_of(argv)
    } else {
        vec!["cargo".to_string()]
    };
    let (wanted, notes) = prepare(&locks.found, &db, &cargo, deadline);
    for n in &notes {
        eprintln!("slot: git dependency not here: {n}");
    }
    if wanted.is_empty() {
        return Ok(None);
    }
    let missing = ask_node(r, &wanted, deadline)?;
    if missing.is_empty() {
        return Ok(None);
    }
    // The lists are read again before each entry is copied: a source
    // denied since the check above, or since the entry before, is not sent.
    let recheck = || {
        let now = self::lists()
            .map_err(|e| CarryError::Refuse(format!("{REASON_PREFIX}list unreadable ({e})")))?;
        check_with(&locks, &now, &lk).map_or(Ok(()), |why| Err(CarryError::Refuse(why)))
    };
    let copy = |entry: &str| {
        sync::copy_db_entry(&db.join(entry), entry, r, b)
            .map_err(|e| CarryError::Skip(format!("{entry}: {e:#}")))
    };
    copy_each(&missing, recheck, copy)?;
    Ok(Some(format!(
        "slot: git dependencies: {} copied to {} in {:.1} s",
        missing.len(),
        r.host,
        t0.elapsed().as_secs_f64()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REV: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn a_lock_file_gives_its_git_sources_once() {
        let text = format!(
            "version = 3\n\n[[package]]\nname = \"a\"\nversion = \"0.1.0\"\n\n\
             [[package]]\nname = \"b\"\nversion = \"0.1.0\"\nsource = \"git+https://Forge.example/org/b.git?branch=main#{REV}\"\n\n\
             [[package]]\nname = \"c\"\nversion = \"0.1.0\"\nsource = \"git+https://Forge.example/org/b.git?branch=main#{REV}\"\n\n\
             [[package]]\nname = \"d\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n\
             [[package]]\nname = \"e\"\nversion = \"0.1.0\"\nsource = \"git+ssh://git@forge.example:2222/org/e/#{REV}\"\n"
        );
        let got = parse_lock(&text).unwrap();
        assert_eq!(
            got,
            vec![
                Source {
                    url: "git+https://Forge.example/org/b.git?branch=main".into(),
                    rev: REV.into(),
                    name: "b".into(),
                },
                Source {
                    url: "git+ssh://git@forge.example:2222/org/e/".into(),
                    rev: REV.into(),
                    name: "e".into(),
                },
            ]
        );
        assert!(parse_lock("not toml [").is_err());
        assert!(Source::parse("git+https://x/y#short").is_none());
    }

    #[test]
    fn the_fetch_url_drops_git_plus_the_query_and_the_fragment() {
        assert_eq!(
            fetch_url(&format!("git+https://h.example.org/g/r.git?branch=x#{REV}")),
            "https://h.example.org/g/r.git"
        );
        assert_eq!(
            fetch_url("git+ssh://git@h.example.org/g/r?rev=1"),
            "ssh://git@h.example.org/g/r"
        );
        assert_eq!(
            fetch_url("git+ssh://git@h.example.org:2222/g/r"),
            "ssh://git@h.example.org:2222/g/r"
        );
        assert_eq!(fetch_url("git+file:///srv/r.git#x"), "file:///srv/r.git");
    }

    #[test]
    fn a_pattern_matches_the_whole_text_as_a_shell_case_does() {
        let pat = "*git.example.org[:/]team/*";
        for yes in [
            "https://git.example.org/team/dep.git",
            "ssh://git@git.example.org/team/a/b",
            "git@git.example.org:team/dep.git",
        ] {
            assert!(glob_match(pat, yes), "{yes}");
        }
        for no in [
            "https://git.example.org/teamwork/dep.git",
            "https://git.example.org/other/team/x",
            "https://git.example.org.evil/team/x",
            "https://git.example.org-team/x",
        ] {
            assert!(!glob_match(pat, no), "{no}");
        }
        assert!(glob_match("*", ""));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("[a-c]x", "bx"));
        assert!(!glob_match("[!a-c]x", "bx"));
        assert!(glob_match("[^a-c]x", "dx"));
        assert!(glob_match("[]]", "]"));
        assert!(glob_match("a[b", "a[b"), "an open bracket is itself");
        assert!(glob_match(r"a\*b", "a*b"));
        assert!(!glob_match(r"a\*b", "axb"));
        assert!(!glob_match("abc", "abcd"), "the whole text");
        assert!(glob_match("*/team/*", "https://h/x/team/y"));
        assert!(glob_match("a*b*c", "a-b-b-c"));
        assert!(!glob_match("a*b*c", "a-b-b-d"));
    }

    #[test]
    fn the_deny_check_takes_the_url_apart_before_it_matches() {
        let lists = DenyLists::parse(["origin:*git.example.org/team/*\n/srv/private\n"]);
        let s = |u: &str| Source::parse(&format!("{u}#{REV}")).unwrap();
        for u in [
            "git+HTTPS://Git.Example.ORG/team/dep.git",
            "git+https://git.example.org:443/team/dep.git",
            "git+https://git.example.org//team/dep.git",
            "git+https://git.example.org./team/dep.git",
            "git+ssh://git.example.org:22/team/dep",
            "git+file:///srv/pri%76ate/dep.git",
            "git+file:///srv//private/dep.git",
            "git+https://git.example.org:8443/team/dep.git",
        ] {
            assert!(lists.denies(&s(u)), "{u}");
        }
        for u in [
            "git+https://git.example.org/teamwork/dep.git",
            "git+file:///srv/privateer/dep.git",
        ] {
            assert!(!lists.denies(&s(u)), "{u}");
        }
        // git's scp form with a port in brackets, as an insteadOf target
        // names it: the port leaves the path.
        assert_eq!(
            normalise("[git@Git.Example.org:2222]:team/dep.git").as_deref(),
            Some("ssh://git@git.example.org:2222/team/dep.git")
        );
        assert_eq!(
            normalise("git@git.example.org:team/dep.git").as_deref(),
            Some("ssh://git@git.example.org/team/dep.git")
        );
        let f = forms("[git@git.example.org:2222]:team/dep.git", &[]).unwrap();
        assert!(lists.denies_forms(&f), "{f:?}");
    }

    #[test]
    fn a_port_userinfo_or_trailing_git_does_not_pass_a_pattern() {
        let s = |u: &str| Source::parse(&format!("{u}#{REV}")).unwrap();
        for (pattern, yes, no) in [
            (
                "*git.example.org[:/]team/*",
                &[
                    "git+ssh://git@git.example.org:2222/team/x",
                    "git+https://u:p@git.example.org:8443/team/x",
                ][..],
                &["git+https://git.example.org/other/x"][..],
            ),
            (
                "https://git.example.org/team/*",
                &[
                    "git+https://user@git.example.org/team/x",
                    "git+https://user@git.example.org:8443/team/x",
                ],
                &[
                    "git+https://git.example.org.evil/team/x",
                    "git+https://user@git.example.org/teamwork/x",
                ],
            ),
            (
                "*git.example.org/team/dep.git",
                &[
                    "git+https://git.example.org/team/dep",
                    "git+https://git.example.org/team/dep/",
                ],
                &["git+https://git.example.org/team/depot"],
            ),
            (
                "*git.example.org/team/dep",
                &[
                    "git+https://git.example.org/team/dep.git",
                    "git+https://git.example.org/team/dep.git/",
                    "git+https://git.example.org/team/dep//",
                ],
                &["git+https://git.example.org/team/dep.gitx"],
            ),
            (
                // A pattern that names a scheme denies the other schemes'
                // URLs of the same host and path.
                "https://git.example.org/team/*",
                &[
                    "git+ssh://git@git.example.org/team/x",
                    "git+ssh://git@git.example.org:2222/team/x",
                    "git+git://git.example.org/team/x",
                    "git+http://git.example.org/team/x",
                ],
                &["git+ssh://git@git.example.org/other/x"],
            ),
        ] {
            let lists = DenyLists::parse([format!("origin:{pattern}\n").as_str()]);
            for u in yes {
                assert!(lists.denies(&s(u)), "{pattern} lets through {u}");
            }
            for u in no {
                assert!(!lists.denies(&s(u)), "{pattern} denies {u}");
            }
        }
    }

    /// A [`Lookup`] with git and ssh from `PATH` and an empty cargo home
    /// under `dir`.
    fn lookup(dir: &Path) -> Lookup {
        let home = dir.join("cargo-home");
        std::fs::create_dir_all(&home).unwrap();
        Lookup {
            git: vec!["git".into()],
            ssh: vec!["ssh".into()],
            limit: LOOKUP,
            cargo_home: Ok(home),
        }
    }

    /// `sh <dir>/<name>.sh` running `body`.
    fn script(dir: &Path, name: &str, body: &str) -> Vec<OsString> {
        let path = dir.join(format!("{name}.sh"));
        std::fs::write(&path, body).unwrap();
        vec!["sh".into(), path.into()]
    }

    /// Locks naming `urls` (with [`REV`]) in one lock file in `dir`.
    fn locks_of(dir: &Path, urls: &[&str]) -> Locks {
        let sources = urls
            .iter()
            .map(|u| Source::parse(&format!("{u}#{REV}")).unwrap())
            .collect();
        Locks {
            found: vec![(dir.to_path_buf(), sources)],
            dirs: vec![dir.to_path_buf()],
            ..Locks::default()
        }
    }

    #[test]
    fn a_lookup_that_fails_or_runs_over_its_bound_keeps_the_job_here() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path();
        let lists = DenyLists::parse(["origin:*denied.example.org*\n"]);
        let echo = script(
            dir,
            "echo",
            "for a; do last=$a; done; printf '%s\\n' \"$last\"\n",
        );
        let same = script(dir, "same", "printf 'user x\\nhostname %s\\n' \"$2\"\n");
        let fail = script(dir, "fail", "exit 1\n");
        let hang = script(dir, "hang", "exec sleep 5\n");
        let nameless = script(dir, "nameless", "printf 'user x\\n'\n");
        let silent = script(dir, "silent", "exit 0\n");
        let base = Lookup {
            git: echo.clone(),
            ssh: same.clone(),
            limit: Duration::from_millis(300),
            ..lookup(dir)
        };
        let https = locks_of(dir, &["git+https://open.example.org/team/x"]);
        let ssh = locks_of(dir, &["git+ssh://git@open.example.org/team/x"]);
        assert_eq!(
            check_with(&https, &lists, &base),
            None,
            "both lookups answer"
        );
        assert_eq!(check_with(&ssh, &lists, &base), None, "both lookups answer");
        for (what, locks, lk) in [
            (
                "git fails",
                &https,
                Lookup {
                    git: fail.clone(),
                    ..base.clone()
                },
            ),
            (
                "git hangs",
                &https,
                Lookup {
                    git: hang.clone(),
                    ..base.clone()
                },
            ),
            (
                "ssh fails",
                &ssh,
                Lookup {
                    ssh: fail.clone(),
                    ..base.clone()
                },
            ),
            (
                "ssh hangs",
                &ssh,
                Lookup {
                    ssh: hang.clone(),
                    ..base.clone()
                },
            ),
            (
                "ssh names no host",
                &ssh,
                Lookup {
                    ssh: nameless.clone(),
                    ..base.clone()
                },
            ),
            (
                "git prints nothing",
                &https,
                Lookup {
                    git: silent.clone(),
                    ..base.clone()
                },
            ),
        ] {
            let t0 = Instant::now();
            let why = check_with(locks, &lists, &lk);
            assert!(
                why.as_deref().is_some_and(|w| w.starts_with(REASON_PREFIX)),
                "{what}: {why:?}"
            );
            assert!(t0.elapsed() < Duration::from_secs(4), "{what}: bounded");
        }
    }

    /// A host that would be an ssh option is never handed to ssh, with or
    /// without ssh on this machine: the lookup is undetermined.
    #[test]
    fn a_host_that_is_an_ssh_option_is_not_looked_up() {
        let t = tempfile::tempdir().unwrap();
        let ran = t.path().join("ran");
        let lk = Lookup {
            ssh: script(
                t.path(),
                "ssh",
                &format!("touch {}\nprintf 'hostname x\\n'\n", ran.display()),
            ),
            ..lookup(t.path())
        };
        for host in ["-oProxyCommand=x", "-F/dev/null", ""] {
            assert!(ssh_hostname(host, &lk).is_err(), "{host:?}");
        }
        assert!(!ran.exists(), "ssh ran");
    }

    /// A deny list that exists and cannot be read is an error, not an
    /// empty list.
    #[test]
    fn an_unreadable_deny_list_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join(ORIGINS_FILE);
        std::fs::write(&f, "origin:*denied.example.org*\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_to_string(&f).is_ok();
        let got = DenyLists::load(&[t.path().join("absent"), f.clone()]);
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        if !readable {
            assert!(got.is_err(), "an unreadable list was read as {got:?}");
        }
        assert!(DenyLists::load(&[t.path().join("absent")]).is_ok_and(|l| l.is_empty()));
    }

    #[test]
    fn an_ssh_alias_of_a_denied_host_keeps_the_job_here() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path();
        let lk = Lookup {
            git: script(
                dir,
                "echo",
                "for a; do last=$a; done; printf '%s\\n' \"$last\"\n",
            ),
            ssh: script(
                dir,
                "alias",
                "case \"$2\" in forge-alias) h=git.example.org;; *) h=$2;; esac\n\
                 printf 'hostname %s\\n' \"$h\"\n",
            ),
            ..lookup(dir)
        };
        let lists = DenyLists::parse(["origin:*git.example.org/team/*\n"]);
        let alias = locks_of(dir, &["git+ssh://git@forge-alias/team/dep"]);
        assert!(!lists.denies(&alias.found[0].1[0]), "the alias as written");
        assert_eq!(
            check_with(&alias, &lists, &lk),
            Some(format!("{REASON_PREFIX}git+ssh://git@forge-alias/team/dep"))
        );
        let other = locks_of(dir, &["git+ssh://git@other-alias/team/dep"]);
        assert_eq!(check_with(&other, &lists, &lk), None);
    }

    #[test]
    fn a_lock_url_that_starts_with_a_dash_is_no_option_of_git() {
        let t = tempfile::tempdir().unwrap();
        let lk = Lookup {
            git: vec![
                "git".into(),
                "-c".into(),
                "url.https://denied.example.org/.insteadOf=-x://".into(),
            ],
            ..lookup(t.path())
        };
        assert_eq!(
            git_rewrite("-x://h/p", t.path(), &lk),
            Ok(Some("https://denied.example.org/h/p".into()))
        );
        let lists = DenyLists::parse(["origin:https://denied.example.org/*\n"]);
        let locks = locks_of(t.path(), &["git+-x://h/p"]);
        assert_eq!(
            check_with(&locks, &lists, &lk),
            Some(format!("{REASON_PREFIX}git+-x://h/p"))
        );
    }

    #[test]
    fn a_git_source_replaced_in_cargo_config_keeps_the_job_here() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        // `denied` holds crate `dep`; `allowed.git` is a copy of it.
        let denied = root.join("denied");
        std::fs::create_dir_all(denied.join("src")).unwrap();
        std::fs::write(
            denied.join("Cargo.toml"),
            "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(denied.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        git(&denied, &["init", "-q"]);
        git(&denied, &["add", "."]);
        git(&denied, &["commit", "-qm", "x"]);
        let rev = git(&denied, &["rev-parse", "HEAD"]);
        git(&root, &["clone", "-q", "--bare", "denied", "allowed.git"]);
        let allowed = format!("file://{}/allowed.git", root.display());
        let app = root.join("app");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::create_dir_all(app.join(".cargo")).unwrap();
        std::fs::write(
            app.join("Cargo.toml"),
            format!(
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                 [dependencies]\ndep = {{ git = \"{allowed}\" }}\n"
            ),
        )
        .unwrap();
        std::fs::write(app.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(
            app.join("Cargo.lock"),
            format!(
                "version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n\
                 dependencies = [\"dep\"]\n\n[[package]]\nname = \"dep\"\nversion = \"0.1.0\"\n\
                 source = \"git+{allowed}#{rev}\"\n"
            ),
        )
        .unwrap();
        std::fs::write(
            app.join(".cargo/config.toml"),
            format!(
                "[source.allowed]\ngit = \"{allowed}\"\nreplace-with = \"den\"\n\
                 [source.den]\ngit = \"file://{}\"\n",
                denied.display()
            ),
        )
        .unwrap();
        let lk = lookup(&root);
        let home = lk.cargo_home.clone().unwrap();
        let mut locks = locked(&[app.join("Cargo.lock")]);
        locks.configs = config_files(&app, &locks.dirs);
        // The lock file names the allowed URL; cargo maps it to the
        // checkout of the denied repository.
        let cargo = [
            "env".to_string(),
            format!("CARGO_HOME={}", home.display()),
            "cargo".to_string(),
        ];
        let deadline = Instant::now() + Duration::from_secs(50);
        let (wanted, notes) = prepare(&locks.found, &db_dir(&home), &cargo, deadline);
        assert!(
            wanted.len() == 1 && wanted[0].entry.starts_with("denied-"),
            "{wanted:?} {notes:?}"
        );
        let lists = DenyLists::parse([format!("origin:file://{}\n", denied.display()).as_str()]);
        assert!(!lists.denies(&locks.found[0].1[0]), "the lock file's URL");
        let why = check_with(&locks, &lists, &lk);
        assert!(
            why.as_deref().is_some_and(|w| w.starts_with(REASON_PREFIX)),
            "{why:?}"
        );
    }

    #[test]
    fn a_source_replacement_in_any_cargo_config_keeps_the_job_here() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path();
        let file = |name: &str, text: &str| {
            let p = dir.join(name);
            std::fs::write(&p, text).unwrap();
            p
        };
        let git_replaced = file(
            "git-replaced",
            "[source.dep]\ngit = \"https://h.example.org/g/dep\"\nreplace-with = \"v\"\n\
             [source.v]\ndirectory = \"vendor\"\n",
        );
        let by_git = file(
            "by-git",
            "[source.crates-io]\nreplace-with = \"g\"\n[source.g]\ngit = \"https://h.example.org/g/x\"\n",
        );
        let half_a = file("half-a", "[source.crates-io]\nreplace-with = \"g\"\n");
        let half_b = file(
            "half-b",
            "[source.g]\ngit = \"https://h.example.org/g/x\"\n",
        );
        let vendored = file(
            "vendored",
            "[source.crates-io]\nreplace-with = \"v\"\n[source.v]\ndirectory = \"vendor\"\n",
        );
        let plain = file("plain", "[build]\njobs = 1\n");
        let broken = file("broken", "[source.x\n");
        // cargo's `include` pulls in files this check does not read: the
        // replacement in `mirror.toml` would pass unseen.
        file(
            "mirror.toml",
            "[source.pub]\ngit = \"https://h.example.org/g/dep\"\nreplace-with = \"int\"\n\
             [source.int]\ngit = \"https://denied.example.org/team/dep\"\n",
        );
        let includes = file("includes", "include = [\"mirror.toml\"]\n");
        let includes_one = file(
            "includes-one",
            "include = \"mirror.toml\"\n[build]\njobs = 1\n",
        );
        let includes_table = file(
            "includes-table",
            "[[include]]\npath = \"mirror.toml\"\noptional = true\n",
        );
        for (what, files) in [
            ("a git source replaced", vec![git_replaced.clone()]),
            ("a registry replaced by a git source", vec![by_git]),
            ("split over two files", vec![half_a.clone(), half_b]),
            ("a file that does not parse", vec![plain.clone(), broken]),
            ("an include list", vec![includes]),
            ("an include string", vec![includes_one]),
            ("an include table", vec![includes_table]),
        ] {
            assert!(git_source_replaced(&files).is_some(), "{what}");
        }
        assert_eq!(git_source_replaced(&[vendored, plain, half_a]), None);
        let lists = DenyLists::parse(["origin:*denied.example.org*\n"]);
        let lk = Lookup {
            git: script(
                dir,
                "echo",
                "for a; do last=$a; done; printf '%s\\n' \"$last\"\n",
            ),
            ..lookup(dir)
        };
        let mut locks = locks_of(dir, &["git+https://open.example.org/g/x"]);
        assert_eq!(check_with(&locks, &lists, &lk), None);
        locks.configs = vec![git_replaced.clone()];
        assert!(check_with(&locks, &lists, &lk).is_some(), "the checkout's");
        locks.configs.clear();
        let home = lk.cargo_home.clone().unwrap();
        std::fs::copy(&git_replaced, home.join("config.toml")).unwrap();
        assert!(
            check_with(&locks, &lists, &lk).is_some(),
            "the cargo home's"
        );
        let none = Lookup {
            cargo_home: Err("no HOME".into()),
            ..lk
        };
        std::fs::remove_file(home.join("config.toml")).unwrap();
        assert!(check_with(&locks, &lists, &none).is_some(), "no cargo home");
    }

    #[test]
    fn a_relative_or_missing_cargo_home_is_refused() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(cargo_home_from(os("/c"), None), Ok(PathBuf::from("/c")));
        assert_eq!(
            cargo_home_from(os(""), os("/h")),
            Ok(PathBuf::from("/h/.cargo"))
        );
        assert_eq!(
            cargo_home_from(None, os("/h")),
            Ok(PathBuf::from("/h/.cargo"))
        );
        for (cargo_home, home) in [
            (os("rel/ch"), os("/h")),
            (os("."), os("/h")),
            (None, None),
            (os(""), None),
            (None, os("rel")),
            (None, os("")),
        ] {
            assert!(
                cargo_home_from(cargo_home.clone(), home.clone()).is_err(),
                "{cargo_home:?} {home:?}"
            );
        }
    }

    #[test]
    fn a_source_denied_during_the_copy_stops_it_before_the_next_entry() {
        let missing = ["dep-1".to_string(), "dep-2".to_string()];
        let mut checks = 0;
        let mut copied = Vec::new();
        let got = copy_each(
            &missing,
            || {
                checks += 1;
                if checks > 1 {
                    Err(CarryError::Refuse("denied since".into()))
                } else {
                    Ok(())
                }
            },
            |e| {
                copied.push(e.to_string());
                Ok(())
            },
        );
        assert_eq!(got, Err(CarryError::Refuse("denied since".into())));
        assert_eq!(copied, ["dep-1"]);
    }

    #[test]
    fn a_rule_line_takes_comments_and_spelling_and_anything_else_fails_closed() {
        let s = Source::parse(&format!("git+https://git.example.org/team/x#{REV}")).unwrap();
        for text in [
            "origin:*git.example.org/team/* # the private forge\n",
            "Origin : *git.example.org/team/*\n",
            "ORIGIN:*git.example.org/team/*\n",
            "  origin :*git.example.org/team/*\t\n",
        ] {
            let lists = DenyLists::parse([text]);
            assert!(lists.denies(&s), "{text:?}");
            assert_eq!(lists.problem(), None, "{text:?}");
        }
        let junk = DenyLists::parse(["# comment\n\n/srv/a # trees\ngit.example.org/team\n"]);
        assert!(
            junk.problem().is_some_and(|p| p.contains("line 4")),
            "{:?}",
            junk.problem()
        );
        let empty = DenyLists::parse(["origin:\n"]);
        assert!(empty.problem().is_some(), "an empty pattern");
    }

    #[test]
    fn the_probe_answers_entry_names_only() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("db");
        let w = Wanted {
            entry: "dep-00ff00ff00ff00ff".into(),
            rev: REV.into(),
        };
        let answer = node_answer(&db, std::slice::from_ref(&w));
        assert_eq!(answer, "missing dep-00ff00ff00ff00ff\n");
        assert!(!answer.contains(&db.display().to_string()));
    }

    #[test]
    fn either_file_holds_paths_and_origin_patterns() {
        let lists = DenyLists::parse([
            "# customer data\n/srv/private/\norigin:*git.example.org[:/]team/*\n",
            "origin:https://other.example.org/*\n/srv/second\n",
        ]);
        let s = |u: &str| Source::parse(&format!("{u}#{REV}")).unwrap();
        let open = [
            s("git+https://git.example.org/public/a"),
            s("git+file:///srv/open/b"),
            s("git+https://git.example.org/teamwork/a"),
        ];
        assert_eq!(denied(&open, &lists), None);
        for (src, want) in [
            (
                "git+file:///srv/private/c.git",
                "git+file:///srv/private/c.git",
            ),
            ("git+file:///srv/second/d", "git+file:///srv/second/d"),
            (
                "git+https://git.example.org/team/d.git?branch=main",
                "git+https://git.example.org/team/d.git?branch=main",
            ),
            (
                "git+ssh://git@git.example.org/team/e",
                "git+ssh://git@git.example.org/team/e",
            ),
            (
                "git+https://other.example.org/x/y",
                "git+https://other.example.org/x/y",
            ),
        ] {
            let list = [open[0].clone(), s(src)];
            assert_eq!(
                denied(&list, &lists),
                Some(format!("git dependency denied: {want}")),
                "{src}"
            );
        }
        assert!(
            DenyLists::parse(["# nothing\nrelative/path\n"])
                .problem()
                .is_some(),
            "a line that is no rule"
        );
    }

    #[test]
    fn the_nodes_step_takes_only_entries_and_full_revisions() {
        let ok = parse_words(&[format!("dep-0123abcd:{REV}")]).unwrap();
        assert_eq!(
            ok,
            [Wanted {
                entry: "dep-0123abcd".into(),
                rev: REV.into()
            }]
        );
        for bad in [
            format!("../x-1:{REV}"),
            format!(".x-1:{REV}"),
            format!("a/b-1:{REV}"),
            "dep-1:abc".to_string(),
            format!("nodash:{REV}"),
            format!("dep-1{REV}"),
        ] {
            assert!(parse_words(std::slice::from_ref(&bad)).is_none(), "{bad}");
        }
    }

    #[test]
    fn only_entries_the_job_asked_for_are_taken_from_the_answer() {
        let wanted = [Wanted {
            entry: "dep-1".into(),
            rev: REV.into(),
        }];
        assert_eq!(
            missing_of("missing dep-1\n\n", &wanted),
            Ok(vec!["dep-1".into()])
        );
        assert_eq!(missing_of("", &wanted), Ok(vec![]));
        for bad in [
            "missing dep-1\nmissing evil-2\n",
            "missing ../x\n",
            "db /n/.cargo/git/db\nmissing dep-1\n",
        ] {
            assert!(missing_of(bad, &wanted).is_err(), "{bad:?}");
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args([
                "-c",
                "commit.gpgSign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn an_object_is_found_loose_and_packed_without_git_and_nothing_else_counts() {
        let t = tempfile::tempdir().unwrap();
        let work = t.path().join("w");
        std::fs::create_dir(&work).unwrap();
        git(&work, &["init", "-q"]);
        std::fs::write(work.join("f"), "x").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-qm", "x"]);
        let rev = git(&work, &["rev-parse", "HEAD"]);
        let db = t.path().join("db");
        std::fs::create_dir(&db).unwrap();
        // `clone --local` copies loose objects; `--no-local` plus a repack
        // gives a pack and its index.
        let loose = db.join("dep-00ff00ff00ff00ff");
        git(
            t.path(),
            &["clone", "-q", "--bare", "w", loose.to_str().unwrap()],
        );
        let packed = db.join("dep-11ee11ee11ee11ee");
        git(
            t.path(),
            &[
                "clone",
                "-q",
                "--bare",
                "--no-local",
                "w",
                packed.to_str().unwrap(),
            ],
        );
        git(&packed, &["repack", "-adq"]);
        let idx = std::fs::read_dir(packed.join("objects/pack"))
            .unwrap()
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "idx"));
        assert!(idx, "packed");
        let empty = db.join("dep-22dd22dd22dd22dd");
        git(t.path(), &["init", "-q", "--bare", empty.to_str().unwrap()]);
        assert!(has_object(&loose, &rev));
        assert!(has_object(&packed, &rev));
        assert!(has_object(&packed, &rev.to_uppercase()));
        assert!(!has_object(&empty, &rev));
        assert!(!has_object(&packed, REV));
        assert!(!has_object(&packed, "abc"));
        // A link where an entry should be counts as missing.
        std::os::unix::fs::symlink(&packed, db.join("dep-33cc33cc33cc33cc")).unwrap();
        let w = |e: &str| Wanted {
            entry: e.into(),
            rev: rev.clone(),
        };
        assert_eq!(
            node_answer(
                &db,
                &[
                    w("dep-11ee11ee11ee11ee"),
                    w("dep-22dd22dd22dd22dd"),
                    w("dep-33cc33cc33cc33cc"),
                    w("dep-44")
                ]
            ),
            "missing dep-22dd22dd22dd22dd\nmissing dep-33cc33cc33cc33cc\nmissing dep-44\n"
        );
    }

    #[test]
    fn cargo_metadata_names_the_entry_of_each_git_source() {
        let json = format!(
            r#"{{"packages":[
              {{"source":"git+https://h.example.org/g/dep.git?branch=main#{REV}","manifest_path":"/c/.cargo/git/checkouts/dep-00ff00ff00ff00ff/0123456/sub/Cargo.toml"}},
              {{"source":"registry+https://github.com/rust-lang/crates.io-index","manifest_path":"/c/.cargo/registry/src/x/serde-1.0.0/Cargo.toml"}},
              {{"source":null,"manifest_path":"/w/app/Cargo.toml"}},
              {{"source":"git+https://h.example.org/g/bad#{REV}","manifest_path":"/c/.cargo/git/checkouts/../x/Cargo.toml"}}
            ]}}"#
        );
        let checkouts = Path::new("/c/.cargo/git/checkouts");
        assert_eq!(
            metadata_entries(&json, checkouts),
            [(
                format!("git+https://h.example.org/g/dep.git?branch=main#{REV}"),
                "dep-00ff00ff00ff00ff".to_string()
            )]
        );
        assert_eq!(metadata_entries("not json", checkouts), Vec::new());
    }

    /// The entry is the first component below this machine's checkouts
    /// directory: a crate inside a repository at a path that itself holds
    /// `git/checkouts/<other>/` names its own entry, and a manifest outside
    /// the checkouts directory names none.
    #[test]
    fn a_nested_checkouts_path_does_not_steer_the_entry() {
        let json = format!(
            r#"{{"packages":[
              {{"source":"git+https://h.example.org/g/dep#{REV}","manifest_path":"/c/.cargo/git/checkouts/dep-00ff00ff00ff00ff/0123456/git/checkouts/fork-11ee11ee11ee11ee/x/Cargo.toml"}},
              {{"source":"git+https://h.example.org/g/other#{REV}","manifest_path":"/elsewhere/git/checkouts/other-22dd22dd22dd22dd/0123456/Cargo.toml"}}
            ]}}"#
        );
        assert_eq!(
            metadata_entries(&json, Path::new("/c/.cargo/git/checkouts")),
            [(
                format!("git+https://h.example.org/g/dep#{REV}"),
                "dep-00ff00ff00ff00ff".to_string()
            )]
        );
    }

    #[test]
    fn a_url_is_normalised_or_refused() {
        for (raw, want) in [
            (
                "HTTPS://User@Git.Example.ORG.:0443/team//dep.git",
                Some("https://User@git.example.org/team/dep.git"),
            ),
            (
                "ssh://git@git.example.org:22/team/dep",
                Some("ssh://git@git.example.org/team/dep"),
            ),
            (
                "ssh://git@git.example.org:2222/team/dep",
                Some("ssh://git@git.example.org:2222/team/dep"),
            ),
            (
                "git@git.example.org:team/dep.git",
                Some("ssh://git@git.example.org/team/dep.git"),
            ),
            ("file:///srv//pri%76ate/x", Some("file:///srv/private/x")),
            ("file://localhost/srv/x", Some("file:///srv/x")),
            ("https://[::1]:443/x", Some("https://[::1]/x")),
            ("https://h.example.org:44x/x", None),
            ("https:///x", None),
            ("file://relative/x", None),
            ("https://h.example.org/%zz", None),
            ("dep.git", None),
        ] {
            assert_eq!(normalise(raw).as_deref(), want, "{raw}");
        }
    }

    #[test]
    fn an_ssh_alias_resolves_to_its_host() {
        if Command::new("ssh").arg("-V").output().is_err() {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let cfg = t.path().join("config");
        std::fs::write(&cfg, "Host forge-alias\n  HostName git.example.org\n").unwrap();
        let lk = Lookup {
            ssh: vec!["ssh".into(), "-F".into(), cfg.into()],
            ..Lookup::here()
        };
        assert_eq!(
            ssh_hostname("forge-alias", &lk),
            Ok(Some("git.example.org".into()))
        );
        assert_eq!(ssh_hostname("git.example.org", &lk), Ok(None));
        assert!(
            ssh_hostname("-oProxyCommand=x", &lk).is_err(),
            "a host that is an option is not looked up, and is undetermined"
        );
    }

    #[test]
    fn a_lock_file_that_does_not_parse_or_a_git_source_without_a_full_revision_is_a_problem() {
        let t = tempfile::tempdir().unwrap();
        let pkg = |s: &str| {
            format!(
                "version = 3\n[[package]]\nname = \"d\"\nversion = \"0.1.0\"\nsource = \"{s}\"\n"
            )
        };
        let files: Vec<PathBuf> = [
            ("garbage", "[[package]\nname = ".to_string()),
            ("short", pkg("git+file:///srv/x#abc")),
            ("no-rev", pkg("git+file:///srv/x")),
            ("no-url", pkg(&format!("git+dep.git#{REV}"))),
            ("good", pkg(&format!("git+file:///srv/x#{REV}"))),
        ]
        .iter()
        .map(|(n, text)| {
            let p = t.path().join(n).join("Cargo.lock");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            p
        })
        .chain([t.path().join("gone/Cargo.lock")])
        .collect();
        let l = locked(&files);
        assert_eq!(l.found.len(), 1, "{l:?}");
        assert_eq!(l.problems.len(), 5, "{l:?}");
        let lists = DenyLists::parse(["/srv/elsewhere\n"]);
        assert!(
            check(&l, &lists).is_some_and(|r| r.starts_with("git dependency denied: lock file ")),
            "fails closed"
        );
        assert_eq!(check(&l, &DenyLists::default()), None, "no list, no check");
    }
}
