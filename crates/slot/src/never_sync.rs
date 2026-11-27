//! The never-sync list of remote mode: trees whose content never leaves
//! this machine.
//!
//! The list is a file of absolute path prefixes, one per line, `#` starts a
//! comment. Its path is `never_sync_file` of the host configuration
//! (`cluster.toml`), else `$XDG_CONFIG_HOME/slot/remote-deny`. A job is
//! never synced when its working directory, its checkout's root, the
//! checkout's git directories, or a tree an argument names
//! (`--manifest-path`, `-C`, `--dir`, `--directory`) lies under a listed
//! prefix, or when the tree it would sync is the home directory or one
//! above it. The client then runs the job in this machine's pool with
//! `local_reason` `never sync: …`, which also keeps the daemon from handing
//! it to a node, whatever `SLOT_HOST` says.
//!
//! A prefix matches on whole path components (`/a/b` holds `/a/b/c`, not
//! `/a/bc`), on the path as written and on the path with its symlinks
//! resolved, both for the prefix and for the job's path. A list that cannot
//! be read keeps every job here.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// File name of the default list in slot's configuration directory.
pub const FILE_NAME: &str = "remote-deny";

/// Start of every reason this module gives.
pub const REASON_PREFIX: &str = "never sync: ";

/// Default path of the list: `$XDG_CONFIG_HOME/slot/remote-deny`, else
/// `~/.config/slot/remote-deny`.
pub fn default_path() -> PathBuf {
    crate::hostconfig::config_dir().join(FILE_NAME)
}

/// The list's path: `never_sync_file` of the host configuration file at
/// `cluster` (`None`: no file is read), else [`default_path`]. An error when
/// the file exists and does not parse, or names a relative path.
pub fn path_from(cluster: Option<&Path>) -> Result<PathBuf, String> {
    file_from(cluster, "never_sync_file", default_path)
}

/// The path the key `key` of the host configuration file at `cluster`
/// names (`None`: no file is read), else `default()`. An error when the
/// file exists and does not parse, or the key names a relative path.
pub fn file_from(
    cluster: Option<&Path>,
    key: &str,
    default: impl FnOnce() -> PathBuf,
) -> Result<PathBuf, String> {
    let Some(cluster) = cluster else {
        return Ok(default());
    };
    let text = match std::fs::read_to_string(cluster) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(default()),
        Err(e) => return Err(format!("{}: {e}", cluster.display())),
    };
    let table: toml::Table = toml::from_str(&text)
        .map_err(|e| format!("{}: {}", cluster.display(), first_line(&e.to_string())))?;
    match table.get(key) {
        None => Ok(default()),
        Some(toml::Value::String(p)) if Path::new(p).is_absolute() => Ok(PathBuf::from(p)),
        Some(v) => Err(format!(
            "{}: {key} = {v:?}: name an absolute path",
            cluster.display()
        )),
    }
}

fn first_line(s: &str) -> &str {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

/// One listed prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Prefix {
    /// As the file writes it, for the reason.
    written: String,
    /// Its lexical form and, when it differs, its form with symlinks resolved.
    forms: Vec<PathBuf>,
}

/// The parsed list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct List {
    prefixes: Vec<Prefix>,
}

impl List {
    /// Parses the file's text; the second value names each line that is
    /// not an absolute path (it is skipped).
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut prefixes = Vec::new();
        let mut skipped = Vec::new();
        for (i, line) in text.lines().enumerate() {
            // An `origin:` line is a rule for remote URLs (`git_deps.rs`),
            // not a tree.
            if crate::git_deps::origin_pattern(line).is_some() {
                continue;
            }
            let line = line.split_once('#').map_or(line, |(l, _)| l).trim();
            if line.is_empty() {
                continue;
            }
            if !Path::new(line).is_absolute() {
                skipped.push(format!("line {}: not an absolute path", i + 1));
                continue;
            }
            let written = line.trim_end_matches('/');
            let written = if written.is_empty() { "/" } else { written };
            prefixes.push(Prefix {
                written: written.to_string(),
                forms: forms(Path::new(written)),
            });
        }
        (Self { prefixes }, skipped)
    }

    /// Reads the list at `path`: no file is an empty list. Lines that are
    /// not absolute paths are named on stderr and skipped.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let (list, skipped) = Self::parse(&text);
                for s in skipped {
                    eprintln!("slot: {}: {s}; skipped", path.display());
                }
                Ok(list)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// The listed prefix `path` lies under, as written in the file.
    pub fn denies(&self, path: &Path) -> Option<&str> {
        let mine = forms(path);
        self.prefixes
            .iter()
            .find(|p| {
                p.forms
                    .iter()
                    .any(|f| mine.iter().any(|m| m.starts_with(f)))
            })
            .map(|p| p.written.as_str())
    }

    /// Whether the list names no prefix.
    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty()
    }
}

/// `p` with `.` and `..` taken out, without asking the file system.
pub fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `p` with its symlinks resolved: the longest part of it that exists,
/// resolved, followed by the rest. `None` when no part of it resolves.
fn resolved(p: &Path) -> Option<PathBuf> {
    let p = lexical(p);
    let mut base = p.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = base.canonicalize() {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return Some(lexical(&out));
        }
        rest.push(base.file_name()?.to_os_string());
        base = base.parent()?;
    }
}

/// The forms of `p` a prefix is compared in: as written (made plain) and
/// with symlinks resolved.
fn forms(p: &Path) -> Vec<PathBuf> {
    let plain = lexical(p);
    let mut out = vec![plain.clone()];
    if let Some(r) = resolved(&plain)
        && r != plain
    {
        out.push(r);
    }
    out
}

/// What of a job decides where its content goes.
#[derive(Debug, Clone, Copy)]
pub struct Job<'a> {
    /// The working directory.
    pub cwd: &'a Path,
    /// The root of its git checkout, if it has one.
    pub top: Option<&'a Path>,
    /// The checkout's git directories (a linked worktree's lie in its
    /// main repository).
    pub git_dirs: &'a [PathBuf],
    /// The command.
    pub argv: &'a [String],
}

/// Paths the arguments of `argv` name as a tree to work in:
/// `--manifest-path`, `-C`, `--dir` and `--directory`, in the separate and
/// the `=` form (`-C<dir>` too), made absolute against `cwd`.
pub fn named_paths(argv: &[String], cwd: &Path) -> Vec<PathBuf> {
    const FLAGS: &[&str] = &["--manifest-path", "-C", "--dir", "--directory"];
    let mut out = Vec::new();
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let value = if FLAGS.contains(&a.as_str()) {
            it.next().cloned()
        } else if let Some((flag, v)) = a.split_once('=')
            && FLAGS.contains(&flag)
        {
            Some(v.to_string())
        } else if let Some(v) = a.strip_prefix("-C")
            && !v.is_empty()
            && !a.starts_with("--")
        {
            Some(v.to_string())
        } else {
            None
        };
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            out.push(lexical(&cwd.join(v)));
        }
    }
    out
}

/// The root of the git checkout that holds `dir`, if any.
fn git_top(dir: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .env("LC_ALL", "C")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--path-format=absolute", "--show-toplevel"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// The trees an argument of `argv` names: each named path, the directory
/// that holds it when it is a file, and that directory's checkout root.
fn named_trees(argv: &[String], cwd: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in named_paths(argv, cwd) {
        let dir = if p.is_dir() {
            p.clone()
        } else {
            p.parent().map_or_else(|| p.clone(), Path::to_path_buf)
        };
        let top = dir.is_dir().then(|| git_top(&dir)).flatten();
        out.push(p);
        out.push(dir);
        out.extend(top);
    }
    out
}

/// Whether syncing `tree` would carry the home directory `home`: the tree
/// is `/`, `home` or a directory above it.
fn holds_home(tree: &Path, home: Option<&Path>) -> bool {
    let tree_forms = forms(tree);
    if tree_forms.iter().any(|t| t == Path::new("/")) {
        return true;
    }
    let Some(home) = home.filter(|h| h.is_absolute()) else {
        return false;
    };
    let home_forms = forms(home);
    tree_forms
        .iter()
        .any(|t| home_forms.iter().any(|h| h.starts_with(t)))
}

/// Why `job` is never synced, or `None`: a path of it under a prefix of
/// `list` (`never sync: <prefix>`), or a tree that would carry `home`
/// (`never sync: home directory`).
pub fn check(list: &List, job: &Job<'_>, home: Option<&Path>) -> Option<String> {
    let named = named_trees(job.argv, job.cwd);
    let tree = job.top.unwrap_or(job.cwd);
    let paths = std::iter::once(job.cwd)
        .chain(job.top)
        .chain(job.git_dirs.iter().map(PathBuf::as_path))
        .chain(named.iter().map(PathBuf::as_path));
    if !list.is_empty() {
        for p in paths {
            if let Some(prefix) = list.denies(p) {
                return Some(format!("{REASON_PREFIX}{prefix}"));
            }
        }
    }
    let trees =
        std::iter::once(tree).chain(named.iter().map(PathBuf::as_path).filter(|p| p.is_dir()));
    for t in trees {
        if holds_home(t, home) {
            return Some(format!("{REASON_PREFIX}home directory"));
        }
    }
    None
}

/// [`check`] against the list this machine configures and `$HOME`. A list
/// that cannot be read keeps the job here, with the reason naming why.
pub fn of_job(job: &Job<'_>) -> Option<String> {
    let list = path_from(crate::hostconfig::client_path().as_deref()).and_then(|p| List::load(&p));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match list {
        Ok(list) => check(&list, job, home.as_deref()),
        Err(e) => Some(format!("{REASON_PREFIX}list unreadable ({e})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn job<'a>(cwd: &'a Path, top: Option<&'a Path>, argv: &'a [String]) -> Job<'a> {
        Job {
            cwd,
            top,
            git_dirs: &[],
            argv,
        }
    }

    #[test]
    fn the_file_takes_absolute_prefixes_and_comments() {
        let (list, skipped) = List::parse(
            "# trees with customer data\n/srv/a/\n\n  /srv/b # second\nrelative/c\n/srv/d//\norigin:*git.example.org[:/]team/*\n",
        );
        assert_eq!(skipped, ["line 5: not an absolute path"]);
        let written: Vec<&str> = list.prefixes.iter().map(|p| p.written.as_str()).collect();
        assert_eq!(written, ["/srv/a", "/srv/b", "/srv/d"]);
    }

    #[test]
    fn a_prefix_matches_whole_components_only() {
        let (list, _) = List::parse("/srv/a/b\n");
        assert_eq!(list.denies(Path::new("/srv/a/b")), Some("/srv/a/b"));
        assert_eq!(list.denies(Path::new("/srv/a/b/c/d")), Some("/srv/a/b"));
        assert_eq!(list.denies(Path::new("/srv/a/bc")), None);
        assert_eq!(list.denies(Path::new("/srv/a")), None);
        assert_eq!(list.denies(Path::new("/srv/a/x/../b/c")), Some("/srv/a/b"));
    }

    #[test]
    fn a_prefix_matches_through_symlinks_on_either_side() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let real = root.join("real");
        std::fs::create_dir_all(real.join("tree/sub")).unwrap();
        std::os::unix::fs::symlink(&real, root.join("alias")).unwrap();
        let (by_alias, _) = List::parse(&format!("{}\n", root.join("alias/tree").display()));
        assert!(by_alias.denies(&real.join("tree/sub")).is_some());
        assert!(by_alias.denies(&real.join("tree/new-file")).is_some());
        let (by_real, _) = List::parse(&format!("{}\n", real.join("tree").display()));
        assert!(by_real.denies(&root.join("alias/tree/sub")).is_some());
        assert!(by_real.denies(&root.join("alias/other")).is_none());
    }

    #[test]
    fn arguments_that_name_a_tree_are_read_in_every_form() {
        let cwd = Path::new("/w/here");
        let got = named_paths(
            &v(&[
                "cargo",
                "build",
                "--manifest-path",
                "/x/a/Cargo.toml",
                "--manifest-path=../b/Cargo.toml",
                "-C",
                "/x/c",
                "-C/x/d",
                "--dir=/x/e",
                "--directory",
                "f",
                "--color",
                "always",
            ]),
            cwd,
        );
        let want: Vec<PathBuf> = [
            "/x/a/Cargo.toml",
            "/w/b/Cargo.toml",
            "/x/c",
            "/x/d",
            "/x/e",
            "/w/here/f",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn a_job_is_never_synced_by_its_cwd_its_tree_or_a_tree_it_names() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let denied = root.join("denied");
        let open = root.join("open");
        std::fs::create_dir_all(denied.join("sub")).unwrap();
        std::fs::create_dir_all(&open).unwrap();
        std::fs::write(denied.join("Cargo.toml"), "").unwrap();
        let (list, _) = List::parse(&format!("{}\n", denied.display()));
        let want = Some(format!("never sync: {}", denied.display()));
        let none: Vec<String> = v(&["true"]);
        assert_eq!(
            check(&list, &job(&denied.join("sub"), None, &none), None),
            want
        );
        assert_eq!(check(&list, &job(&open, Some(&denied), &none), None), want);
        let manifest = v(&[
            "cargo",
            "test",
            "--manifest-path",
            &denied.join("Cargo.toml").to_string_lossy(),
        ]);
        assert_eq!(
            check(&list, &job(&open, Some(&open), &manifest), None),
            want
        );
        let dir = v(&["make", "-C", "../denied"]);
        assert_eq!(check(&list, &job(&open, Some(&open), &dir), None), want);
        let task = v(&["task", "--dir=../denied/sub", "check"]);
        assert_eq!(check(&list, &job(&open, Some(&open), &task), None), want);
        let git_dirs = [denied.join(".git")];
        let linked = Job {
            cwd: &open,
            top: Some(&open),
            git_dirs: &git_dirs,
            argv: &none,
        };
        assert_eq!(check(&list, &linked, None), want);
        assert_eq!(check(&list, &job(&open, Some(&open), &none), None), None);
    }

    #[test]
    fn a_tree_that_would_carry_the_home_directory_is_never_synced() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let home = root.join("home/u");
        let work = home.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let list = List::default();
        let none = v(&["true"]);
        let reason = Some("never sync: home directory".to_string());
        assert_eq!(check(&list, &job(&home, None, &none), Some(&home)), reason);
        assert_eq!(
            check(
                &list,
                &job(&work, Some(&root.join("home")), &none),
                Some(&home)
            ),
            reason
        );
        assert_eq!(
            check(&list, &job(&work, Some(&work), &none), Some(&home)),
            None
        );
        assert_eq!(
            check(&list, &job(Path::new("/"), None, &none), None),
            reason
        );
        let named = v(&["make", "-C", &home.to_string_lossy()]);
        assert_eq!(
            check(&list, &job(&work, Some(&work), &named), Some(&home)),
            reason
        );
    }

    #[test]
    fn the_host_configuration_names_the_list() {
        let t = tempfile::tempdir().unwrap();
        let cluster = t.path().join("cluster.toml");
        assert_eq!(path_from(Some(&cluster)), Ok(default_path()), "no file");
        assert_eq!(path_from(None), Ok(default_path()));
        std::fs::write(&cluster, "spill_after_s = 5\n").unwrap();
        assert_eq!(path_from(Some(&cluster)), Ok(default_path()), "no key");
        std::fs::write(&cluster, "never_sync_file = \"/etc/slot/deny\"\n").unwrap();
        assert_eq!(
            path_from(Some(&cluster)),
            Ok(PathBuf::from("/etc/slot/deny"))
        );
        std::fs::write(&cluster, "never_sync_file = \"deny\"\n").unwrap();
        assert!(path_from(Some(&cluster)).unwrap_err().contains("absolute"));
        std::fs::write(&cluster, "never_sync_file = [\n").unwrap();
        assert!(path_from(Some(&cluster)).is_err());
        let (nodes, _) = crate::hostconfig::parse_all(
            "never_sync_file = \"/etc/slot/deny\"\n[remote.a]\nssh = \"ssh a\"\n",
        )
        .unwrap();
        assert_eq!(nodes.len(), 1, "the daemon accepts the key");
    }

    #[test]
    fn no_list_file_is_an_empty_list_and_an_unreadable_one_an_error() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(List::load(&t.path().join("none")), Ok(List::default()));
        assert!(List::load(t.path()).is_err(), "a directory is not a list");
    }
}
