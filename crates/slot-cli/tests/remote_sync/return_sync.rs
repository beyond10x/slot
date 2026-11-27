//! The return sync: after a node job, what the job wrote in its tree comes
//! back to this machine's tree, and what it deleted there is deleted here
//! when this machine's copy is still the one that was sent. The node is
//! [`forced_ssh`] on this machine with the tree mirrored under its workspace
//! root, so the node's tree and this one are two directories.

use super::{Daemon, forced_ssh, git, remote_call_env};
use std::path::{Path, PathBuf};

/// A repository `a` below `root` with a few small files, committed.
fn tree(root: &Path) -> PathBuf {
    let a = root.join("a");
    for (path, text) in [
        ("out/stale.txt", "stale\n"),
        ("out/kept.txt", "kept\n"),
        ("gone/x.txt", "x\n"),
        ("keep.txt", "keep\n"),
        ("node_modules/dep.js", "dep\n"),
    ] {
        let p = a.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    git(&a, &["init", "-q"]);
    git(&a, &["add", "."]);
    git(&a, &["commit", "-qm", "tree"]);
    a
}

/// The lines of `err` that the return sync printed.
fn back_lines(err: &str) -> Vec<&str> {
    err.lines()
        .filter(|l| l.starts_with("slot: return sync"))
        .collect()
}

/// A job that deletes a file and a directory that were sent, writes a new
/// file, and writes and deletes under `target`, `node_modules` and `.git`:
/// the deletions and the new file reach this tree, nothing under the three
/// exclusions does, and one line counts it.
#[test]
fn a_file_the_job_deleted_is_deleted_here() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let a = tree(&root);
    let ssh = forced_ssh(&root, &d, "");
    let job = "rm out/stale.txt && rm -r gone && echo new > out/new.txt \
               && mkdir -p target .git && echo t > target/t && echo g > .git/g \
               && rm node_modules/dep.js";
    let out = remote_call_env(&d, &ssh, &a, &["sh", "-c", job], &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(
        !a.join("out/stale.txt").exists(),
        "a deleted file stayed: {err}"
    );
    assert!(
        !a.join("gone").exists(),
        "a deleted directory stayed: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(a.join("out/new.txt"))
            .ok()
            .as_deref(),
        Some("new\n"),
        "a written file did not come back: {err}"
    );
    assert!(a.join("out/kept.txt").exists(), "{err}");
    assert!(a.join("keep.txt").exists(), "{err}");
    assert!(!a.join("target").exists(), "target came back: {err}");
    assert!(!a.join(".git/g").exists(), ".git came back: {err}");
    assert!(
        a.join("node_modules/dep.js").exists(),
        "node_modules was touched: {err}"
    );
    assert_eq!(
        back_lines(&err),
        ["slot: return sync from node: 1 back, 3 deleted, 0 kept"],
        "{err}"
    );
}

/// A file the job deleted that changed here after the sync is kept, and
/// named on a line of its own.
#[test]
fn a_file_changed_here_after_the_sync_is_kept_and_named() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let a = tree(&root);
    let ssh = forced_ssh(&root, &d, "");
    // The node is this machine: the job changes this tree's copy itself,
    // after the sync and before the return.
    let here = a.join("keep.txt");
    let job = format!(
        "rm keep.txt out/stale.txt && printf 'changed\\n' >> '{}'",
        here.display()
    );
    let out = remote_call_env(&d, &ssh, &a, &["sh", "-c", &job], &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(
        std::fs::read_to_string(&here).ok().as_deref(),
        Some("keep\nchanged\n"),
        "{err}"
    );
    assert!(!a.join("out/stale.txt").exists(), "{err}");
    assert_eq!(
        back_lines(&err),
        [
            "slot: return sync: kept keep.txt: changed here since the sync",
            "slot: return sync from node: 0 back, 1 deleted, 1 kept",
        ],
        "{err}"
    );
}

/// `SLOT_RETURN_SYNC=0` turns the return sync off. A host wrapper that
/// marks itself (`SLOT_SHIM_PID`) does its own, so the client leaves it,
/// unless `SLOT_RETURN_SYNC=1` asks for the client's.
#[test]
fn the_switch_and_a_host_wrapper_leave_the_return_to_others() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let a = tree(&root);
    let ssh = forced_ssh(&root, &d, "");
    let argv = ["rm", "out/stale.txt"];
    for env in [
        &[("SLOT_RETURN_SYNC", "0")][..],
        &[("SLOT_SHIM_PID", "1")][..],
    ] {
        let out = remote_call_env(&d, &ssh, &a, &argv, env);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{env:?}: {err}");
        assert!(a.join("out/stale.txt").exists(), "{env:?}: {err}");
        assert!(back_lines(&err).is_empty(), "{env:?}: {err}");
    }
    let env = [("SLOT_SHIM_PID", "1"), ("SLOT_RETURN_SYNC", "1")];
    let out = remote_call_env(&d, &ssh, &a, &argv, &env);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(!a.join("out/stale.txt").exists(), "{err}");
}

/// A node whose `slot shell` refuses the listing (one older than the return
/// sync's steps) costs one line; the job's exit and this tree stay as they
/// are.
#[test]
fn a_node_that_refuses_the_listing_costs_one_line() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let a = tree(&root);
    let refuse =
        "case \"$*\" in *'--sender -lre.'*) echo 'slot shell: refused' >&2; exit 126;; esac";
    let ssh = forced_ssh(&root, &d, refuse);
    let out = remote_call_env(&d, &ssh, &a, &["sh", "-c", "rm out/stale.txt; exit 7"], &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(7), "{err}");
    assert!(a.join("out/stale.txt").exists(), "{err}");
    let lines = back_lines(&err);
    assert_eq!(lines.len(), 1, "{err}");
    assert!(
        lines[0].starts_with(
            "slot: return sync skipped: could not list the tree on node before the job"
        ),
        "{err}"
    );
    assert!(!err.contains("refused"), "{err}");
}
