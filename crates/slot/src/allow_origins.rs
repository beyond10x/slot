//! The allow list of remote mode: the remotes whose trees may reach a build
//! node.
//!
//! The deny lists (`never_sync.rs`, `git_deps.rs`) name what must stay
//! here, and so only what someone thought of in advance: a repository with
//! no remote matches no `origin:` pattern. The allow list turns that
//! around. Its path is `allow_origins_file` of the host configuration
//! (`cluster.toml`), else `$XDG_CONFIG_HOME/slot/remote-allow-origins`; one
//! shell pattern per line (`case` semantics, without regard to case; an
//! `origin:` before it is allowed), `#` starts a comment.
//!
//! When the file exists, a tree goes to a node only when one of its own
//! remotes (`remote.<name>.url` of the repository's configuration) matches
//! a line in one of the forms the deny check knows without a lookup (as
//! written, normalised, bare: `git_deps::forms`), and no `origin:` pattern
//! of the deny lists matches one of its remotes. A tree with no remote, or
//! no git at all, stays here. The client checks the job's own tree and
//! every tree `.slot.toml` declares; a refused tree runs here with
//! `local_reason` `never sync: no allowed remote (<tree>)`. Without the
//! file nothing changes. A file that cannot be read keeps every job here.

use std::path::{Path, PathBuf};

use crate::git_deps;
use crate::never_sync::REASON_PREFIX;

/// File name of the default list in slot's configuration directory.
pub const FILE_NAME: &str = "remote-allow-origins";

/// The reason of a refused tree, before the tree.
pub const NO_ALLOWED_REMOTE: &str = "never sync: no allowed remote";

/// Default path of the list: `$XDG_CONFIG_HOME/slot/remote-allow-origins`.
pub fn default_path() -> PathBuf {
    crate::hostconfig::config_dir().join(FILE_NAME)
}

/// The list's path: `allow_origins_file` of the host configuration file at
/// `cluster` (`None`: no file is read), else [`default_path`].
pub fn path_from(cluster: Option<&Path>) -> Result<PathBuf, String> {
    crate::never_sync::file_from(cluster, "allow_origins_file", default_path)
}

/// The parsed list: patterns, in lower case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct List {
    patterns: Vec<String>,
}

impl List {
    /// Parses the file's text: each line that is not empty or a comment is
    /// a pattern, with an `origin:` before it taken off.
    pub fn parse(text: &str) -> Self {
        let patterns = text
            .lines()
            .filter_map(|line| {
                let p =
                    git_deps::origin_pattern(line).unwrap_or_else(|| git_deps::uncommented(line));
                (!p.is_empty()).then(|| p.to_lowercase())
            })
            .collect();
        Self { patterns }
    }

    /// The list at `path`; `None` when there is no file.
    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(Self::parse(&text))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Whether a pattern matches `url` in one of its forms: as written,
    /// normalised and bare (`git_deps::forms`, no lookup); only as written
    /// when it cannot be taken apart.
    pub fn allows(&self, url: &str) -> bool {
        let forms = git_deps::forms(url, &[]).unwrap_or_else(|| vec![url.to_string()]);
        forms.iter().any(|f| {
            let lower = f.to_lowercase();
            self.patterns
                .iter()
                .any(|p| git_deps::glob_match(p, &lower))
        })
    }
}

/// Why the tree at `tree` (the root of a checkout; `None` for no git) may
/// not leave this machine under `list`, without the tree: none of its own
/// remotes is allowed ([`NO_ALLOWED_REMOTE`]), or one is on the deny lists
/// `deny` (`never sync: origin <url>`).
pub fn check_with(
    list: &List,
    tree: Option<&Path>,
    deny: &git_deps::DenyLists,
    lk: &git_deps::Lookup,
) -> Option<String> {
    let Some(tree) = tree else {
        return Some(NO_ALLOWED_REMOTE.into());
    };
    let urls = git_deps::local_remote_urls(tree);
    if !urls.iter().any(|u| list.allows(u)) {
        return Some(NO_ALLOWED_REMOTE.into());
    }
    git_deps::remote_denied(tree, deny, lk)
}

/// [`check_with`] against the lists this machine configures: `None` when
/// there is no allow list. A list that cannot be read keeps the tree here.
pub fn refusal(tree: Option<&Path>) -> Option<String> {
    let list =
        match path_from(crate::hostconfig::client_path().as_deref()).and_then(|p| List::load(&p)) {
            Ok(Some(l)) => l,
            Ok(None) => return None,
            Err(e) => return Some(format!("{REASON_PREFIX}allow list unreadable ({e})")),
        };
    let deny = match git_deps::lists() {
        Ok(d) => d,
        Err(e) => return Some(format!("{REASON_PREFIX}list unreadable ({e})")),
    };
    check_with(&list, tree, &deny, &git_deps::Lookup::here())
}

/// [`refusal`] of the job's own tree `top` (its checkout's root; `None`
/// outside git, where `cwd` is the tree), with the tree named:
/// `never sync: no allowed remote (<tree>)`.
pub fn of_job(top: Option<&Path>, cwd: &Path) -> Option<String> {
    refusal(top).map(|why| format!("{why} ({})", top.unwrap_or(cwd).display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    fn checkout(root: &Path, name: &str, remotes: &[&str]) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        for (i, url) in remotes.iter().enumerate() {
            git(&dir, &["remote", "add", &format!("r{i}"), url]);
        }
        dir
    }

    fn lookup() -> git_deps::Lookup {
        git_deps::Lookup {
            git: vec!["git".into()],
            ssh: vec!["true".into()],
            limit: std::time::Duration::from_secs(10),
            cargo_home: Ok(PathBuf::from("/nonexistent")),
        }
    }

    #[test]
    fn the_file_takes_patterns_comments_and_origin_lines() {
        let list = List::parse(
            "# forges\n*git.example.org[:/]team/*\n\n  ORIGIN: *Forge.example/o/* # second\n#x\n",
        );
        assert_eq!(
            list.patterns,
            ["*git.example.org[:/]team/*", "*forge.example/o/*"]
        );
    }

    #[test]
    fn a_pattern_matches_any_form_of_a_url_without_regard_to_case() {
        let list = List::parse("*git.example.org[:/]team/*\nhttps://forge.example/o/r\n");
        for yes in [
            "https://git.example.org/team/a.git",
            "git@GIT.example.org:team/a.git",
            "ssh://git@git.example.org:2222/team/a",
            "git@forge.example:o/r.git",
        ] {
            assert!(list.allows(yes), "{yes}");
        }
        for no in [
            "https://git.example.org/teamwork/a.git",
            "https://forge.example/o/rx",
            "/srv/git/a.git",
        ] {
            assert!(!list.allows(no), "{no}");
        }
        assert!(!List::default().allows("https://git.example.org/team/a"));
    }

    #[test]
    fn a_tree_needs_an_allowed_remote_of_its_own_and_no_denied_one() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let list = List::parse("*git.example.org[:/]team/*\n");
        let none = git_deps::DenyLists::default();
        let lk = lookup();
        let bare = checkout(&root, "bare", &[]);
        let other = checkout(&root, "other", &["https://elsewhere.example/x/y.git"]);
        let mixed = checkout(
            &root,
            "mixed",
            &[
                "https://elsewhere.example/x/y.git",
                "git@git.example.org:team/a.git",
            ],
        );
        let reason = Some(NO_ALLOWED_REMOTE.to_string());
        assert_eq!(check_with(&list, None, &none, &lk), reason, "no git");
        assert_eq!(check_with(&list, Some(&bare), &none, &lk), reason);
        assert_eq!(check_with(&list, Some(&other), &none, &lk), reason);
        assert_eq!(check_with(&list, Some(&mixed), &none, &lk), None);
        let deny = git_deps::DenyLists::parse(["origin:*elsewhere.example/*\n"]);
        assert_eq!(
            check_with(&list, Some(&mixed), &deny, &lk),
            Some("never sync: origin https://elsewhere.example/x/y.git".into())
        );
        let folder = root.join("folder");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(check_with(&list, Some(&folder), &none, &lk), reason);
    }

    #[test]
    fn the_host_configuration_names_the_list_and_no_file_is_no_list() {
        let t = tempfile::tempdir().unwrap();
        let cluster = t.path().join("cluster.toml");
        assert_eq!(path_from(Some(&cluster)), Ok(default_path()));
        std::fs::write(&cluster, "allow_origins_file = \"/etc/slot/allow\"\n").unwrap();
        assert_eq!(
            path_from(Some(&cluster)),
            Ok(PathBuf::from("/etc/slot/allow"))
        );
        std::fs::write(&cluster, "allow_origins_file = \"allow\"\n").unwrap();
        assert!(path_from(Some(&cluster)).unwrap_err().contains("absolute"));
        let (nodes, _) = crate::hostconfig::parse_all(
            "allow_origins_file = \"/etc/slot/allow\"\n[remote.a]\nssh = \"ssh a\"\n",
        )
        .unwrap();
        assert_eq!(nodes.len(), 1, "the daemon accepts the key");
        assert_eq!(List::load(&t.path().join("none")), Ok(None));
        assert!(List::load(t.path()).is_err(), "a directory is not a list");
    }
}
