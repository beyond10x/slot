//! The allow list of remote mode (`remote-allow-origins`): when the file
//! exists, a tree reaches a node only when one of its git remotes matches a
//! line of it; a tree without a remote, without git or with only other
//! remotes runs on this machine with the reason in its record. And `slot
//! nodes forget`, which removes a tree that reached a node, through the
//! node's forced command. The node is [`forced_ssh_with`] on this machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use super::{
    Daemon, SAME_PATH, SLOT, forced_ssh, forced_ssh_with, git, remote_call_env, remote_command,
    repository,
};

/// Writes `text` to `file` in the client's configuration directory.
fn config(d: &Daemon, file: &str, text: &str) {
    let dir = d.dir.path().join("config/slot");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(file), text).unwrap();
}

/// What ssh was asked to run, one call per line; empty when it never ran.
fn ssh_calls(root: &Path) -> String {
    std::fs::read_to_string(root.join("ssh.log")).unwrap_or_default()
}

/// The daemon's job log line of the job whose command is `argv`.
fn logged_line(d: &Daemon, argv: &[&str]) -> String {
    let want = super::super::serde_like(argv);
    assert!(
        super::super::wait_for(|| d.logged(argv) == 1),
        "not on the local daemon"
    );
    std::fs::read_to_string(d.dir.path().join("state/jobs.jsonl"))
        .unwrap()
        .lines()
        .find(|l| l.contains(&want))
        .unwrap()
        .to_string()
}

/// A repository with one commit and no remote.
fn no_remote(root: &Path) -> PathBuf {
    let dir = root.join("private");
    std::fs::create_dir(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("note.txt"), "private\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "one"]);
    dir
}

/// A daemon that admits same-path trees under `root`, and the ssh that
/// reaches it.
fn same_path_node(root: &Path) -> (Daemon, PathBuf) {
    let d = Daemon::start_with(&["--same-path-root".as_ref(), root.as_os_str()]);
    let ssh = forced_ssh_with(
        root,
        &d,
        "",
        &format!("--same-path-root {}", root.display()),
    );
    (d, ssh)
}

/// Runs `argv` in `cwd` with `env` and checks that it ran on this
/// machine's daemon with `reason`, and said so.
fn runs_here(
    d: &Daemon,
    ssh: &Path,
    cwd: &Path,
    argv: &[&str],
    env: &[(&str, &str)],
    reason: &str,
) {
    let out = remote_call_env(d, ssh, cwd, argv, env);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{argv:?}: {err}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{}\n", argv[1]),
        "{argv:?}: {err}"
    );
    assert!(
        err.contains(&format!(
            "slot: {reason}: the tree is not synced to node; running on this machine"
        )),
        "{argv:?}: {err}"
    );
    let line = logged_line(d, argv);
    assert!(
        line.contains(&format!("\"local_reason\":\"{reason}\"")),
        "{argv:?}: {line}"
    );
}

/// The case the list exists for: a repository with no remote, a plain
/// folder and a repository whose only remote the list does not name stay
/// on this machine with the reason; a job the daemon handed to a node ends
/// with exit 3 so that daemon queues it here. Nothing reaches the node.
#[test]
fn a_tree_without_an_allowed_remote_runs_here_with_the_reason() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (d, ssh) = same_path_node(&root);
    config(
        &d,
        "remote-allow-origins",
        "# forges\n*git.example.org[:/]team/*\n",
    );
    let private = no_remote(&root);
    let reason = format!("never sync: no allowed remote ({})", private.display());
    runs_here(&d, &ssh, &private, &["echo", "no-remote"], &[], &reason);
    runs_here(
        &d,
        &ssh,
        &private,
        &["echo", "no-remote-same-path"],
        SAME_PATH,
        &reason,
    );

    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    let reason = format!("never sync: no allowed remote ({})", folder.display());
    runs_here(
        &d,
        &ssh,
        &folder,
        &["echo", "plain-folder"],
        SAME_PATH,
        &reason,
    );

    let (other, _) = repository(&root);
    let reason = format!("never sync: no allowed remote ({})", other.display());
    runs_here(&d, &ssh, &other, &["echo", "other-remote"], &[], &reason);

    let phases = root.join("phases.json");
    let handed = [
        ("SLOT_SPILLED", "1"),
        ("SLOT_PHASES_FILE", phases.to_str().unwrap()),
    ];
    let argv = ["echo", "handed"];
    let out = remote_call_env(&d, &ssh, &private, &argv, &handed);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    let reason = format!("never sync: no allowed remote ({})", private.display());
    assert!(
        err.lines()
            .any(|l| l == format!("slot: {reason}: the tree is not synced to node")),
        "{err}"
    );
    assert_eq!(d.logged(&argv), 0, "{err}");
    assert_eq!(ssh_calls(&root), "", "ssh ran");
}

/// A repository whose remote matches a line is synced as before, in every
/// form the line may take; one that a deny entry's `origin:` pattern also
/// matches stays here.
#[test]
fn a_tree_whose_remote_is_allowed_is_synced_as_before() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, _) = repository(&root);
    let ssh = forced_ssh(&root, &d, "");
    for (i, line) in [
        "*example.invalid[:/]o/*",
        "origin:*EXAMPLE.invalid[:/]o/r",
        "ssh://example.invalid/o/r.git # the scp form, bare",
    ]
    .iter()
    .enumerate()
    {
        config(&d, "remote-allow-origins", &format!("{line}\n"));
        let word = format!("allowed-{i}");
        let out = remote_call_env(&d, &ssh, &a, &["echo", &word], &[]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{line}: {err}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{word}\n"));
        assert!(err.contains("slot: synced to node"), "{line}: {err}");
        assert!(!err.contains("never sync"), "{line}: {err}");
    }
    config(
        &d,
        "remote-deny-origins",
        "origin:*example.invalid[:/]o/r*\n",
    );
    let reason = format!(
        "never sync: origin git@example.invalid:o/r.git ({})",
        a.display()
    );
    runs_here(&d, &ssh, &a, &["echo", "denied"], &[], &reason);
}

/// A declared sibling tree whose remote the list does not name keeps the
/// job here, with the tree in the reason; named too, both are synced.
#[test]
fn a_declared_tree_without_an_allowed_remote_keeps_the_job_here() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, _) = repository(&root);
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
    let (d, ssh) = same_path_node(&root);
    config(&d, "remote-allow-origins", "*example.invalid[:/]o/*\n");
    let reason = format!(
        "never sync: no allowed remote (declared tree {})",
        specs.display()
    );
    runs_here(&d, &ssh, &a, &["echo", "sibling"], SAME_PATH, &reason);
    assert_eq!(ssh_calls(&root), "", "ssh ran");

    config(
        &d,
        "remote-allow-origins",
        "*example.invalid[:/]o/*\n*git.example.org[:/]team/*\n",
    );
    let out = remote_call_env(&d, &ssh, &a, &["cat", "../specs/spec.txt"], SAME_PATH);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "the spec\n", "{err}");
    assert!(
        ssh_calls(&root).contains(&format!(" {}/", specs.display())),
        "sibling not synced"
    );
}

/// A free loopback address.
fn free_port() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

/// A node daemon with its web view on `http` and same-path trees under
/// `root`, and the ssh that reaches it with the same `--http`.
fn web_node(root: &Path, http: &str) -> (Daemon, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let child = Command::new(SLOT)
        .env_remove("CARGO_NET_OFFLINE")
        .env("SLOT_CLUSTER_FILE", "off")
        .args(["serve", "--http", http, "--tokens", "1"])
        .args(["--cpu-quota", "false", "--call-headroom", "0"])
        .args(["--sccache-watch", "false"])
        .args(["--min-free-disk", "0", "--min-free-memory", "0"])
        .arg("--socket")
        .arg(dir.path().join("sock"))
        .arg("--state-dir")
        .arg(dir.path().join("state"))
        .arg("--same-path-root")
        .arg(root)
        .env_remove("SLOT_JOB")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let d = Daemon { child, dir };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !(d.socket().exists() && std::net::TcpStream::connect(http).is_ok()) {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let ssh = forced_ssh_with(
        root,
        &d,
        "",
        &format!("--http {http} --same-path-root {}", root.display()),
    );
    (d, ssh)
}

/// `slot nodes forget node <path>` against a cluster file whose node
/// `node` is reached through `ssh`.
fn forget(root: &Path, ssh: &Path, path: &Path) -> Output {
    let cluster = root.join("cluster.toml");
    std::fs::write(
        &cluster,
        format!(
            "[remote.node]\nssh = \"{} node\"\nslot_path = \"{SLOT}\"\n",
            ssh.display()
        ),
    )
    .unwrap();
    Command::new(SLOT)
        .args(["nodes", "forget", "node"])
        .arg(path)
        .arg("--cluster-file")
        .arg(&cluster)
        .env_remove("SLOT_JOB")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// The mirrors in the workspace root: directories, not links or dot files.
fn mirrors(ws: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(ws)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| !p.is_symlink() && p.is_dir())
                .filter(|p| {
                    !p.file_name()
                        .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `slot nodes forget` removes a mirror and a same-path tree (with the
/// workspace root's link to it), printing what it removed and the bytes;
/// it refuses `..`, a link, a root, a path outside the roots and a tree a
/// running job uses, and removes nothing then.
#[test]
fn slot_nodes_forget_removes_a_tree_and_refuses_the_rest() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let http = free_port();
    let (d, ssh) = web_node(&root, &http);
    let ws = d.dir.path().join("state/ws");
    let (a, _) = repository(&root);

    // A running job's mirror is refused.
    let started = root.join("started");
    let stop = root.join("stop");
    let body = format!(
        "touch {}; while [ ! -e {} ]; do sleep 0.1; done",
        started.display(),
        stop.display()
    );
    let mut job = remote_command(&d, &ssh, &a, &["sh", "-c", &body], &[])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(
        super::super::wait_for(|| started.exists()),
        "job did not start"
    );
    let mirror = mirrors(&ws).pop().expect("a mirror");
    let out = forget(&root, &ssh, &mirror);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("used by job"), "{err}");
    assert!(mirror.is_dir(), "removed while a job ran in it");
    std::fs::write(&stop, "").unwrap();
    assert!(job.wait().unwrap().success());

    let out = forget(&root, &ssh, &mirror);
    let (so, err) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(
        so.contains(&format!("removed {}", mirror.display())),
        "{so}"
    );
    assert!(so.contains("bytes freed"), "{so}");
    assert!(!mirror.exists(), "the mirror is still there");

    // A same-path tree, and the workspace root's link to it.
    let tree = root.join("trees/synced");
    std::fs::create_dir_all(tree.join("src")).unwrap();
    std::fs::write(tree.join("src/lib.rs"), "x".repeat(10_000)).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::os::unix::fs::symlink(&tree, ws.join("00aa")).unwrap();
    let out = forget(&root, &ssh, &tree);
    let (so, err) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(so.contains(&format!("removed {}", tree.display())), "{so}");
    assert!(!tree.exists() && !ws.join("00aa").is_symlink(), "{so}");

    // Refusals: nothing is removed.
    let kept = root.join("kept");
    std::fs::create_dir_all(kept.join("sub")).unwrap();
    std::os::unix::fs::symlink(&kept, root.join("link")).unwrap();
    for (path, why) in [
        (root.join("kept/../kept"), "`..`"),
        (root.join("link"), "symbolic link"),
        (root.join("link/sub"), "symbolic link"),
        (root.clone(), "root"),
        (ws.clone(), "root"),
        (PathBuf::from("/etc"), "outside"),
    ] {
        let out = forget(&root, &ssh, &path);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(126), "{}: {err}", path.display());
        assert!(err.contains("refused"), "{}: {err}", path.display());
        assert!(err.contains(why), "{}: {err}", path.display());
    }
    assert!(kept.join("sub").is_dir() && root.join("link").is_symlink());
}
