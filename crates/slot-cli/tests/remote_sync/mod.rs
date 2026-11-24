//! Remote mode's sync against a stand-in build server: the server is this
//! machine, reached through a fake `ssh` that runs `slot shell` as the
//! forced command would, with a daemon of the test's own. No network.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use super::{Daemon, SLOT};

/// Files of the test repository and their size: 2 MiB that do not compress.
const FILES: usize = 32;
const FILE_BYTES: usize = 64 << 10;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
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
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository with an `origin` and [`FILES`] files of noise, and a second
/// worktree of it at the same commit.
fn repository(root: &Path) -> (PathBuf, PathBuf) {
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir(&a).unwrap();
    git(&a, &["init", "-q"]);
    git(
        &a,
        &["remote", "add", "origin", "git@example.invalid:o/r.git"],
    );
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for i in 0..FILES {
        let bytes: Vec<u8> = (0..FILE_BYTES)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x.to_le_bytes()[0]
            })
            .collect();
        std::fs::write(a.join(format!("f{i:02}.bin")), bytes).unwrap();
    }
    git(&a, &["add", "."]);
    git(&a, &["commit", "-qm", "noise"]);
    git(
        &a,
        &["worktree", "add", "-q", "--detach", b.to_str().unwrap()],
    );
    (a, b)
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A fake `ssh HOST WORDS…` that runs WORDS through `slot shell` against
/// daemon `d`, as the forced command of the server's key does.
fn forced_ssh(dir: &Path, d: &Daemon, before: &str) -> PathBuf {
    forced_ssh_with(dir, d, before, "")
}

/// [`forced_ssh`] with `shell_args` added to `slot shell`.
fn forced_ssh_with(dir: &Path, d: &Daemon, before: &str, shell_args: &str) -> PathBuf {
    let path = dir.join("ssh");
    script(
        &path,
        &format!(
            "shift\necho \"$*\" >> {log}\n{before}\nSSH_ORIGINAL_COMMAND=\"$*\" exec {SLOT} shell --socket {sock} --state-dir {state} {shell_args}",
            log = dir.join("ssh.log").display(),
            sock = d.socket().display(),
            state = d.dir.path().join("state").display(),
        ),
    );
    path
}

fn remote_call(d: &Daemon, ssh: &Path, cwd: &Path, argv: &[&str]) -> Output {
    remote_call_env(d, ssh, cwd, argv, &[])
}

fn remote_call_env(
    d: &Daemon,
    ssh: &Path,
    cwd: &Path,
    argv: &[&str],
    env: &[(&str, &str)],
) -> Output {
    Command::new(SLOT)
        .args(argv)
        .env_remove("SLOT_REMOTE_SAME_PATH")
        .envs(env.iter().copied())
        .current_dir(cwd)
        .env("SLOT_HOST", "node")
        .env("SLOT_SSH", ssh)
        .env("SLOT_WSROOT", d.dir.path().join("state/ws"))
        .env("SLOT_REMOTE", SLOT)
        .env("SLOT_SOCKET", d.socket())
        .env("SLOT_SYNC_STALL", "2s")
        .env("SLOT_SYNC_LIMIT", "30s")
        .env_remove("SLOT_JOB")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// The bytes of the client's `slot: synced to …` line.
fn bytes_sent(err: &str) -> Option<u64> {
    let line = err
        .lines()
        .find(|l| l.starts_with("slot: synced to node in "))?;
    let (head, _) = line.split_once(" bytes sent")?;
    head.rsplit(' ').next()?.parse().ok()
}

#[test]
fn a_new_worktree_sends_only_what_differs_from_its_sibling_mirror() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let (a, b) = repository(t.path());
    let ssh = forced_ssh(t.path(), &d, "");
    let tree = (FILES * FILE_BYTES) as u64;

    let first = remote_call(&d, &ssh, &a, &["wc", "-c", "f00.bin"]);
    let err = String::from_utf8_lossy(&first.stderr);
    assert_eq!(first.status.code(), Some(0), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&first.stdout),
        format!("{FILE_BYTES} f00.bin\n"),
        "ran in the mirror: {err}"
    );
    let full = bytes_sent(&err).unwrap_or_else(|| panic!("no sync line: {err}"));
    assert!(full >= tree, "first sync sent {full} of {tree}: {err}");

    std::fs::write(b.join("note.txt"), "from b\n").unwrap();
    let second = remote_call(&d, &ssh, &b, &["cat", "note.txt"]);
    let err = String::from_utf8_lossy(&second.stderr);
    assert_eq!(second.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&second.stdout), "from b\n", "{err}");
    let seeded = bytes_sent(&err).unwrap_or_else(|| panic!("no sync line: {err}"));
    assert!(
        seeded * 20 < tree,
        "the sibling's worktree sent {seeded} of {tree} bytes: {err}"
    );
    assert!(err.contains("1 sibling mirror as base"), "{err}");
    eprintln!("first sync {full} bytes, the sibling's first sync {seeded} bytes, tree {tree}");
}

#[test]
fn a_server_without_the_sibling_step_gets_a_plain_sync() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let (a, _) = repository(t.path());
    let ssh = forced_ssh(
        t.path(),
        &d,
        "case \"$2\" in __siblings) echo \"slot shell: refused\" >&2; exit 126;; esac",
    );
    let out = remote_call(&d, &ssh, &a, &["wc", "-c", "f01.bin"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{FILE_BYTES} f01.bin\n"),
        "ran remote: {err}"
    );
    assert!(err.contains("bytes sent not counted"), "{err}");
    let log = std::fs::read_to_string(t.path().join("ssh.log")).unwrap();
    let rsync = log.lines().find(|l| l.starts_with("rsync ")).unwrap();
    assert!(
        !rsync.contains("--stats") && !rsync.contains("--copy-dest"),
        "an older server refuses these: {rsync}"
    );
}

/// Starts a sync that never moves a byte, or whose rsync is killed by
/// someone else, and checks that the job ran on this machine's daemon, not
/// on the server, and that no connection to the server's daemon was made.
fn falls_back(rsync_step: &str, why: &str) {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let (a, _) = repository(t.path());
    let ssh = t.path().join("ssh");
    script(
        &ssh,
        &format!(
            "shift\necho \"$*\" >> {}\ncase \"$1\" in rsync) {rsync_step};; esac\nexit 126",
            t.path().join("ssh.log").display()
        ),
    );
    let t0 = Instant::now();
    let out = remote_call(&d, &ssh, &a, &["echo", "ran-here"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ran-here\n", "{err}");
    assert!(err.contains("running it on this machine's daemon"), "{err}");
    assert!(err.contains(why), "{err}");
    assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
    let log = std::fs::read_to_string(t.path().join("ssh.log")).unwrap();
    assert!(!log.contains("proxy"), "went to the server: {log}");
    assert!(
        super::wait_for(|| d.logged(&["echo", "ran-here"]) == 1),
        "not on the local daemon"
    );
}

#[test]
fn a_stalled_sync_is_killed_and_the_job_runs_here() {
    falls_back("exec sleep 20", "no byte moved for 2 s");
}

#[test]
fn a_sync_whose_rsync_is_killed_runs_the_job_here() {
    falls_back("kill -9 $PPID; exec sleep 1", "rsync exited");
}

const SAME_PATH: &[(&str, &str)] = &[("SLOT_REMOTE_SAME_PATH", "1")];

/// With `SLOT_REMOTE_SAME_PATH=1` the tree is synced to its own absolute
/// path and the job runs there; a second worktree is seeded from the first
/// through its link in the workspace root.
#[test]
fn a_same_path_job_runs_at_the_clients_absolute_path() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, b) = repository(&root);
    let d = Daemon::start_with(&["--same-path-root".as_ref(), root.as_os_str()]);
    let ssh = forced_ssh_with(
        &root,
        &d,
        "",
        &format!("--same-path-root {}", root.display()),
    );
    std::fs::create_dir(a.join("sub")).unwrap();
    let out = remote_call_env(&d, &ssh, &a.join("sub"), &["pwd"], SAME_PATH);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        a.join("sub").to_string_lossy(),
        "{err}"
    );
    let log = std::fs::read_to_string(root.join("ssh.log")).unwrap();
    let rsync = log.lines().find(|l| l.starts_with("rsync ")).unwrap();
    assert!(
        rsync.ends_with(&format!(" {}/", a.display())),
        "synced elsewhere: {rsync}"
    );
    let ws = d.dir.path().join("state/ws");
    let links: Vec<PathBuf> = std::fs::read_dir(&ws)
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .collect();
    assert_eq!(links, std::slice::from_ref(&a), "the mirror's link by id");

    std::fs::write(b.join("note.txt"), "from b\n").unwrap();
    let out = remote_call_env(&d, &ssh, &b, &["sh", "-c", "pwd; cat note.txt"], SAME_PATH);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{}\nfrom b\n", b.display()),
        "{err}"
    );
    assert!(err.contains("1 sibling mirror as base"), "{err}");
}

/// A same-path tree outside the server's `--same-path-root`: `slot shell`
/// refuses the sync, and the daemon refuses the job.
#[test]
fn a_same_path_tree_outside_the_roots_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, _) = repository(&root);
    let other = root.join("elsewhere");
    let d = Daemon::start_with(&["--same-path-root".as_ref(), other.as_os_str()]);
    let ssh = forced_ssh_with(
        &root,
        &d,
        "",
        &format!("--same-path-root {}", other.display()),
    );
    let out = remote_call_env(&d, &ssh, &a, &["echo", "ran"], SAME_PATH);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("running it on this machine's daemon"), "{err}");

    // The shell admits it, the daemon does not.
    let ssh = forced_ssh_with(
        &root,
        &d,
        "",
        &format!("--same-path-root {}", root.display()),
    );
    let out = remote_call_env(&d, &ssh, &a, &["echo", "ran"], SAME_PATH);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("not below a --same-path-root"), "{err}");
}

/// A cargo package `tiny` in a git repository: one binary, one integration
/// test, a `Cargo.lock`, and `extra` added to `Cargo.toml`.
fn tiny_crate(root: &Path, extra: &str) -> PathBuf {
    let r = root.join("tiny");
    std::fs::create_dir_all(r.join("src")).unwrap();
    std::fs::create_dir_all(r.join("tests")).unwrap();
    std::fs::write(
        r.join("Cargo.toml"),
        format!(
            "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n{extra}"
        ),
    )
    .unwrap();
    std::fs::write(
        r.join("Cargo.lock"),
        "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 3\n\n[[package]]\nname = \"tiny\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        r.join("src/main.rs"),
        "fn main() {\n    println!(\"hi from tiny\");\n}\n",
    )
    .unwrap();
    std::fs::write(
        r.join("tests/it.rs"),
        "#[test]\nfn adds() {\n    assert_eq!(1 + 1, 2);\n}\n",
    )
    .unwrap();
    std::fs::write(r.join(".gitignore"), "/target/\n").unwrap();
    git(&r, &["init", "-q"]);
    git(&r, &["add", "."]);
    git(&r, &["commit", "-qm", "tiny"]);
    r
}

/// A `cargo` that appends `<CARGO_NET_OFFLINE>|<args>` to `cargo.log` in
/// `dir` and runs the cargo that built this test.
fn logging_cargo(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join("cargo");
    script(
        &path,
        &format!(
            "echo \"$CARGO_NET_OFFLINE|$*\" >> {log}\nexec {real} \"$@\"",
            log = dir.join("cargo.log").display(),
            real = env!("CARGO"),
        ),
    );
    path
}

/// A remote call of a cargo job. The caller names no target directory, so
/// the local one is `<repository>/target`.
fn remote_cargo(d: &Daemon, ssh: &Path, cwd: &Path, argv: &[&str]) -> Output {
    Command::new(SLOT)
        .args(argv)
        .current_dir(cwd)
        .env("SLOT_HOST", "node")
        .env("SLOT_SSH", ssh)
        .env("SLOT_WSROOT", d.dir.path().join("state/ws"))
        .env("SLOT_REMOTE", SLOT)
        .env("SLOT_SOCKET", d.socket())
        .env("SLOT_SYNC_STALL", "20s")
        .env("SLOT_SYNC_LIMIT", "50s")
        .env_remove("SLOT_JOB")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_NET_OFFLINE")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// The one target directory the stand-in node built into.
fn node_target(d: &Daemon) -> PathBuf {
    let dirs: Vec<PathBuf> = std::fs::read_dir(d.dir.path().join("state/cache/target"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.into_iter().next().unwrap()
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode()
}

#[test]
fn a_remote_cargo_build_leaves_the_binary_in_the_local_target_dir() {
    let t0 = Instant::now();
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = tiny_crate(t.path(), "");
    let ssh = forced_ssh(t.path(), &d, "");
    let cargo = logging_cargo(t.path());
    let out = remote_cargo(&d, &ssh, &repo, &[cargo.to_str().unwrap(), "build"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "",
        "cargo's JSON reached the caller: {err}"
    );
    assert!(err.contains("Compiling tiny"), "cargo's own lines: {err}");
    assert!(
        err.lines()
            .any(|l| l.starts_with("slot: fetched 1 artifact, ") && l.ends_with(" s")),
        "{err}"
    );
    let local = repo.join("target/debug/tiny");
    let node = node_target(&d).join("debug/tiny");
    assert_eq!(
        std::fs::read(&local).unwrap(),
        std::fs::read(&node).unwrap()
    );
    assert_eq!(
        std::fs::metadata(&local).unwrap().modified().unwrap(),
        std::fs::metadata(&node).unwrap().modified().unwrap(),
        "mtime kept"
    );
    assert_ne!(mode(&local) & 0o111, 0, "executable");
    let ran = Command::new(&local).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "hi from tiny\n");
    assert!(
        !repo.join("target/debug/deps").exists(),
        "only the product came back"
    );
    let log = std::fs::read_to_string(t.path().join("cargo.log")).unwrap();
    assert!(
        log.lines()
            .any(|l| l == "true|build --message-format=json-render-diagnostics"),
        "{log}"
    );
    eprintln!("took {:?}", t0.elapsed());
}

#[test]
fn a_remote_cargo_test_no_run_leaves_the_test_binary() {
    let t0 = Instant::now();
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = tiny_crate(t.path(), "");
    let ssh = forced_ssh(t.path(), &d, "");
    let cargo = logging_cargo(t.path());
    let out = remote_cargo(
        &d,
        &ssh,
        &repo,
        &[cargo.to_str().unwrap(), "test", "--no-run"],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{err}");
    let deps = repo.join("target/debug/deps");
    let it: Vec<PathBuf> = std::fs::read_dir(&deps)
        .unwrap_or_else(|e| panic!("{}: {e}: {err}", deps.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("it-") && !n.contains('.'))
        })
        .collect();
    assert_eq!(it.len(), 1, "{it:?}: {err}");
    let listed = Command::new(&it[0]).arg("--list").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout).lines().next(),
        Some("adds: test"),
        "the test binary runs here"
    );
    assert!(repo.join("target/debug/tiny").exists(), "{err}");
    assert!(
        err.lines()
            .any(|l| l.starts_with("slot: fetched 3 artifacts, ")),
        "the binary, its unit tests and the integration test: {err}"
    );
    eprintln!("took {:?}", t0.elapsed());
}

#[test]
fn a_server_that_takes_one_path_per_fetch_gets_the_products_one_by_one() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = tiny_crate(t.path(), "");
    // The `slot shell` of 0.33.0 and older refuses a fetch of several paths.
    let ssh = forced_ssh(
        t.path(),
        &d,
        "case \"$*\" in *\"/./\"*\"/./\"*) echo \"slot shell: refused\" >&2; exit 126;; esac",
    );
    let cargo = logging_cargo(t.path());
    let out = remote_cargo(
        &d,
        &ssh,
        &repo,
        &[cargo.to_str().unwrap(), "test", "--no-run"],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(
        err.lines()
            .any(|l| l.starts_with("slot: fetched 3 artifacts, ")),
        "{err}"
    );
    assert!(repo.join("target/debug/tiny").exists(), "{err}");
    let log = std::fs::read_to_string(t.path().join("ssh.log")).unwrap();
    let single = log
        .lines()
        .filter(|l| l.contains("--sender") && l.matches("/./").count() == 1)
        .count();
    assert_eq!(single, 3, "{log}");
}

#[test]
fn a_second_job_with_the_same_lock_file_does_not_fetch_again() {
    let t0 = Instant::now();
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = tiny_crate(t.path(), "");
    let ssh = forced_ssh(t.path(), &d, "");
    let cargo = logging_cargo(t.path());
    let cargo = cargo.to_str().unwrap();
    for argv in [[cargo, "check"], [cargo, "build"]] {
        let out = remote_cargo(&d, &ssh, &repo, &argv);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{err}");
    }
    let log = std::fs::read_to_string(t.path().join("cargo.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(
        lines,
        [
            "|fetch --locked",
            "true|check",
            "true|build --message-format=json-render-diagnostics"
        ],
        "one fetch, then each job offline"
    );
    eprintln!("took {:?}", t0.elapsed());
}

#[test]
fn a_job_whose_fetch_fails_runs_on_this_machines_daemon() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = tiny_crate(
        t.path(),
        "\n[dependencies]\nabsent = { path = \"absent\" }\n",
    );
    let ssh = forced_ssh(t.path(), &d, "");
    let cargo = logging_cargo(t.path());
    let out = remote_cargo(&d, &ssh, &repo, &[cargo.to_str().unwrap(), "check"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("cargo fetch --locked failed")
            && err.contains("running it on this machine's daemon"),
        "{err}"
    );
    let log = std::fs::read_to_string(t.path().join("cargo.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines, ["|fetch --locked", "|check"], "{err}");
}
