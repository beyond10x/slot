//! The sibling trees a repository's `.slot.toml` declares (`trees`) reach a
//! same-path node with the job, each at its own path, under the deny rules
//! of the job's own tree. The node is [`forced_ssh`] on this machine, so a
//! sync is seen in what ssh was asked to run.

use super::{Daemon, SAME_PATH, forced_ssh_with, git, remote_call_env, repository};
use std::path::{Path, PathBuf};

/// What ssh was asked to run, one call per line; empty when it never ran.
fn ssh_calls(root: &Path) -> String {
    std::fs::read_to_string(root.join("ssh.log")).unwrap_or_default()
}

/// A repository `specs` beside `a` with one file and an `origin`, and `a`
/// (the job's tree) whose `.slot.toml` declares it as `../specs`.
fn with_sibling(root: &Path) -> (PathBuf, PathBuf) {
    let (a, _) = repository(root);
    let specs = root.join("specs");
    std::fs::create_dir(&specs).unwrap();
    git(&specs, &["init", "-q"]);
    git(
        &specs,
        &[
            "remote",
            "add",
            "origin",
            "https://git.example.org/team/specs.git",
        ],
    );
    std::fs::write(specs.join("spec.txt"), "the spec\n").unwrap();
    std::fs::write(a.join(".slot.toml"), "trees = [\"../specs\"]\n").unwrap();
    (a, specs)
}

/// A daemon that admits same-path trees under `root`, and the ssh that
/// reaches it.
fn node(root: &Path) -> (Daemon, PathBuf) {
    let d = Daemon::start_with(&["--same-path-root".as_ref(), root.as_os_str()]);
    let ssh = forced_ssh_with(
        root,
        &d,
        "",
        &format!("--same-path-root {}", root.display()),
    );
    (d, ssh)
}

/// The rsync calls of `log` whose destination is `dir` itself.
fn synced_to<'a>(log: &'a str, dir: &Path) -> Vec<&'a str> {
    let end = format!(" {}/", dir.display());
    log.lines()
        .filter(|l| l.starts_with("rsync ") && l.ends_with(&end))
        .collect()
}

/// The declared sibling is synced to its own path on the node before the
/// job, and so is its git directory; the job's record names it.
#[test]
fn a_declared_sibling_tree_is_synced_to_its_own_path() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, specs) = with_sibling(&root);
    let (d, ssh) = node(&root);
    let argv = ["cat", "../specs/spec.txt"];
    let out = remote_call_env(&d, &ssh, &a, &argv, SAME_PATH);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "the spec\n", "{err}");
    let log = ssh_calls(&root);
    assert_eq!(
        synced_to(&log, &specs).len(),
        1,
        "sibling not synced: {log}"
    );
    assert_eq!(
        synced_to(&log, &specs.join(".git")).len(),
        1,
        "its git directory not synced: {log}"
    );
    assert!(
        super::super::wait_for(|| d.logged(&argv) == 1),
        "no record on the node"
    );
    let line = std::fs::read_to_string(d.dir.path().join("state/jobs.jsonl"))
        .unwrap()
        .lines()
        .find(|l| l.contains(&super::super::serde_like(&argv)))
        .unwrap()
        .to_string();
    assert!(
        line.contains(&format!("\"extra_trees\":[\"{}\"]", specs.display())),
        "{line}"
    );
}

/// A declared tree on the never-sync list, by its path or by an `origin:`
/// pattern of its own remote, keeps the job on this machine with a reason
/// that names the tree; nothing is synced.
#[test]
fn a_declared_tree_on_the_never_sync_list_keeps_the_job_here() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, specs) = with_sibling(&root);
    let (d, ssh) = node(&root);
    let config = d.dir.path().join("config/slot");
    std::fs::create_dir_all(&config).unwrap();
    let lists = [
        ("remote-deny", format!("{}\n", specs.display())),
        (
            "remote-deny-origins",
            "origin:*git.example.org[:/]team/*\n".to_string(),
        ),
    ];
    for (i, (file, text)) in lists.iter().enumerate() {
        for (other, _) in &lists {
            let _ = std::fs::remove_file(config.join(other));
        }
        std::fs::write(config.join(file), text).unwrap();
        let word = format!("case-{i}");
        let argv = ["echo", word.as_str()];
        let out = remote_call_env(&d, &ssh, &a, &argv, SAME_PATH);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{file}: {err}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("{word}\n"),
            "{err}"
        );
        let named = format!("declared tree {}", specs.display());
        assert!(err.contains("never sync: "), "{file}: {err}");
        assert!(err.contains(&named), "{file}: {err}");
        assert!(err.contains("running on this machine"), "{file}: {err}");
        assert!(
            super::super::wait_for(|| d.logged(&argv) == 1),
            "{file}: not on this machine's daemon"
        );
        let line = std::fs::read_to_string(d.dir.path().join("state/jobs.jsonl"))
            .unwrap()
            .lines()
            .find(|l| l.contains(&super::super::serde_like(&argv)))
            .unwrap()
            .to_string();
        assert!(line.contains("\"local_reason\":\"never sync: "), "{line}");
        assert!(line.contains(&named), "{line}");
    }
    assert_eq!(ssh_calls(&root), "", "ssh ran");
}

/// An entry `${SPECS_DIR:-../nope}` follows the variable to a checkout
/// outside its default path when the job's environment carries it to the
/// node (`SLOT_ENV`), and the job there reads it. Set here but not sent, the
/// variable is not the job's: the default applies and the remote job ends
/// with exit 3, naming the entry.
#[test]
fn a_declared_tree_follows_the_variable_the_job_carries() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, _) = with_sibling(&root);
    let specs = root.join("checkouts/specs");
    std::fs::create_dir_all(specs.parent().unwrap()).unwrap();
    std::fs::rename(root.join("specs"), &specs).unwrap();
    std::fs::write(
        a.join(".slot.toml"),
        "trees = [\"${SPECS_DIR:-../nope}\"]\n",
    )
    .unwrap();
    let (d, ssh) = node(&root);
    let dir = specs.display().to_string();
    let argv = ["sh", "-c", "cat \"$SPECS_DIR/spec.txt\""];
    let env = [
        ("SLOT_REMOTE_SAME_PATH", "1"),
        ("SPECS_DIR", dir.as_str()),
        ("SLOT_ENV", "SPECS_DIR"),
    ];
    let out = remote_call_env(&d, &ssh, &a, &argv, &env);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "the spec\n", "{err}");
    let log = ssh_calls(&root);
    assert_eq!(synced_to(&log, &specs).len(), 1, "not synced: {log}");

    let out = remote_call_env(&d, &ssh, &a, &["echo", "ran"], &env[..2]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{err}");
    assert!(
        err.contains(&root.join("nope").display().to_string()),
        "{err}"
    );
    assert!(err.contains("${SPECS_DIR:-../nope}"), "{err}");
}

/// A declared tree that does not exist ends the remote job with exit 3 and
/// its path; nothing is synced and the job does not run.
#[test]
fn a_missing_declared_tree_ends_the_remote_job_with_exit_3() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, _) = repository(&root);
    std::fs::write(a.join(".slot.toml"), "trees = [\"../nope\"]\n").unwrap();
    let (d, ssh) = node(&root);
    let out = remote_call_env(&d, &ssh, &a, &["echo", "ran"], SAME_PATH);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{err}");
    assert!(
        err.contains(&root.join("nope").display().to_string()),
        "{err}"
    );
    let log = ssh_calls(&root);
    assert!(!log.contains("rsync"), "a sync ran: {log}");
}
