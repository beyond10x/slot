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
    let path = dir.join("ssh");
    script(
        &path,
        &format!(
            "shift\necho \"$*\" >> {log}\n{before}\nSSH_ORIGINAL_COMMAND=\"$*\" exec {SLOT} shell --socket {sock} --state-dir {state}",
            log = dir.join("ssh.log").display(),
            sock = d.socket().display(),
            state = d.dir.path().join("state").display(),
        ),
    );
    path
}

fn remote_call(d: &Daemon, ssh: &Path, cwd: &Path, argv: &[&str]) -> Output {
    Command::new(SLOT)
        .args(argv)
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
