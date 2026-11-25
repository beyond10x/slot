//! Remote mode's sync against a stand-in build server: the server is this
//! machine, reached through a fake `ssh` that runs `slot shell` as the
//! forced command would, with a daemon of the test's own. No network.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use super::{Daemon, SLOT};

mod fresh;
mod node_view;
mod phases;
mod safety;

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

/// A remote job that cannot start runs on this machine's daemon only with this.
const FALLBACK: &[(&str, &str)] = &[("SLOT_REMOTE_FALLBACK", "local")];

/// The directory every test's tempdir lies in: the client keeps a tree's
/// own path on the server only below its same-path root.
fn temp_root() -> PathBuf {
    std::env::temp_dir().canonicalize().unwrap()
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
        .env_remove("CARGO_TARGET_DIR")
        .env("SLOT_REMOTE_SAME_PATH_ROOT", temp_root())
        .env("XDG_CONFIG_HOME", d.dir.path().join("config"))
        .env("SLOT_CLUSTER_FILE", "off")
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

/// Writes `content` to `path` and gives it modification time `mtime`.
fn write_at(path: &Path, content: &str, mtime: std::time::SystemTime) {
    std::fs::write(path, content).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

fn short_head(dir: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_file_changed_at_the_same_size_and_time_reaches_the_mirror() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let (a, _) = repository(t.path());
    let ssh = forced_ssh(t.path(), &d, "");
    let src = a.join("lib.rs");
    std::fs::write(&src, "pub fn one() {}\n").unwrap();
    let mtime = std::fs::metadata(&src).unwrap().modified().unwrap();

    let first = remote_call(&d, &ssh, &a, &["cat", "lib.rs"]);
    let err = String::from_utf8_lossy(&first.stderr);
    assert_eq!(
        String::from_utf8_lossy(&first.stdout),
        "pub fn one() {}\n",
        "{err}"
    );

    write_at(&src, "pub fn two() {}\n", mtime);
    let second = remote_call(&d, &ssh, &a, &["cat", "lib.rs"]);
    let err = String::from_utf8_lossy(&second.stderr);
    assert_eq!(second.status.code(), Some(0), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        "pub fn two() {}\n",
        "the mirror kept the old content: {err}"
    );
    assert!(
        err.contains(&format!(", at {} + 1 changed file", short_head(&a))),
        "the line names the sources: {err}"
    );
    let log = std::fs::read_to_string(t.path().join("ssh.log")).unwrap();
    let rsync = log.lines().find(|l| l.starts_with("rsync ")).unwrap();
    let flags = rsync.split(' ').nth(2).unwrap();
    assert!(
        flags.split('.').next().unwrap().contains('c'),
        "the server's rsync compares by content: {rsync}"
    );
}

#[test]
fn a_sibling_file_of_the_same_size_and_time_is_not_taken_for_the_worktrees() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let (a, b) = repository(t.path());
    let ssh = forced_ssh(t.path(), &d, "");
    std::fs::write(a.join("lib.rs"), "pub fn one() {}\n").unwrap();
    let mtime = std::fs::metadata(a.join("lib.rs"))
        .unwrap()
        .modified()
        .unwrap();
    let first = remote_call(&d, &ssh, &a, &["cat", "lib.rs"]);
    assert_eq!(first.status.code(), Some(0));

    write_at(&b.join("lib.rs"), "pub fn two() {}\n", mtime);
    let second = remote_call(&d, &ssh, &b, &["cat", "lib.rs"]);
    let err = String::from_utf8_lossy(&second.stderr);
    assert_eq!(second.status.code(), Some(0), "{err}");
    assert!(err.contains("1 sibling mirror as base"), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        "pub fn two() {}\n",
        "the sibling's file was taken: {err}"
    );
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
/// someone else, and checks that the job does not run without
/// `SLOT_REMOTE_FALLBACK=local`, and with it ran on this machine's daemon, not
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
    // Without `SLOT_REMOTE_FALLBACK=local` the job does not run at all.
    let out = remote_call(&d, &ssh, &a, &["echo", "ran-here"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{err}");
    assert!(err.contains("the job did not run"), "{err}");
    assert!(err.contains(why), "{err}");
    let t0 = Instant::now();
    let out = remote_call_env(&d, &ssh, &a, &["echo", "ran-here"], FALLBACK);
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
    let out = remote_call_env(&d, &ssh, &a, &["echo", "ran"], &[SAME_PATH[0], FALLBACK[0]]);
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
    remote_cargo_env(d, ssh, cwd, argv, &[])
}

/// [`remote_cargo`] with `env` added to the caller's environment.
fn remote_cargo_env(
    d: &Daemon,
    ssh: &Path,
    cwd: &Path,
    argv: &[&str],
    env: &[(&str, &str)],
) -> Output {
    Command::new(SLOT)
        .env_remove("SLOT_NO_PRODUCTS")
        .envs(env.iter().copied())
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
        // A forge runner sets CARGO_TERM_COLOR=always; the job gets it and
        // cargo would colour the lines this test reads.
        .env("CARGO_TERM_COLOR", "never")
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

/// Executables in `deps` whose name starts with `<stem>-`, without the
/// sibling files that have an extension.
fn hashed_binaries(deps: &Path, stem: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(deps)
        .unwrap_or_else(|e| panic!("{}: {e}", deps.display()))
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&format!("{stem}-")) && !n.contains('.'))
        .collect();
    out.sort();
    out
}

#[test]
fn two_remote_test_runs_of_a_rebuilt_target_leave_one_test_binary_here() {
    let t0 = Instant::now();
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = tiny_crate(t.path(), "");
    let ssh = forced_ssh(t.path(), &d, "");
    let cargo = logging_cargo(t.path());
    let cargo = cargo.to_str().unwrap();
    let out = remote_cargo(&d, &ssh, &repo, &[cargo, "test"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let deps = repo.join("target/debug/deps");
    let first = hashed_binaries(&deps, "it");
    assert_eq!(first.len(), 1, "{first:?}: {err}");
    // A new package version gives every target of it a new hash.
    for f in ["Cargo.toml", "Cargo.lock"] {
        let p = repo.join(f);
        let s = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, s.replace("0.1.0", "0.1.1")).unwrap();
    }
    let out = remote_cargo(&d, &ssh, &repo, &[cargo, "test"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let node = hashed_binaries(&node_target(&d).join("debug/deps"), "it");
    assert_eq!(node.len(), 2, "the hash changed on the node: {node:?}");
    let it = hashed_binaries(&deps, "it");
    assert_eq!(it.len(), 1, "{it:?}: {err}");
    assert_ne!(it, first, "the new binary is the one kept");
    assert_eq!(hashed_binaries(&deps, "tiny").len(), 1, "{err}");
    let line = err
        .lines()
        .find(|l| l.starts_with("slot: fetched 3 artifacts, "))
        .unwrap_or_else(|| panic!("{err}"));
    assert!(
        line.contains("; removed 2 superseded files, "),
        "the unit tests and the integration test of 0.1.0: {line}"
    );
    eprintln!("took {:?}", t0.elapsed());
}

#[test]
fn slot_no_products_copies_nothing_back() {
    let t0 = Instant::now();
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = tiny_crate(t.path(), "");
    let ssh = forced_ssh(t.path(), &d, "");
    let cargo = logging_cargo(t.path());
    let out = remote_cargo_env(
        &d,
        &ssh,
        &repo,
        &[cargo.to_str().unwrap(), "build"],
        &[("SLOT_NO_PRODUCTS", "1")],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(!repo.join("target").exists(), "{err}");
    assert!(node_target(&d).join("debug/tiny").is_file(), "built there");
    assert_eq!(
        err.lines()
            .filter(|l| l.starts_with("slot: artifacts not fetched: SLOT_NO_PRODUCTS"))
            .count(),
        1,
        "{err}"
    );
    assert!(!err.contains("slot: fetched"), "{err}");
    let log = std::fs::read_to_string(t.path().join("cargo.log")).unwrap();
    assert!(
        log.lines().any(|l| l == "true|build"),
        "the caller's command unchanged: {log}"
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
            "|fetch",
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
    let out = remote_cargo_env(
        &d,
        &ssh,
        &repo,
        &[cargo.to_str().unwrap(), "check"],
        FALLBACK,
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("cargo fetch failed") && err.contains("running it on this machine's daemon"),
        "{err}"
    );
    let log = std::fs::read_to_string(t.path().join("cargo.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines, ["|fetch", "|check"], "{err}");
}

/// A daemon and a fake ssh that admit same-path mirrors below `root`.
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

/// The rsync receivers `ssh.log` in `root` shows, by destination path.
fn synced_paths(root: &Path) -> Vec<String> {
    let log = std::fs::read_to_string(root.join("ssh.log")).unwrap();
    log.lines()
        .filter(|l| l.starts_with("rsync --server") && !l.contains("--sender"))
        .filter_map(|l| l.rsplit(' ').next())
        .map(String::from)
        .collect()
}

/// Same-path: a linked worktree's job gets its repository's common git
/// directory at its own path, and a working directory under an ignored
/// path (`target/`) is synced on its own.
#[test]
fn a_same_path_job_carries_its_git_dir_and_ignored_cwd() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (a, b) = repository(&root);
    let (d, ssh) = same_path_node(&root);
    let cwd = b.join("target/scratch/cli");
    std::fs::create_dir_all(&cwd).unwrap();
    let out = remote_call_env(
        &d,
        &ssh,
        &cwd,
        &["sh", "-c", "pwd; git rev-parse --show-toplevel"],
        SAME_PATH,
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{}\n{}\n", cwd.display(), b.display()),
        "{err}"
    );
    let got = synced_paths(&root);
    for want in [
        format!("{}/", b.display()),
        format!("{}/", a.join(".git").display()),
        format!("{}/", cwd.display()),
    ] {
        assert!(got.contains(&want), "{want} not synced: {got:?}\n{err}");
    }
}

/// Same-path: a directory outside any git checkout is the tree.
#[test]
fn a_same_path_job_outside_a_checkout_syncs_its_directory() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (d, ssh) = same_path_node(&root);
    let dir = root.join("copy");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.txt"), "here\n").unwrap();
    let out = remote_call_env(
        &d,
        &ssh,
        &dir,
        &["sh", "-c", "pwd; cat note.txt"],
        SAME_PATH,
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(!err.contains("this machine's daemon"), "{err}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{}\nhere\n", dir.display()),
        "{err}"
    );
    assert!(
        synced_paths(&root).contains(&format!("{}/", dir.display())),
        "{err}"
    );
}

/// Same-path: cargo builds into the client's own target directory, also
/// for a nested workspace named with `--manifest-path`; the node's cache
/// holds no target directory.
#[test]
fn a_same_path_cargo_build_uses_the_clients_target_dir() {
    let t0 = Instant::now();
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (d, ssh) = same_path_node(&root);
    let repo = tiny_crate(&root, "");
    let driver = repo.join("driver");
    std::fs::create_dir_all(driver.join("src")).unwrap();
    std::fs::write(
        driver.join("Cargo.toml"),
        "[package]\nname = \"driver\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(driver.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        driver.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"driver\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cargo = bin.join("cargo");
    script(
        &cargo,
        &format!(
            "echo \"$CARGO_TARGET_DIR|$*\" >> {log}\nexec {real} \"$@\"",
            log = root.join("cargo.log").display(),
            real = env!("CARGO"),
        ),
    );
    let cargo = cargo.to_str().unwrap();
    for argv in [
        vec![cargo, "build"],
        vec![cargo, "build", "--manifest-path", "driver/Cargo.toml"],
    ] {
        let out = Command::new(SLOT)
            .args(&argv)
            .current_dir(&repo)
            .env("SLOT_HOST", "node")
            .env("SLOT_SSH", &ssh)
            .env("SLOT_WSROOT", d.dir.path().join("state/ws"))
            .env("SLOT_REMOTE", SLOT)
            .env("SLOT_SOCKET", d.socket())
            .env("SLOT_REMOTE_SAME_PATH", "1")
            .env("SLOT_REMOTE_SAME_PATH_ROOT", temp_root())
            .env("SLOT_SYNC_STALL", "20s")
            .env("SLOT_SYNC_LIMIT", "50s")
            .env_remove("SLOT_JOB")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{argv:?}: {err}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{err}");
    }
    assert!(repo.join("target/debug/tiny").is_file());
    assert!(driver.join("target/debug/driver").is_file());
    let log = std::fs::read_to_string(root.join("cargo.log")).unwrap();
    let dirs: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("|build"))
        .filter_map(|l| l.split('|').next())
        .collect();
    assert_eq!(
        dirs,
        [
            repo.join("target").to_str().unwrap(),
            driver.join("target").to_str().unwrap()
        ],
        "{log}"
    );
    let node = d.dir.path().join("state/cache/target");
    let built: Vec<_> = std::fs::read_dir(&node)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    assert!(built.is_empty(), "built in the node's cache: {built:?}");
    eprintln!("took {:?}", t0.elapsed());
}
