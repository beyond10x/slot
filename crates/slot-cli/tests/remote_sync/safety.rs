//! What never crosses to a build server, held by the client binary itself:
//! a tree on the never-sync list, the file a symlink outside the tree points
//! to, and a variable whose name looks like a credential. Each test calls
//! the binary directly with `SLOT_HOST` already set, as a caller that goes
//! around any wrapper does. No network: the server is [`forced_ssh`].

use super::{Daemon, SLOT, forced_ssh, remote_call_env, repository, script};
use std::path::{Path, PathBuf};

/// Writes the never-sync list of the client's configuration directory.
fn never_sync(d: &Daemon, prefixes: &[&Path]) {
    let dir = d.dir.path().join("config/slot");
    std::fs::create_dir_all(&dir).unwrap();
    let lines: Vec<String> = prefixes.iter().map(|p| p.display().to_string()).collect();
    let text = format!("# test list\n{}\n", lines.join("\n"));
    std::fs::write(dir.join("remote-deny"), text).unwrap();
}

/// What ssh was asked to run, one call per line; empty when it never ran.
fn ssh_calls(root: &Path) -> String {
    std::fs::read_to_string(root.join("ssh.log")).unwrap_or_default()
}

/// Every path under `dir`, recursively, as text.
fn tree_of(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            out.push(p.display().to_string());
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
            }
        }
    }
    out
}

/// The one mirror the server holds.
fn the_mirror(d: &Daemon) -> PathBuf {
    let ws = d.dir.path().join("state/ws");
    let dirs: Vec<PathBuf> = std::fs::read_dir(&ws)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
        })
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs[0].clone()
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

/// A tree on the list is neither synced nor sent to the server, whether the
/// job starts in it, names it with `--manifest-path`, `-C` or `--dir`, or
/// is a worktree of a repository on the list; it runs on this machine's
/// daemon with the rule as its reason.
#[test]
fn a_tree_on_the_never_sync_list_is_not_copied() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, b) = repository(&root);
    let open = root.join("open");
    std::fs::create_dir(&open).unwrap();
    super::git(&open, &["init", "-q"]);
    std::fs::write(a.join("Cargo.toml"), "").unwrap();
    // The list names `a` through a symlink: the match is on resolved paths.
    std::os::unix::fs::symlink(&root, root.join("alias")).unwrap();
    let listed = root.join("alias/a");
    never_sync(&d, &[&listed]);
    let ssh = forced_ssh(&root, &d, "");
    let reason = format!("never sync: {}", listed.display());
    let manifest = format!("--manifest-path={}", a.join("Cargo.toml").display());
    let cases: [(&Path, Vec<&str>); 5] = [
        (&a, vec!["echo", "in-the-tree"]),
        (&open, vec!["echo", "manifest", manifest.as_str()]),
        (&open, vec!["echo", "dash-c", "-C", "../a"]),
        (&open, vec!["echo", "dir", "--dir", "../a"]),
        (&b, vec!["echo", "worktree-of-a"]),
    ];
    for (cwd, argv) in &cases {
        let out = remote_call_env(&d, &ssh, cwd, argv, &[]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{argv:?}: {err}");
        assert!(
            err.contains(&format!("slot: {reason}: the tree is not synced to node")),
            "{argv:?}: {err}"
        );
        let line = logged_line(&d, argv);
        assert!(
            line.contains(&format!("\"local_reason\":\"{reason}\"")),
            "{argv:?}: {line}"
        );
    }
    assert_eq!(ssh_calls(&root), "", "ssh ran");
    assert_eq!(
        tree_of(&d.dir.path().join("state/ws")),
        Vec::<String>::new()
    );

    // Without SLOT_HOST the job carries the reason too, so a daemon with a
    // node never hands it there.
    let out = d
        .client(&[SLOT, "echo", "no-host"])
        .current_dir(&a)
        .env("XDG_CONFIG_HOME", d.dir.path().join("config"))
        .env("SLOT_CLUSTER_FILE", "off")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let line = logged_line(&d, &["echo", "no-host"]);
    assert!(
        line.contains(&format!("\"local_reason\":\"{reason}\"")),
        "{line}"
    );

    // A tree beside it whose name starts the same is synced.
    let a2 = root.join("a2");
    std::fs::create_dir(&a2).unwrap();
    super::git(&a2, &["init", "-q"]);
    let out = remote_call_env(&d, &ssh, &a2, &["echo", "beside"], &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "beside\n", "{err}");
    assert!(err.contains("slot: synced to node"), "{err}");
}

/// A symlink to a file outside the tree reaches the mirror as the link,
/// never as the file's content, and the sync line counts it.
#[test]
fn an_outside_symlink_is_not_followed() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, _) = repository(&root);
    std::fs::write(root.join("secret"), "outside-content\n").unwrap();
    std::os::unix::fs::symlink(root.join("secret"), a.join("abs")).unwrap();
    std::os::unix::fs::symlink("../secret", a.join("up")).unwrap();
    std::os::unix::fs::symlink("f00.bin", a.join("inside")).unwrap();
    let ssh = forced_ssh(&root, &d, "");
    let out = remote_call_env(
        &d,
        &ssh,
        &a,
        &[
            "sh",
            "-c",
            "for l in abs up inside; do test -L $l && echo $l; done",
        ],
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "abs\nup\ninside\n",
        "{err}"
    );
    assert!(
        err.contains(", 2 symlinks leave the tree (sent as links), "),
        "{err}"
    );
    let mirror = the_mirror(&d);
    for l in ["abs", "up"] {
        let m = std::fs::symlink_metadata(mirror.join(l)).unwrap();
        assert!(m.file_type().is_symlink(), "{l} arrived as a file");
    }
    for p in tree_of(&mirror) {
        let m = std::fs::symlink_metadata(&p).unwrap();
        if m.is_file() {
            let bytes = std::fs::read(&p).unwrap();
            assert!(
                !bytes.windows(15).any(|w| w == b"outside-content"),
                "{p} holds the target's content"
            );
        }
    }
}

/// The rsync on `PATH` is a wrapper that adds `--copy-unsafe-links`: the
/// client refuses before any byte is sent and runs the job here.
#[test]
fn an_rsync_that_follows_outside_links_is_refused() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, _) = repository(&root);
    std::fs::write(root.join("secret"), "outside-content\n").unwrap();
    std::os::unix::fs::symlink("../secret", a.join("up")).unwrap();
    let real = std::env::var_os("PATH")
        .and_then(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("rsync"))
                .find(|r| r.is_file())
        })
        .expect("rsync on PATH");
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    script(
        &bin.join("rsync"),
        &format!(
            "case \" $* \" in *\" --server \"*) exec {r} \"$@\" ;; esac\nexec {r} --copy-unsafe-links \"$@\"",
            r = real.display()
        ),
    );
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let ssh = forced_ssh(&root, &d, "");
    let out = remote_call_env(
        &d,
        &ssh,
        &a,
        &["echo", "ran-here"],
        &[
            ("PATH", path.to_str().unwrap()),
            ("SLOT_REMOTE_FALLBACK", "local"),
        ],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(
        err.contains("would send what 1 symlink outside the tree points to (up)"),
        "{err}"
    );
    assert!(err.contains("running it on this machine's daemon"), "{err}");
    assert_eq!(ssh_calls(&root), "", "ssh ran");
    logged_line(&d, &["echo", "ran-here"]);
}

/// A variable named in `SLOT_ENV` crosses unless its name looks like a
/// credential; held-back names are named on stderr, their values never.
#[test]
fn a_secret_looking_variable_does_not_cross() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, _) = repository(&root);
    let ssh = forced_ssh(&root, &d, "");
    let out = remote_call_env(
        &d,
        &ssh,
        &a,
        &[
            "sh",
            "-c",
            "echo \"[$FEATURE_X][$MY_DB_PASS][$NPM_PAT][$SENTRY_DSN][$CARGO_REGISTRY_TOKEN]\"",
        ],
        &[
            ("SLOT_ENV", "FEATURE_X,MY_DB_PASS,NPM_PAT,SENTRY_DSN"),
            ("FEATURE_X", "on"),
            ("MY_DB_PASS", "value-one"),
            ("NPM_PAT", "value-two"),
            ("SENTRY_DSN", "value-three"),
            ("CARGO_REGISTRY_TOKEN", "value-four"),
        ],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "[on][][][][]\n",
        "{err}"
    );
    let held: Vec<&str> = err
        .lines()
        .find_map(|l| l.strip_prefix("slot: not sent to node, the names look like a credential: "))
        .unwrap_or_else(|| panic!("no line names the held names: {err}"))
        .split(", ")
        .collect();
    for name in [
        "CARGO_REGISTRY_TOKEN",
        "MY_DB_PASS",
        "NPM_PAT",
        "SENTRY_DSN",
    ] {
        assert!(held.contains(&name), "{name} not named: {err}");
    }
    assert!(!held.contains(&"FEATURE_X"), "{err}");
    assert!(!err.contains("value-"), "a value reached stderr: {err}");
    let calls = ssh_calls(&root);
    assert!(
        calls.contains("proxy"),
        "the job went to the server: {calls}"
    );
}

/// Same-path: a directory outside any checkout is a tree only when it is
/// a plain folder. The same-path root itself, a folder directly under it,
/// and a folder that holds a repository within two levels are refused:
/// nothing is synced and the job does not run (exit 3). One repository in
/// such a folder is still synced on its own.
#[test]
fn a_tree_root_that_is_not_one_repository_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let home = super::temp_root();
    assert_eq!(root.parent(), Some(home.as_path()));
    let group = root.join("group");
    std::fs::create_dir(&group).unwrap();
    let (a, _) = repository(&group);
    let deep = root.join("deep");
    std::fs::create_dir_all(deep.join("x/r")).unwrap();
    super::git(&deep.join("x/r"), &["init", "-q"]);
    std::fs::write(deep.join("note.txt"), "here\n").unwrap();
    let d = Daemon::start_with(&["--same-path-root".as_ref(), home.as_os_str()]);
    let ssh = super::forced_ssh_with(
        &root,
        &d,
        "",
        &format!("--same-path-root {}", home.display()),
    );
    let cases: [(&Path, &str); 4] = [
        (&home, "it is the same-path root or above it"),
        (&root, "it lies directly under the same-path root"),
        (&group, "it holds the repositories a, b"),
        (&deep, "it holds the repositories x/r"),
    ];
    for (cwd, why) in cases {
        let out = remote_call_env(&d, &ssh, cwd, &["echo", "ran"], super::SAME_PATH);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "{}: {err}", cwd.display());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{err}");
        let want = format!(
            "{} is not one repository's work tree ({why})",
            cwd.display()
        );
        assert!(err.contains(&want), "want {want:?} in {err}");
    }
    let calls = ssh_calls(&root);
    assert!(!calls.contains("rsync"), "a sync ran: {calls}");

    let out = remote_call_env(&d, &ssh, &a, &["echo", "ran"], super::SAME_PATH);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ran\n", "{err}");
    assert!(err.contains("slot: synced to node"), "{err}");
}
