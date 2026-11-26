//! Source mirrors on a build server, and the siblings a new one is seeded
//! from.
//!
//! Each worktree syncs into a mirror of its own, `<ws-root>/<id>/`, so the
//! first sync of a new worktree used to send the whole tree. Agents make a
//! worktree per change, and a server soon holds mirrors of the same
//! repository at nearly the same commit. Before each sync the client asks
//! the server for those siblings (`slot __siblings <ws-root> <id> <key>`);
//! the step records the mirror's repository key in
//! `<ws-root>/.repo-keys/<id>` and prints the other mirrors with the same
//! key, the most recently synced first. rsync gets each as `--copy-dest`:
//! a file the new mirror lacks is copied on the server when a sibling has it
//! unchanged, and a sibling's file is the delta base when it differs, so
//! only changed bytes cross the link.
//!
//! The repository key is a digest of the normalized `origin` URL, so every
//! clone and worktree of one repository shares it; without an `origin` it is
//! a digest of this machine's name and the common git directory.

use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

/// Most siblings one sync names (rsync takes at most 20 `--copy-dest`).
pub const MAX_SIBLINGS: usize = 4;

/// Directory under the workspace root that holds each mirror's key.
pub const KEYS_DIR: &str = ".repo-keys";

/// Whether `s` is a workspace id or a repository key: 1 to 64 lowercase hex
/// digits, so it names one entry and nothing else.
pub fn valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A git remote URL reduced to `host/path`, lower case, without scheme,
/// user, port, trailing `/` or `.git`, so the https and ssh forms of one
/// repository agree. A local path stays a path.
pub fn normalize_url(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    let u = u.strip_suffix(".git").unwrap_or(u).trim_end_matches('/');
    let (host, path) = if let Some((_, rest)) = u.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (host.split_once(':').map_or(host, |(h, _)| h), path)
    } else if let Some((authority, path)) = u
        .split_once(':')
        .filter(|(a, _)| !a.is_empty() && !a.contains('/'))
    {
        (
            authority.rsplit_once('@').map_or(authority, |(_, h)| h),
            path,
        )
    } else {
        return u.to_lowercase();
    };
    format!("{host}/{}", path.trim_start_matches('/')).to_lowercase()
}

/// The repository key of the checkout at `top`: 16 hex digits.
pub fn repo_key(top: &Path) -> String {
    let git = |args: &[&str]| {
        Command::new("git")
            .env("LC_ALL", "C")
            .arg("-C")
            .arg(top)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let name = git(&["config", "--get", "remote.origin.url"]).map_or_else(
        || {
            let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
                .unwrap_or_else(|| top.display().to_string());
            let host = nix::unistd::gethostname()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_default();
            format!("local:{host}:{common}")
        },
        |u| normalize_url(&u),
    );
    digest16(&name)
}

/// The first 8 bytes of SHA-256 of `s`, as 16 hex digits.
pub fn digest16(s: &str) -> String {
    use std::fmt::Write as _;
    Sha256::digest(s.as_bytes())
        .iter()
        .take(8)
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The server side of `slot __siblings`: records that mirror `id` holds
/// repository `key` and returns the other mirrors of `key` that exist under
/// `ws_root`, the most recently recorded first, at most [`MAX_SIBLINGS`].
pub fn siblings(ws_root: &Path, id: &str, key: &str) -> Result<Vec<String>, String> {
    if !valid_id(id) {
        return Err(format!("bad workspace id `{id}`"));
    }
    if !valid_id(key) {
        return Err(format!("bad repository key `{key}`"));
    }
    let dir = ws_root.join(KEYS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{id}.{}", std::process::id()));
    std::fs::write(&tmp, format!("{key}\n"))
        .and_then(|()| std::fs::rename(&tmp, dir.join(id)))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {e}", dir.display())
        })?;
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut found: Vec<(SystemTime, String)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == id || !valid_id(&name) || !ws_root.join(&name).is_dir() {
                return None;
            }
            let theirs = std::fs::read_to_string(e.path()).ok()?;
            (theirs.trim() == key).then_some(())?;
            Some((e.metadata().ok()?.modified().ok()?, name))
        })
        .collect();
    found.sort_by(|a, b| b.cmp(a));
    Ok(found
        .into_iter()
        .take(MAX_SIBLINGS)
        .map(|(_, name)| name)
        .collect())
}

/// Makes `<ws_root>/<id>` a symbolic link to `tree`, a same-path mirror, so
/// later syncs of the repository find it as a sibling by id. A real
/// directory there (a mirror synced without same-path) is left alone.
pub fn link(ws_root: &Path, id: &str, tree: &Path) -> Result<(), String> {
    if !valid_id(id) {
        return Err(format!("bad workspace id `{id}`"));
    }
    let at = ws_root.join(id);
    match std::fs::symlink_metadata(&at) {
        Ok(m) if m.file_type().is_symlink() => {
            if std::fs::read_link(&at).ok().as_deref() == Some(tree) {
                return Ok(());
            }
            std::fs::remove_file(&at).map_err(|e| format!("{}: {e}", at.display()))?;
        }
        Ok(_) => return Ok(()),
        Err(_) => {}
    }
    std::fs::create_dir_all(ws_root).map_err(|e| format!("{}: {e}", ws_root.display()))?;
    std::os::unix::fs::symlink(tree, &at).map_err(|e| format!("{}: {e}", at.display()))
}

/// The workspace id whose same-path link (`link`) names a tree that holds
/// `path`.
pub fn linked_id(ws_root: &Path, path: &Path) -> Option<String> {
    std::fs::read_dir(ws_root).ok()?.flatten().find_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        let to = std::fs::read_link(e.path()).ok()?;
        (valid_id(&name) && path.starts_with(&to)).then_some(name)
    })
}

/// Forgets the key of mirror `id`, when the mirror is deleted.
pub fn forget(ws_root: &Path, id: &str) {
    if valid_id(id) {
        let _ = std::fs::remove_file(ws_root.join(KEYS_DIR).join(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_https_and_ssh_forms_of_one_repository_share_a_key() {
        let same = [
            "https://github.com/Org/Repo.git",
            "https://github.com/org/repo",
            "https://user@github.com/org/repo/",
            "git@github.com:org/repo.git",
            "ssh://git@github.com:22/org/repo.git",
            "ssh://git@github.com/org/repo",
        ];
        for u in same {
            assert_eq!(normalize_url(u), "github.com/org/repo", "{u}");
        }
        assert_ne!(
            normalize_url("git@github.com:org/other.git"),
            "github.com/org/repo"
        );
        assert_eq!(normalize_url("/srv/git/repo.git"), "/srv/git/repo");
        assert_eq!(normalize_url("../repo"), "../repo");
    }

    #[test]
    fn ids_and_keys_are_short_lowercase_hex() {
        assert!(valid_id("0123abcd0123abcd"));
        for bad in [
            "",
            "..",
            "../x",
            "ABCD",
            "a/b",
            ".repo-keys",
            &"a".repeat(65),
        ] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn worktrees_of_one_origin_share_a_key_and_others_do_not() {
        let d = tempfile::tempdir().unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let st = Command::new("git")
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
            assert!(st.status.success(), "{args:?}");
        };
        let a = d.path().join("a");
        let b = d.path().join("b");
        let c = d.path().join("c");
        for r in [&a, &c] {
            std::fs::create_dir(r).unwrap();
            git(r, &["init", "-q"]);
            git(r, &["commit", "-q", "--allow-empty", "-m", "x"]);
        }
        git(
            &a,
            &["remote", "add", "origin", "git@example.invalid:o/r.git"],
        );
        git(
            &a,
            &["worktree", "add", "-q", "--detach", b.to_str().unwrap()],
        );
        let ka = repo_key(&a);
        assert!(valid_id(&ka) && ka.len() == 16, "{ka}");
        assert_eq!(repo_key(&b), ka);
        assert_ne!(repo_key(&c), ka, "no origin: a key of its own");
    }

    fn age(ws: &Path, id: &str, secs: u64) {
        let f = std::fs::File::options()
            .write(true)
            .open(ws.join(KEYS_DIR).join(id))
            .unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn siblings_are_the_other_mirrors_of_the_key_newest_first() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path();
        let key = "00000000000000aa";
        assert!(siblings(ws, "a1", key).unwrap().is_empty(), "the first");
        for id in ["a1", "a2", "a3", "a4", "a5", "b1", "c1"] {
            std::fs::create_dir_all(ws.join(id)).unwrap();
            let k = if id == "b1" { "00000000000000bb" } else { key };
            siblings(ws, id, k).unwrap();
        }
        for (id, secs) in [("a1", 50), ("a2", 40), ("a3", 30), ("a4", 20), ("a5", 10)] {
            age(ws, id, secs);
        }
        // c1 is the newest, but its mirror was deleted: never offered.
        std::fs::remove_dir(ws.join("c1")).unwrap();
        let got = siblings(ws, "d1", key).unwrap();
        assert_eq!(got, ["a5", "a4", "a3", "a2"], "newest first, at most 4");
        assert_eq!(
            std::fs::read_to_string(ws.join(KEYS_DIR).join("d1")).unwrap(),
            format!("{key}\n")
        );
        assert_eq!(siblings(ws, "d1", "00000000000000bb").unwrap(), ["b1"]);
        forget(ws, "b1");
        assert_eq!(siblings(ws, "d1", "00000000000000bb").unwrap(), [""; 0]);
        assert!(siblings(ws, "../x", key).is_err());
        assert!(siblings(ws, "d1", "../../etc").is_err());
    }

    #[test]
    fn a_same_path_mirror_is_linked_by_id_and_a_real_mirror_is_kept() {
        let t = tempfile::tempdir().unwrap();
        let ws = t.path().join("ws");
        let tree = t.path().join("trees/a");
        std::fs::create_dir_all(&tree).unwrap();
        link(&ws, "a1", &tree).unwrap();
        link(&ws, "a1", &tree).unwrap();
        assert_eq!(std::fs::read_link(ws.join("a1")).unwrap(), tree);
        assert_eq!(linked_id(&ws, &tree.join("sub")).as_deref(), Some("a1"));
        assert_eq!(linked_id(&ws, &t.path().join("trees/b")), None);
        let moved = t.path().join("trees/b");
        link(&ws, "a1", &moved).unwrap();
        assert_eq!(std::fs::read_link(ws.join("a1")).unwrap(), moved);
        std::fs::create_dir_all(ws.join("b1")).unwrap();
        link(&ws, "b1", &tree).unwrap();
        assert!(ws.join("b1").is_dir() && !ws.join("b1").is_symlink());
        assert!(link(&ws, "../x", &tree).is_err());
    }
}
