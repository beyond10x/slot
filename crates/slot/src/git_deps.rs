//! The git dependencies of a job's checkout reach the node with the job.
//!
//! A cargo command on a node fetches the `git+` sources its `Cargo.lock`
//! names from the node, which may have no way to them: a private forge it
//! holds no credential for, or no network to it. The node's fetch-once step
//! (`cargo_fetch.rs`) runs there too, and only for a cargo command the
//! client sends itself, not for a wrapper's inner cargo call.
//!
//! So before a job is placed on a node the client reads every `Cargo.lock`
//! of the checkout (the ones the sync sends), takes the `git+` sources and
//! their locked revisions, and makes sure each revision is in this
//! machine's cargo git database: when one is missing it runs
//! `cargo fetch --locked` here, with this machine's credentials and outside
//! the pool. It asks the node which of those database entries
//! (`$CARGO_HOME/git/db/<name>-<hash>`) lack the revision
//! (`slot __cargo-git`), and copies only those, over the same ssh and rsync
//! as the sync. cargo on the node then finds the locked revision in its
//! database and does not fetch. Only git objects cross: no credential, no
//! `FETCH_HEAD`.
//!
//! A source on a remote deny list keeps the job on this machine with the
//! reason `git dependency denied: <source>`. The lists are the never-sync
//! file (`never_sync.rs`, `remote-deny`) and `remote-deny-origins` in
//! slot's configuration directory; either may hold either kind of line:
//! `/abs/path`, which denies a `git+file` source under that prefix, and
//! `origin:<pattern>`, a shell pattern (`case` semantics) matched against
//! the URL cargo fetches (`fetch_url`).

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::never_sync;
use crate::sync;

/// File name of the origin deny list in slot's configuration directory.
pub const ORIGINS_FILE: &str = "remote-deny-origins";

/// Start of the reason of a job a denied source keeps here.
pub const REASON_PREFIX: &str = "git dependency denied: ";

/// Longest the fetch here, the probe and the copies may take together.
pub const LIMIT: Duration = Duration::from_secs(300);

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
    /// The `source` value of a lock file's package, when it is `git+`.
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

/// The `git+` sources of a lock file's text, each once.
pub fn parse_lock(text: &str) -> Vec<Source> {
    let Ok(table) = toml::from_str::<toml::Table>(text) else {
        return Vec::new();
    };
    let mut out: Vec<Source> = table
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("source")?.as_str())
        .filter_map(Source::parse)
        .collect();
    out.sort();
    out.dedup();
    out
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
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files.dedup();
    files
}

/// The `git+` sources of each lock file in `files`, by the directory that
/// holds it; files without one are left out.
pub fn locked(files: &[PathBuf]) -> Vec<(PathBuf, Vec<Source>)> {
    files
        .iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(f).ok()?;
            let sources = parse_lock(&text);
            let dir = f.parent()?.to_path_buf();
            (!sources.is_empty()).then_some((dir, sources))
        })
        .collect()
}

/// The URL cargo fetches for a source: without `git+`, its `?query` and
/// its `#fragment` (`git+https://h/g/r.git?branch=x#<rev>` is
/// `https://h/g/r.git`).
pub fn fetch_url(source: &str) -> &str {
    let url = source.strip_prefix("git+").unwrap_or(source);
    url.split(['?', '#']).next().unwrap_or(url)
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

/// The remote deny lists as the git dependencies read them. Each file
/// (`remote-deny`, `remote-deny-origins`) may hold either kind of line:
/// `/abs/path`, a path prefix, and `origin:<pattern>`, a shell pattern
/// matched against a remote URL. Other lines are skipped.
#[derive(Debug, Clone, Default)]
pub struct DenyLists {
    paths: never_sync::List,
    origins: Vec<String>,
}

impl DenyLists {
    /// Parses the texts of the list files.
    pub fn parse<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        let mut paths = String::new();
        let mut origins = Vec::new();
        for text in texts {
            for line in text.lines() {
                let t = line.trim();
                if let Some(p) = t.strip_prefix("origin:") {
                    if !p.is_empty() {
                        origins.push(p.to_string());
                    }
                } else if t.starts_with('/') {
                    paths.push_str(t);
                    paths.push('\n');
                }
            }
        }
        Self {
            paths: never_sync::List::parse(&paths).0,
            origins,
        }
    }

    /// Reads the files at `files`: a missing file is an empty list.
    pub fn load(files: &[PathBuf]) -> Result<Self, String> {
        let mut texts = Vec::new();
        for f in files {
            match std::fs::read_to_string(f) {
                Ok(t) => texts.push(t),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", f.display())),
            }
        }
        Ok(Self::parse(texts.iter().map(String::as_str)))
    }

    /// Whether the lists name nothing.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.origins.is_empty()
    }

    /// Whether `s` is denied: its fetch URL matches an `origin:` pattern,
    /// or it is a `file` URL whose path lies under a listed prefix.
    pub fn denies(&self, s: &Source) -> bool {
        let url = fetch_url(&s.url);
        if self.origins.iter().any(|p| glob_match(p, url)) {
            return true;
        }
        matches!(url_parts(url), Some((scheme, _, path)) if scheme == "file"
            && self.paths.denies(Path::new(&path)).is_some())
    }
}

/// Why `sources` keep a job here: the first one `lists` deny.
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

/// Why the job in `cwd` of the checkout at `top` stays here for a git
/// dependency, against the lists this machine configures: the never-sync
/// list (`never_sync::path_from`) and `remote-deny-origins`. Reads no lock
/// file when both name nothing. A list that cannot be read keeps the job
/// here.
pub fn of_job(top: Option<&Path>, cwd: &Path) -> Option<String> {
    let mut files = vec![origins_path()];
    if let Ok(p) = never_sync::path_from(crate::hostconfig::client_path().as_deref()) {
        files.insert(0, p);
    }
    let lists = match DenyLists::load(&files) {
        Ok(l) => l,
        Err(e) => return Some(format!("{REASON_PREFIX}list unreadable ({e})")),
    };
    if lists.is_empty() {
        return None;
    }
    let top = top.unwrap_or(cwd);
    let locks = locked(&lock_files(top, cwd));
    denied(locks.iter().flat_map(|(_, s)| s), &lists)
}

/// This machine's cargo home: `CARGO_HOME`, else `~/.cargo`, made absolute.
pub fn cargo_home() -> PathBuf {
    let home = std::env::var_os("CARGO_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || {
                std::env::var_os("HOME")
                    .map_or_else(|| PathBuf::from("."), PathBuf::from)
                    .join(".cargo")
            },
            PathBuf::from,
        );
    if home.is_absolute() {
        home
    } else {
        std::env::current_dir().unwrap_or_default().join(home)
    }
}

/// The cargo git database under `cargo_home`.
pub fn db_dir(cargo_home: &Path) -> PathBuf {
    cargo_home.join("git").join("db")
}

/// Whether the git database at `dir` holds commit `rev`.
pub fn has_rev(dir: &Path, rev: &str) -> bool {
    dir.is_dir()
        && Command::new("git")
            .arg("--git-dir")
            .arg(dir)
            .args(["cat-file", "-e", &format!("{rev}^{{commit}}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// The entries of database `db` for `s` that hold its revision.
pub fn entries(db: &Path, s: &Source) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(db)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            n.rsplit_once('-').is_some_and(|(name, hash)| {
                name.eq_ignore_ascii_case(&s.name)
                    && !hash.is_empty()
                    && hash.bytes().all(|b| b.is_ascii_hexdigit())
            })
        })
        .filter(|n| valid_entry(n) && has_rev(&db.join(n), &s.rev))
        .collect();
    out.sort();
    out
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

/// The database entries the lock files `locks` name, each holding its
/// revision in this machine's database `db`. A lock file with a revision
/// missing there gets one `cargo fetch --locked` here with `cargo` (the
/// job's cargo and toolchain), outside the pool and with this machine's
/// credentials. The second value says what could not be had.
pub fn prepare(
    locks: &[(PathBuf, Vec<Source>)],
    db: &Path,
    cargo: &[String],
    deadline: Instant,
) -> (Vec<Wanted>, Vec<String>) {
    let mut wanted = Vec::new();
    let mut notes = Vec::new();
    for (dir, sources) in locks {
        let found = |s: &Source| entries(db, s);
        let mut have: Vec<Vec<String>> = sources.iter().map(found).collect();
        let mut fetched = false;
        if have.iter().any(Vec::is_empty) {
            let (prog, args) = cargo
                .split_first()
                .map_or(("cargo", &[][..]), |(p, a)| (p.as_str(), a));
            let mut c = Command::new(prog);
            c.args(args)
                .args(["fetch", "--locked"])
                .current_dir(dir)
                .env("SLOT_SHIM_OFF", "1")
                .env_remove("SLOT_HOST")
                .env_remove("CARGO_NET_OFFLINE");
            match run_bounded(&mut c, deadline) {
                Ok((true, _, _)) => fetched = true,
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
        if fetched {
            have = sources.iter().map(found).collect();
        }
        for (s, e) in sources.iter().zip(have) {
            if e.is_empty() && fetched {
                notes.push(format!("{}#{} is not in {}", s.url, s.rev, db.display()));
            }
            wanted.extend(e.into_iter().map(|entry| Wanted {
                entry,
                rev: s.rev.clone(),
            }));
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

/// What the node's step prints: `db <dir>`, then `missing <entry>` for each
/// wanted entry of database `db` that does not hold its revision.
pub fn node_answer(db: &Path, wanted: &[Wanted]) -> String {
    let mut out = format!("db {}\n", db.display());
    for w in wanted {
        if !has_rev(&db.join(&w.entry), &w.rev) {
            let _ = writeln!(out, "missing {}", w.entry);
        }
    }
    out
}

/// The node's database directory and its missing entries, from its answer.
pub fn parse_answer(text: &str) -> Option<(String, Vec<String>)> {
    let mut lines = text.lines();
    let db = lines.next()?.strip_prefix("db ")?.to_string();
    if !Path::new(&db).is_absolute() {
        return None;
    }
    let missing = lines
        .filter_map(|l| l.strip_prefix("missing "))
        .filter(|e| valid_entry(e))
        .map(String::from)
        .collect();
    Some((db, missing))
}

/// Asks the node which of `wanted` it lacks.
fn ask_node(
    r: &sync::Remote<'_>,
    wanted: &[Wanted],
    deadline: Instant,
) -> Result<(String, Vec<String>), String> {
    let (ssh, opts) = r.ssh.split_first().ok_or("no ssh command")?;
    let mut c = Command::new(ssh);
    c.args(opts)
        .arg(r.host)
        .arg(r.remote_slot)
        .arg("__cargo-git")
        .args(probe_words(wanted));
    let (ok, out, tail) = run_bounded(&mut c, deadline)?;
    if !ok {
        return Err(format!(
            "{} did not say which it holds (an older slot there?): {tail}",
            r.host
        ));
    }
    parse_answer(&out).ok_or_else(|| format!("{} gave no database directory", r.host))
}

/// Brings the git dependencies of the checkout at `top` to the node of
/// `r`: the line to print when something was copied, `Ok(None)` when
/// nothing had to be, `Err` with what went wrong. `argv` names the cargo
/// whose fetch fills this machine's database.
pub fn carry(
    top: &Path,
    cwd: &Path,
    argv: &[String],
    r: &sync::Remote<'_>,
    b: &sync::Bounds,
) -> Result<Option<String>, String> {
    let locks = locked(&lock_files(top, cwd));
    if locks.is_empty() {
        return Ok(None);
    }
    let t0 = Instant::now();
    let deadline = t0 + LIMIT;
    let db = db_dir(&cargo_home());
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
    let (wanted, notes) = prepare(&locks, &db, &cargo, deadline);
    for n in &notes {
        eprintln!("slot: git dependency not here: {n}");
    }
    if wanted.is_empty() {
        return Ok(None);
    }
    let (node_db, missing) = ask_node(r, &wanted, deadline)?;
    if missing.is_empty() {
        return Ok(None);
    }
    for entry in &missing {
        sync::copy_dir(&db.join(entry), &format!("{node_db}/{entry}"), r, b)
            .map_err(|e| format!("{entry}: {e:#}"))?;
    }
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
        let got = parse_lock(&text);
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
        assert_eq!(parse_lock("not toml ["), Vec::new());
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
    fn either_file_holds_paths_and_origin_patterns() {
        let lists = DenyLists::parse([
            "# customer data\n/srv/private/\norigin:*git.example.org[:/]team/*\nnot a rule\norigin:\n",
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
        assert!(DenyLists::parse(["# nothing\nrelative/path\norigin:\n"]).is_empty());
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
        assert_eq!(
            parse_answer("db /n/.cargo/git/db\nmissing dep-1\nmissing ../x\n"),
            Some(("/n/.cargo/git/db".into(), vec!["dep-1".into()]))
        );
        assert_eq!(parse_answer("db relative\n"), None);
        assert_eq!(parse_answer(""), None);
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
    fn an_entry_counts_only_when_it_holds_the_revision() {
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
        git(
            t.path(),
            &["clone", "-q", "--bare", "w", "db/dep-00ff00ff00ff00ff"],
        );
        git(
            t.path(),
            &["init", "-q", "--bare", "db/dep-11ee11ee11ee11ee"],
        );
        git(
            t.path(),
            &["clone", "-q", "--bare", "w", "db/depot-22dd22dd22dd22dd"],
        );
        let s = Source::parse(&format!("git+file:///f/dep.git#{rev}")).unwrap();
        assert_eq!(entries(&db, &s), ["dep-00ff00ff00ff00ff"]);
        let w = |e: &str| Wanted {
            entry: e.into(),
            rev: rev.clone(),
        };
        assert_eq!(
            node_answer(
                &db,
                &[
                    w("dep-00ff00ff00ff00ff"),
                    w("dep-11ee11ee11ee11ee"),
                    w("dep-33")
                ]
            ),
            format!(
                "db {}\nmissing dep-11ee11ee11ee11ee\nmissing dep-33\n",
                db.display()
            )
        );
    }
}
