//! The project a job belongs to: the namespace of its repository's remote.
//!
//! A forge groups repositories under a namespace (`group/repo`,
//! `group/subgroup/repo`); several repositories that are worked on as one
//! project share it, and so do all worktrees of one repository. The project
//! of a job is that namespace, read from `git remote get-url origin` in the
//! job's directory. `project = "…"` in the repository's `.slot.toml` wins over
//! it. A directory without a git remote has no project (empty string); the
//! usage view then falls back to the repository name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};

use crate::repo_config::RepoConfig;

/// The namespace part of a git remote URL: every path segment but the last,
/// joined with `/`. `None` for a local path or a URL with no namespace.
pub fn namespace(url: &str) -> Option<String> {
    let path = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme == "file" {
            return None;
        }
        rest.split_once('/')?.1
    } else if url.starts_with('/') || url.starts_with('.') {
        return None;
    } else {
        // scp-like `user@host:path`
        url.split_once(':')?.1
    };
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    (segments.len() >= 2).then(|| segments[..segments.len() - 1].join("/"))
}

/// The project of a repository whose `.slot.toml` is `config` and whose
/// origin is `origin`: `.slot.toml`'s `project`, else the namespace of
/// `origin`, else empty.
pub fn project_of(config: &RepoConfig, origin: Option<&str>) -> String {
    if let Some(p) = config.project.as_deref().filter(|p| !p.is_empty()) {
        return p.to_string();
    }
    origin.and_then(namespace).unwrap_or_default()
}

static CACHE: LazyLock<Mutex<HashMap<PathBuf, String>>> = LazyLock::new(Mutex::default);

/// The project of the directory `cwd`, from git and `.slot.toml`; cached per
/// directory for the life of the process. Empty when `cwd` is not in a git
/// repository with a remote.
pub fn resolve(cwd: &Path) -> String {
    if let Some(p) = CACHE.lock().expect("project cache").get(cwd) {
        return p.clone();
    }
    let p = lookup(cwd);
    CACHE
        .lock()
        .expect("project cache")
        .insert(cwd.to_path_buf(), p.clone());
    p
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn lookup(cwd: &Path) -> String {
    if !cwd.is_dir() {
        return String::new();
    }
    let config = git(cwd, &["rev-parse", "--show-toplevel"])
        .and_then(|top| RepoConfig::load(Path::new(&top)).ok())
        .unwrap_or_default();
    let origin = git(cwd, &["remote", "get-url", "origin"]);
    project_of(&config, origin.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_namespace_is_every_segment_but_the_repository() {
        for (url, want) in [
            ("git@forge.example:group/repo.git", Some("group")),
            ("git@forge.example:group/sub/repo.git", Some("group/sub")),
            ("https://forge.example/group/repo", Some("group")),
            ("https://forge.example/group/repo.git/", Some("group")),
            (
                "ssh://git@forge.example:2222/group/sub/repo.git",
                Some("group/sub"),
            ),
            ("https://user@forge.example/group/repo.git", Some("group")),
            ("git@forge.example:repo.git", None),
            ("https://forge.example/repo", None),
            ("/srv/git/group/repo.git", None),
            ("file:///srv/git/group/repo.git", None),
            ("", None),
        ] {
            assert_eq!(namespace(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn slot_toml_names_the_project_over_the_remote() {
        let own = RepoConfig::parse("project = \"web\"").unwrap();
        assert_eq!(
            project_of(&own, Some("git@forge.example:group/repo.git")),
            "web"
        );
        let none = RepoConfig::default();
        assert_eq!(
            project_of(&none, Some("git@forge.example:group/repo.git")),
            "group"
        );
        assert_eq!(project_of(&none, Some("/srv/git/repo.git")), "");
        assert_eq!(project_of(&none, None), "");
    }

    #[test]
    fn a_directory_resolves_through_git_and_is_cached() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .stdout(Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "-q"]);
        run(&[
            "remote",
            "add",
            "origin",
            "git@forge.example:team-a/api.git",
        ]);
        let sub = repo.join("src");
        std::fs::create_dir(&sub).unwrap();
        assert_eq!(resolve(&sub), "team-a");
        run(&[
            "remote",
            "set-url",
            "origin",
            "git@forge.example:team-b/api.git",
        ]);
        assert_eq!(resolve(&sub), "team-a", "cached for the process");
        assert_eq!(resolve(&d.path().join("gone")), "");
    }
}
