//! The sibling trees a repository's `.slot.toml` declares (`trees`): a
//! checkout its gate reads beside its own (`../../specs`), a crate its
//! build names with `--manifest-path ../driver/Cargo.toml`.
//!
//! In same-path remote mode the client syncs each declared tree to its own
//! path on the node before the job, with its git directories, under the
//! bounds of the job's own tree. Each passes the checks the job's own tree
//! passes, plus the `origin:` patterns of the deny lists against its own
//! remotes; one that is refused keeps the job on this machine with the
//! reason as its `local_reason` and nothing is synced. A declared tree that
//! does not exist ends a remote job (exit 3) with its path.
//!
//! The node gets each tree as it is on this machine. Its remotes are not
//! reached from the node: a `git fetch` there fails without credentials.

use std::path::{Path, PathBuf};

use crate::never_sync;
use crate::repo_config::{self, RepoConfig};
use crate::same_path;

/// Why the declared trees keep a job from going to a node as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A tree may not leave this machine: the job runs here, with this
    /// reason.
    Refused(String),
    /// A tree does not exist or is no directory: a remote job ends with
    /// this line.
    Missing(String),
}

/// The trees `declared` relative to the repository top `top` (an absolute
/// entry stays), canonicalised, in order and without repeats. An error
/// names the first one that does not exist, cannot be read or is no
/// directory.
pub fn resolve(top: &Path, declared: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in declared {
        let p = never_sync::lexical(&top.join(d));
        let real = p.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "declared tree {} does not exist ({} `trees`)",
                    p.display(),
                    repo_config::FILE
                )
            } else {
                format!("declared tree {} cannot be read: {e}", p.display())
            }
        })?;
        if !real.is_dir() {
            return Err(format!(
                "declared tree {} is not a directory ({} `trees`)",
                p.display(),
                repo_config::FILE
            ));
        }
        if !out.contains(&real) {
            out.push(real);
        }
    }
    Ok(out)
}

/// Why `tree` may not be synced for a job, or `None`: it does not lie under
/// the same-path root `root`; it is no one repository's work tree
/// (`same_path::tree_refusal`); it, its git directories or a git dependency
/// of its lock files is on a deny list, as for the job's own tree; or one
/// of its remotes matches an `origin:` pattern. The reason names the tree.
pub fn refusal(tree: &Path, root: Option<&Path>) -> Option<String> {
    let why = if same_path::applies(tree, root) {
        same_path::tree_refusal(tree, root)
            .or_else(|| {
                never_sync::of_job(&never_sync::Job {
                    cwd: tree,
                    top: Some(tree),
                    git_dirs: &same_path::git_dirs(tree),
                    argv: &[],
                })
            })
            .or_else(|| crate::git_deps::of_job(Some(tree), tree))
            .or_else(|| match crate::git_deps::lists() {
                Ok(lists) => {
                    crate::git_deps::remote_denied(tree, &lists, &crate::git_deps::Lookup::here())
                }
                Err(e) => Some(format!(
                    "{}list unreadable ({e})",
                    never_sync::REASON_PREFIX
                )),
            })
    } else {
        Some(match root {
            Some(r) => format!(
                "it lies outside the same-path root {}, so it cannot keep its path on a node",
                r.display()
            ),
            None => "no same-path root is known".into(),
        })
    }?;
    Some(format!("{why} (declared tree {})", tree.display()))
}

/// The declared trees of the checkout at `top`, each checked
/// ([`refusal`]) against the same-path root `root`. A `.slot.toml` that
/// does not parse declares none (the client reports it where it reads
/// `local`).
pub fn check(top: &Path, root: Option<&Path>) -> Result<Vec<PathBuf>, Problem> {
    let Ok(config) = RepoConfig::load(top) else {
        return Ok(Vec::new());
    };
    let trees = resolve(top, &config.trees).map_err(Problem::Missing)?;
    for t in &trees {
        if let Some(why) = refusal(t, root) {
            return Err(Problem::Refused(why));
        }
    }
    Ok(trees)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_trees_resolve_against_the_top_once_each() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let top = root.join("g/app");
        std::fs::create_dir_all(&top).unwrap();
        std::fs::create_dir_all(root.join("specs")).unwrap();
        std::fs::write(root.join("file"), "").unwrap();
        let abs = root.join("specs").display().to_string();
        let got = resolve(&top, &["../../specs".into(), abs]).unwrap();
        assert_eq!(got, [root.join("specs")]);
        let e = resolve(&top, &["../nope".into()]).unwrap_err();
        assert!(
            e.contains(&format!("{} does not exist", root.join("g/nope").display())),
            "{e}"
        );
        let e = resolve(&top, &["../../file".into()]).unwrap_err();
        assert!(e.contains("is not a directory"), "{e}");
    }

    #[test]
    fn a_tree_outside_the_same_path_root_is_refused_by_name() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let tree = root.join("specs");
        std::fs::create_dir_all(tree.join(".git")).unwrap();
        let other = root.join("elsewhere");
        let why = refusal(&tree, Some(&other)).unwrap();
        assert!(why.contains("outside the same-path root"), "{why}");
        assert!(
            why.ends_with(&format!("(declared tree {})", tree.display())),
            "{why}"
        );
    }
}
