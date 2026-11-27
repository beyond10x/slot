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
//! An entry may name its path through a variable the gate reads too
//! (`${SPECS_DIR:-../../specs}`), expanded with the variables the job takes
//! to the node ([`expand`]).
//!
//! The node gets each tree as it is on this machine. Its remotes are not
//! reached from the node: a `git fetch` there fails without credentials.

use std::collections::HashMap;
use std::hash::BuildHasher;
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
/// entry stays), each entry expanded first ([`expand`]) with the job's
/// variables `vars` and the client's home `home`; canonicalised, in order
/// and without repeats. An error names the first one that does not expand,
/// does not exist, cannot be read or is no directory, with its entry when
/// that differs from the path.
pub fn resolve<S: BuildHasher>(
    top: &Path,
    declared: &[String],
    vars: &HashMap<String, String, S>,
    home: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in declared {
        let expanded = expand(d, vars, home)?;
        let p = never_sync::lexical(&top.join(&expanded));
        let named = if expanded == *d {
            p.display().to_string()
        } else {
            format!("{} (`{d}`)", p.display())
        };
        let real = p.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "declared tree {named} does not exist ({} `trees`)",
                    repo_config::FILE
                )
            } else {
                format!("declared tree {named} cannot be read: {e}")
            }
        })?;
        if !real.is_dir() {
            return Err(format!(
                "declared tree {named} is not a directory ({} `trees`)",
                repo_config::FILE
            ));
        }
        if !out.contains(&real) {
            out.push(real);
        }
    }
    Ok(out)
}

/// The entry `entry` of `trees` with a leading `~/` replaced by the home
/// `home`, and each `${NAME}` and `${NAME:-default}` by the value of `NAME`
/// in `vars` (`default` when it is unset or empty, as in POSIX sh). Nothing
/// else is expanded: `$NAME` without braces, a `~` elsewhere and the text
/// of a default stay as they are. An error names the entry: an unset
/// `${NAME}` without a default, a `${` that is not one of the two forms, a
/// `~/` without a home, an entry that expands to nothing.
pub fn expand<S: BuildHasher>(
    entry: &str,
    vars: &HashMap<String, String, S>,
    home: Option<&Path>,
) -> Result<String, String> {
    let bad = |why: String| {
        format!(
            "declared tree `{entry}`: {why} ({} `trees`)",
            repo_config::FILE
        )
    };
    let mut out = String::new();
    let mut rest = entry;
    if let Some(after) = entry.strip_prefix("~/") {
        let home = home.ok_or_else(|| bad("there is no home for `~/`".into()))?;
        out.push_str(&home.to_string_lossy());
        out.push('/');
        rest = after;
    }
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let inner = &rest[at + 2..];
        let end = inner
            .find('}')
            .ok_or_else(|| bad("a `${` is not closed".into()))?;
        let (name, default) = match inner[..end].split_once(":-") {
            Some((n, d)) => (n, Some(d)),
            None => (&inner[..end], None),
        };
        let valid = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(bad(format!(
                "`${{{}}}` is neither `${{NAME}}` nor `${{NAME:-default}}`",
                &inner[..end]
            )));
        }
        match (vars.get(name).filter(|v| !v.is_empty()), default) {
            (Some(v), _) => out.push_str(v),
            (None, Some(d)) => out.push_str(d),
            (None, None) if vars.contains_key(name) => {}
            (None, None) => {
                return Err(bad(format!(
                    "{name} is not set in the job's environment (a node gets the variables \
                     its toolchain sends and those SLOT_ENV names)"
                )));
            }
        }
        rest = &inner[end + 1..];
    }
    out.push_str(rest);
    if out.is_empty() {
        return Err(bad("it expands to an empty path".into()));
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

/// The declared trees of the checkout at `top`, expanded with the job's
/// variables `vars` and the client's home `home` ([`resolve`]), each
/// checked ([`refusal`]) against the same-path root `root`. A `.slot.toml`
/// that does not parse declares none (the client reports it where it reads
/// `local`).
pub fn check<S: BuildHasher>(
    top: &Path,
    root: Option<&Path>,
    vars: &HashMap<String, String, S>,
    home: Option<&Path>,
) -> Result<Vec<PathBuf>, Problem> {
    let Ok(config) = RepoConfig::load(top) else {
        return Ok(Vec::new());
    };
    let trees = resolve(top, &config.trees, vars, home).map_err(Problem::Missing)?;
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
        let none = HashMap::new();
        let got = resolve(&top, &["../../specs".into(), abs], &none, None).unwrap();
        assert_eq!(got, [root.join("specs")]);
        let e = resolve(&top, &["../nope".into()], &none, None).unwrap_err();
        assert!(
            e.contains(&format!("{} does not exist", root.join("g/nope").display())),
            "{e}"
        );
        let e = resolve(&top, &["../../file".into()], &none, None).unwrap_err();
        assert!(e.contains("is not a directory"), "{e}");
    }

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn an_entry_follows_the_variable_its_default_and_the_home() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let top = root.join("app");
        std::fs::create_dir_all(&top).unwrap();
        std::fs::create_dir_all(root.join("specs")).unwrap();
        std::fs::create_dir_all(root.join("checkout/specs")).unwrap();
        let entry = ["${SPECS_DIR:-../specs}".to_string()];
        let other = root.join("checkout/specs");
        let want = vec![other.clone()];
        let set = vars(&[("SPECS_DIR", other.to_str().unwrap())]);
        assert_eq!(resolve(&top, &entry, &set, None).unwrap(), want);
        assert_eq!(
            resolve(&top, &entry, &vars(&[]), None).unwrap(),
            [root.join("specs")]
        );
        assert_eq!(
            resolve(&top, &entry, &vars(&[("SPECS_DIR", "")]), None).unwrap(),
            [root.join("specs")]
        );
        let rel = vars(&[("SPECS_DIR", "../checkout/specs")]);
        assert_eq!(resolve(&top, &entry, &rel, None).unwrap(), want);
        assert_eq!(
            resolve(&top, &["~/checkout/specs".into()], &vars(&[]), Some(&root)).unwrap(),
            want
        );
        let e = resolve(&top, &["${SPECS_DIR}".into()], &vars(&[]), None).unwrap_err();
        assert!(e.contains("SPECS_DIR is not set"), "{e}");
        assert!(e.contains("${SPECS_DIR}"), "{e}");
        let e = resolve(&top, &entry, &vars(&[("SPECS_DIR", "/nope")]), None).unwrap_err();
        assert!(
            e.contains("declared tree /nope (`${SPECS_DIR:-../specs}`) does not exist"),
            "{e}"
        );
        let e = resolve(&top, &["~/specs".into()], &vars(&[]), None).unwrap_err();
        assert!(e.contains("no home"), "{e}");
    }

    #[test]
    fn only_braced_variables_and_a_leading_tilde_are_expanded() {
        let home = Path::new("/h");
        let v = vars(&[("A", "x"), ("E", "")]);
        let ok = |s: &str| expand(s, &v, Some(home)).unwrap();
        assert_eq!(ok("$A/b"), "$A/b");
        assert_eq!(ok("a/~/b"), "a/~/b");
        assert_eq!(ok("~"), "~");
        assert_eq!(ok("~/${A}/y"), "/h/x/y");
        assert_eq!(ok("${A}${A:-d}"), "xx");
        assert_eq!(ok("${E:-d}/${B:-../s}"), "d/../s");
        assert_eq!(ok("${B:-~/s}"), "~/s");
        assert_eq!(ok("${E}x"), "x");
        for bad in ["${A", "${}", "${1A}", "${A-x}", "${A:=x}", "${A B}"] {
            let e = expand(bad, &v, Some(home)).unwrap_err();
            assert!(e.contains(bad), "{bad}: {e}");
        }
        let e = expand("${E}", &v, Some(home)).unwrap_err();
        assert!(e.contains("empty"), "{e}");
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
