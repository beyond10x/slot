//! What a same-path remote job (`SLOT_REMOTE_SAME_PATH`) needs on the server
//! beside its tree, as a CI runner has it: the git directories, the working
//! directory when the tree's sync leaves it out, and the target directory
//! at the client's own path.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// How one extra path is synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A directory with the `.gitignore` filter, without its own `target/`.
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

/// `target_directory` of `cargo metadata` output.
fn target_of(json: &[u8]) -> Option<PathBuf> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    v.get("target_directory")?
        .as_str()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// The target directory a same-path job builds into, the client's own:
/// `CARGO_TARGET_DIR` (`env_target`, relative to `cwd`); for a cargo
/// command the one `cargo metadata` names for its manifest
/// (`--manifest-path` included), else `target` beside the nearest lock
/// file; `None` for any other command, which keeps cargo's default.
pub fn target_dir(
    argv: &[String],
    cwd: &Path,
    top: &Path,
    env_target: Option<&str>,
) -> Option<PathBuf> {
    if let Some(t) = env_target.filter(|t| !t.is_empty()) {
        return Some(cwd.join(t));
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
    let from = manifest
        .as_deref()
        .map(|m| cwd.join(m))
        .and_then(|m| m.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| cwd.to_path_buf());
    Some(crate::artifacts::local_target(&from, top, None))
}

/// rsync filter options of a sync of `kind`.
pub fn filters(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Tree => &["--filter=:- .gitignore", "--exclude=/target/"],
        Kind::Git => &["--exclude=*.lock"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
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
        assert_eq!(
            target_dir(&build, &top, &top, None),
            Some(top.join("target"))
        );
        assert_eq!(
            target_dir(&build, &top.join("src"), &top, Some("../out")),
            Some(top.join("src/../out"))
        );
        let nested = v(&[cargo, "build", "--manifest-path", "driver/Cargo.toml"]);
        assert_eq!(
            target_dir(&nested, &top, &top, None),
            Some(top.join("driver/target"))
        );
        std::fs::create_dir_all(top.join(".cargo")).unwrap();
        std::fs::write(
            top.join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"elsewhere\"\n",
        )
        .unwrap();
        assert_eq!(
            target_dir(&build, &top, &top, None),
            Some(top.join("elsewhere"))
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
