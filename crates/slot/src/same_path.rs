//! What a same-path remote job (`SLOT_REMOTE_SAME_PATH`) needs on the server
//! beside its tree, as a CI runner has it: the git directories, the working
//! directory when the tree's sync leaves it out, and the target directory
//! at the client's own path.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// How one extra path is synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A directory without its own `target/` and its cargo build directories.
    Tree,
    /// A git directory: everything but `*.lock` files.
    Git,
}

/// A path synced to its own path on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extra {
    /// Absolute path, the same on both machines.
    pub path: PathBuf,
    /// How it is synced.
    pub kind: Kind,
}

/// Whether a tree keeps its own path on the server: only under the client's
/// home, which the server's account shares and its `slot shell` admits.
/// Any other tree (a runner's work directory elsewhere) goes to the
/// server's workspace root like a mirror.
pub fn applies(top: &Path, home: Option<&Path>) -> bool {
    home.is_some_and(|h| plain(h) && h != Path::new("/") && plain(top) && top.starts_with(h))
}

/// Why `top` may not be synced as a remote tree, if it may not. A tree is
/// one repository's work tree, or a plain folder (a scratch copy) that holds
/// no repository. Refused: `home` (the same-path root) or a directory above
/// it; a folder directly under `home` that is no work tree (a group of
/// repositories, a home-level folder); a folder that is no work tree and
/// holds a repository within two levels. A folder that cannot be read is
/// refused.
pub fn tree_refusal(top: &Path, home: Option<&Path>) -> Option<String> {
    let refuse = |why: String| {
        Some(format!(
            "{} is not one repository's work tree ({why}); nothing is synced",
            top.display()
        ))
    };
    if home.is_some_and(|h| h.starts_with(top)) {
        return refuse("it is the same-path root or above it".into());
    }
    if std::fs::symlink_metadata(top.join(".git")).is_ok() {
        return None;
    }
    if home.is_some_and(|h| top.parent() == Some(h)) {
        return refuse("it lies directly under the same-path root".into());
    }
    let mut found = Vec::new();
    if let Err(e) = repositories_within(top, Path::new(""), 2, &mut found) {
        return refuse(format!("it cannot be read: {e}"));
    }
    if found.is_empty() {
        return None;
    }
    found.sort();
    let more = found.len().saturating_sub(5);
    found.truncate(5);
    let mut names = found.join(", ");
    if more > 0 {
        names = format!("{names} and {more} more");
    }
    refuse(format!("it holds the repositories {names}"))
}

/// The directories below `dir`, `depth` levels deep at most, that hold a
/// `.git` entry, as paths relative to the first `dir`. Symbolic links are
/// not followed.
fn repositories_within(
    dir: &Path,
    rel: &Path,
    depth: usize,
    found: &mut Vec<String>,
) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if !e.file_type()?.is_dir() {
            continue;
        }
        let p = e.path();
        let r = rel.join(e.file_name());
        if std::fs::symlink_metadata(p.join(".git")).is_ok() {
            found.push(r.to_string_lossy().into_owned());
        } else if depth > 1 {
            repositories_within(&p, &r, depth - 1, found)?;
        }
    }
    Ok(())
}

/// Absolute, with no `.` or `..`.
pub fn plain(p: &Path) -> bool {
    p.is_absolute()
        && p.components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

/// The git directories of `--git-dir` and `--git-common-dir` output: the
/// common one, and the worktree's own when it lies outside it.
pub fn git_dirs_of(out: &str) -> Vec<PathBuf> {
    let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
    let (Some(git), Some(common)) = (lines.next(), lines.next()) else {
        return Vec::new();
    };
    let (git, common) = (PathBuf::from(git), PathBuf::from(common));
    if !plain(&git) || !plain(&common) {
        return Vec::new();
    }
    if git.starts_with(&common) {
        vec![common]
    } else {
        vec![common, git]
    }
}

/// The git directories of the checkout at `dir`: for a linked worktree the
/// repository's common directory, which holds the worktree's own.
pub fn git_dirs(dir: &Path) -> Vec<PathBuf> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| git_dirs_of(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Whether the working directory `cwd` lies under a path the sync of `top`
/// leaves out: one of the toolchain's `excludes` (`/target/`) or a path git
/// ignores.
pub fn cwd_ignored(top: &Path, cwd: &Path, excludes: &[&str]) -> bool {
    let Ok(rel) = cwd.strip_prefix(top) else {
        return false;
    };
    if rel.as_os_str().is_empty() {
        return false;
    }
    if excludes
        .iter()
        .map(|x| x.trim_matches('/'))
        .any(|x| !x.is_empty() && !x.contains('/') && rel.starts_with(x))
    {
        return true;
    }
    Command::new("git")
        .arg("-C")
        .arg(top)
        .args(["check-ignore", "-q", "--"])
        .arg(rel)
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `--manifest-path` of a cargo command line.
pub fn manifest_path(argv: &[String]) -> Option<String> {
    let mut it = argv.iter().skip(1).take_while(|a| *a != "--");
    while let Some(a) = it.next() {
        if a == "--manifest-path" {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix("--manifest-path=") {
            return Some(v.to_string());
        }
    }
    None
}

/// Where a same-path job builds, and whether the job needs to be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The directory, absolute unless the caller's variable is relative
    /// to a relative working directory.
    pub dir: PathBuf,
    /// cargo resolves `dir` by itself in the same tree on the server:
    /// `target` beside the workspace's root manifest, with no variable of
    /// the caller and no cargo configuration that names another. The
    /// server then exports no `CARGO_TARGET_DIR`: the compile cache keys
    /// every `CARGO_*` variable, and this one would differ per tree.
    pub default: bool,
}

/// `target_directory` of `cargo metadata` output, as a [`Target`]: the
/// default when it is `target` in `workspace_root`.
fn target_of(json: &[u8]) -> Option<Target> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let dir = v
        .get("target_directory")?
        .as_str()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())?;
    let default = v
        .get("workspace_root")
        .and_then(|r| r.as_str())
        .is_some_and(|r| Path::new(r).join("target") == dir);
    Some(Target { dir, default })
}

/// The target directory a same-path job builds into, the client's own:
/// `CARGO_TARGET_DIR` (`env_target`, relative to `cwd`); for a cargo
/// command the one `cargo metadata` names for its manifest
/// (`--manifest-path` included), else `target` beside the nearest lock
/// file; `None` for any other command, which keeps cargo's default.
///
/// This is the one decision of the directory: the record, eviction and the
/// products read it. [`Target::default`] says whether it is the directory
/// cargo finds without being told: not with a variable of the caller, and
/// not when `CARGO_BUILD_TARGET_DIR` or a `build.target-dir` of a cargo
/// configuration moved it, since a configuration outside the tree (in the
/// caller's home) does not exist on the server; nor the guess beside the
/// lock file when `cargo metadata` fails.
pub fn target_dir(
    argv: &[String],
    cwd: &Path,
    top: &Path,
    env_target: Option<&str>,
) -> Option<Target> {
    if let Some(t) = env_target.filter(|t| !t.is_empty()) {
        return Some(Target {
            dir: cwd.join(t),
            default: false,
        });
    }
    if crate::toolchain::for_argv(argv, 1).name() != "rust" {
        return None;
    }
    let manifest = manifest_path(argv);
    let cargo = crate::cargo_fetch::cargo_of(argv);
    let mut c = Command::new(&cargo[0]);
    c.args(&cargo[1..])
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .args(
            manifest
                .iter()
                .flat_map(|m| ["--manifest-path", m.as_str()]),
        )
        .current_dir(cwd)
        .env("SLOT_SHIM_OFF", "1")
        .env_remove("CARGO_TARGET_DIR")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if let Some(t) = c
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| target_of(&o.stdout))
    {
        return Some(t);
    }
    // A guess: a cargo configuration, also one in the tree, may name
    // another directory, so cargo is told this one.
    let from = manifest
        .as_deref()
        .map(|m| cwd.join(m))
        .and_then(|m| m.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| cwd.to_path_buf());
    Some(Target {
        dir: crate::artifacts::local_target(&from, top, None),
        default: false,
    })
}

/// rsync filter options of a sync of `kind`.
pub fn filters(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Tree => &["--exclude=/target/"],
        Kind::Git => &["--exclude=*.lock"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    /// A plain folder is a tree, also with a symlink to a repository in it
    /// (links are not followed); a folder whose subfolder cannot be read is
    /// refused.
    #[test]
    fn a_folder_that_cannot_be_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let top = home.join("g/scratch");
        std::fs::create_dir_all(top.join("sub")).unwrap();
        assert_eq!(tree_refusal(&top, Some(&home)), None);
        std::fs::create_dir(home.join("repo")).unwrap();
        std::fs::create_dir(home.join("repo/.git")).unwrap();
        std::os::unix::fs::symlink(home.join("repo"), top.join("link")).unwrap();
        assert_eq!(tree_refusal(&top, Some(&home)), None);
        let locked = top.join("sub");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&locked).is_ok();
        let got = tree_refusal(&top, Some(&home));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            let why = got.unwrap_or_default();
            assert!(why.contains("it cannot be read"), "{why}");
        }
    }

    #[test]
    fn only_a_tree_under_home_keeps_its_path() {
        let home = Some(Path::new("/srv/acct"));
        assert!(applies(Path::new("/srv/acct/src/slot"), home));
        assert!(!applies(Path::new("/var/runner/work/repo"), home));
        assert!(!applies(Path::new("/srv/acct2/repo"), home));
        assert!(!applies(Path::new("/srv/acct/../v/repo"), home));
        assert!(!applies(Path::new("/srv/repo"), Some(Path::new("/"))));
        assert!(!applies(Path::new("/srv/acct/repo"), None));
    }

    #[test]
    fn a_linked_worktree_syncs_the_common_git_dir() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let main = root.join("main");
        let wt = root.join("wt");
        std::fs::create_dir(&main).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
                .args(["-c", "commit.gpgSign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(ok.status.success(), "{ok:?}");
        };
        git(&main, &["init", "-q"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "x"]);
        git(
            &main,
            &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
        );
        assert_eq!(git_dirs(&wt), vec![main.join(".git")]);
        assert_eq!(git_dirs(&main), vec![main.join(".git")]);
        assert!(git_dirs(&root).is_empty(), "no checkout");
        assert_eq!(
            git_dirs_of("/srv/g/wt\n/srv/u/r/.git\n"),
            vec![PathBuf::from("/srv/u/r/.git"), PathBuf::from("/srv/g/wt")]
        );
        assert_eq!(
            git_dirs_of("/srv/u/../x\n/srv/u/r/.git\n"),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn the_target_dir_is_the_clients_own() {
        let t = tempfile::tempdir().unwrap();
        let top = t.path().canonicalize().unwrap();
        let cargo = env!("CARGO");
        std::fs::create_dir_all(top.join("src")).unwrap();
        std::fs::create_dir_all(top.join("driver/src")).unwrap();
        let pkg = |dir: &Path, name: &str| {
            std::fs::write(
                dir.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n"),
            )
            .unwrap();
            std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        };
        pkg(&top, "outer");
        pkg(&top.join("driver"), "driver");
        let build = v(&[cargo, "build"]);
        // What cargo resolves by itself is the default; a directory the
        // caller or a cargo configuration names is not.
        let at = |dir: PathBuf, default: bool| Some(Target { dir, default });
        assert_eq!(
            target_dir(&build, &top, &top, None),
            at(top.join("target"), true)
        );
        assert_eq!(
            target_dir(&build, &top.join("src"), &top, None),
            at(top.join("target"), true),
            "from a directory below the manifest"
        );
        assert_eq!(
            target_dir(&build, &top.join("src"), &top, Some("../out")),
            at(top.join("src/../out"), false)
        );
        assert_eq!(
            target_dir(&build, &top, &top, top.join("target").to_str()),
            at(top.join("target"), false),
            "the caller named it: exported, also when it is cargo's own"
        );
        assert_eq!(
            target_dir(&build, &top, &top, Some("")),
            at(top.join("target"), true),
            "an empty variable names nothing"
        );
        let nested = v(&[cargo, "build", "--manifest-path", "driver/Cargo.toml"]);
        assert_eq!(
            target_dir(&nested, &top, &top, None),
            at(top.join("driver/target"), true)
        );
        std::fs::create_dir_all(top.join(".cargo")).unwrap();
        std::fs::write(
            top.join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"elsewhere\"\n",
        )
        .unwrap();
        assert_eq!(
            target_dir(&build, &top, &top, None),
            at(top.join("elsewhere"), false)
        );
        // Without a cargo that answers, `target` beside the lock file, a
        // guess: a cargo configuration (here the one in the tree) may name
        // another directory, so it is exported, not left to cargo.
        std::fs::write(top.join("driver/Cargo.lock"), "").unwrap();
        assert_eq!(
            target_dir(
                &v(&["/nonexistent/cargo", "build"]),
                &top.join("driver/src"),
                &top,
                None
            ),
            at(top.join("driver/target"), false)
        );
        assert_eq!(target_dir(&v(&["sh", "run.sh"]), &top, &top, None), None);
        assert_eq!(
            manifest_path(&v(&["cargo", "test", "--manifest-path=a/Cargo.toml"])).as_deref(),
            Some("a/Cargo.toml")
        );
        assert_eq!(
            manifest_path(&v(&["cargo", "run", "--", "--manifest-path", "x"])),
            None
        );
    }

    #[test]
    fn a_cwd_under_target_or_an_ignored_path_is_ignored() {
        let t = tempfile::tempdir().unwrap();
        let top = t.path().canonicalize().unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&top)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(top.join(".gitignore"), "/scratch/\n").unwrap();
        std::fs::create_dir_all(top.join("scratch/a")).unwrap();
        std::fs::create_dir_all(top.join("src")).unwrap();
        let x = &["/target/"];
        assert!(cwd_ignored(&top, &top.join("target/ess-scratch/cli"), x));
        assert!(cwd_ignored(&top, &top.join("scratch/a"), x));
        assert!(!cwd_ignored(&top, &top.join("src"), x));
        assert!(!cwd_ignored(&top, &top, x));
    }
}
