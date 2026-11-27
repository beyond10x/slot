//! A job sent to a build server whose daemon does not answer: the proxy
//! step finds no socket while that daemon restarts or lets its jobs end.
//! The job waits and is sent again; it does not fail at once and does not
//! run here. The server is the stand-in of `remote_sync`, whose `ssh` runs
//! the proxy step against a socket that does not exist.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{Daemon, SLOT, forced_ssh, git, remote_call_env, remote_command};

const WAITING: &str = "slot: waiting for node: its daemon does not answer (a restart or a drain); the job is sent again when it answers";

/// A checkout of one committed file.
fn checkout(root: &Path) -> PathBuf {
    let a = root.join("a");
    std::fs::create_dir(&a).unwrap();
    git(&a, &["init", "-q"]);
    std::fs::write(a.join("note.txt"), "one\n").unwrap();
    git(&a, &["add", "."]);
    git(&a, &["commit", "-qm", "one"]);
    a
}

/// The `before` part of a stand-in `ssh` that does `step` in place of the
/// proxy step; every other step goes to the daemon.
fn proxy_step(step: &str) -> String {
    format!("case \"$*\" in *\" proxy\") {step};; esac")
}

/// More connections than a test makes: the daemon stays away.
const ALWAYS: u32 = 1_000_000;

/// A proxy step that finds no daemon socket, as `slot shell` runs it on a
/// server whose daemon is away, for the first `times` connections.
fn no_socket(dir: &Path, d: &Daemon, times: u32) -> String {
    proxy_step(&format!(
        "n=$(cat {count} 2>/dev/null || echo 0); n=$((n + 1)); echo \"$n\" > {count}; \
         if [ \"$n\" -le {times} ]; then SSH_ORIGINAL_COMMAND=\"$*\" exec {SLOT} shell --socket {gone} --state-dir {state}; fi",
        count = dir.join("proxies").display(),
        gone = dir.join("gone.sock").display(),
        state = d.dir.path().join("state").display(),
    ))
}

/// How often the stand-in `ssh` was asked for the proxy step.
fn proxies(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("ssh.log"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.ends_with(" proxy"))
        .count()
}

/// A command that leaves a line in `marker` wherever it runs, and exits 7.
fn marking(marker: &Path) -> String {
    format!("echo ran >> {}; exit 7", marker.display())
}

/// The environment of a call that waits `wait` for the server's daemon and
/// has neither the opt-in to run here nor a daemon that handed it over.
fn waiting(wait: &str) -> [(&str, &str); 3] {
    [
        ("SLOT_NODE_WAIT", wait),
        ("SLOT_REMOTE_FALLBACK", ""),
        ("SLOT_SPILLED", ""),
    ]
}

#[test]
fn a_job_sent_while_the_servers_daemon_is_away_runs_once_when_it_answers() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let a = checkout(t.path());
    let ssh = forced_ssh(t.path(), &d, &no_socket(t.path(), &d, 1));
    let marker = t.path().join("ran");
    let t0 = Instant::now();
    let out = remote_call_env(
        &d,
        &ssh,
        &a,
        &["sh", "-c", &marking(&marker)],
        &waiting("1s"),
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(7), "the job's own exit code: {err}");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default(),
        "ran\n",
        "the job ran once: {err}"
    );
    assert_eq!(err.matches(WAITING).count(), 1, "{err}");
    assert_eq!(err.matches("slot: synced to node").count(), 1, "{err}");
    assert_eq!(proxies(t.path()), 2, "sent twice: {err}");
    assert!(!err.contains("did not start"), "{err}");
    eprintln!("sent again after {:?}", t0.elapsed());
}

#[test]
fn a_job_whose_servers_daemon_stays_away_did_not_start_and_ran_nowhere() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let a = checkout(t.path());
    let ssh = forced_ssh(t.path(), &d, &no_socket(t.path(), &d, ALWAYS));
    let marker = t.path().join("ran");
    let out = remote_call_env(
        &d,
        &ssh,
        &a,
        &["sh", "-c", &marking(&marker)],
        &waiting("1s"),
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(
        err.contains(
            "slot: remote build on node did not start (its daemon did not answer within 1s); \
             the job did not run (set SLOT_REMOTE_FALLBACK=local to run it on this machine's daemon instead)"
        ),
        "{err}"
    );
    assert_eq!(err.matches(WAITING).count(), 1, "{err}");
    assert!(!marker.exists(), "the job ran: {err}");
    assert_eq!(proxies(t.path()), 2, "sent again at the bound: {err}");
    assert_eq!(d.logged(&["sh", "-c", &marking(&marker)]), 0, "{err}");
}

#[test]
fn a_job_whose_ssh_fails_does_not_wait_for_the_server() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let a = checkout(t.path());
    let ssh = forced_ssh(t.path(), &d, &proxy_step("read -r line; exit 255"));
    let t0 = Instant::now();
    let out = remote_call_env(&d, &ssh, &a, &["echo", "ran"], &waiting("30s"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.contains("slot: connection to daemon lost"), "{err}");
    assert!(!err.contains("slot: waiting for node"), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{err}");
    assert_eq!(proxies(t.path()), 1, "{err}");
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
}

#[test]
fn a_job_lost_after_its_process_started_is_not_sent_again() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let a = checkout(t.path());
    let start = r#"read -r line; printf '{"t":"start","id":1}\n'; exit 0"#;
    let ssh = forced_ssh(t.path(), &d, &proxy_step(start));
    let t0 = Instant::now();
    let out = remote_call_env(&d, &ssh, &a, &["echo", "ran"], &waiting("30s"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.contains("slot: connection to daemon lost"), "{err}");
    assert!(!err.contains("slot: waiting for node"), "{err}");
    assert_eq!(proxies(t.path()), 1, "its process may have run: {err}");
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
}

#[test]
fn with_a_wait_of_zero_a_job_whose_servers_daemon_is_away_fails_at_once() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let a = checkout(t.path());
    let ssh = forced_ssh(t.path(), &d, &no_socket(t.path(), &d, 1));
    let marker = t.path().join("ran");
    let out = remote_call_env(
        &d,
        &ssh,
        &a,
        &["sh", "-c", &marking(&marker)],
        &waiting("0"),
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    // The request is written before the step ends, or the write finds the
    // step gone: either way the call ends as it did before the wait.
    assert!(
        err.contains("slot: connection to daemon lost") || err.contains("slot: send: "),
        "{err}"
    );
    assert!(!err.contains("slot: waiting for node"), "{err}");
    assert!(!err.contains("did not start"), "{err}");
    assert!(!marker.exists(), "the job ran: {err}");
    assert_eq!(proxies(t.path()), 1, "{err}");
}

/// A job this machine's daemon handed to the server (`SLOT_SPILLED` with
/// `SLOT_PHASES_FILE`, as the daemon sets both) has that daemon's rule: its
/// client ends at once with exit 3, on which the daemon queues the job here
/// again. It does not wait, whatever `SLOT_NODE_WAIT` names: the wait would
/// count as the job's run time here.
#[test]
fn a_job_a_daemon_handed_to_the_server_does_not_wait_for_its_daemon() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let a = checkout(t.path());
    let ssh = forced_ssh(t.path(), &d, &no_socket(t.path(), &d, ALWAYS));
    let marker = t.path().join("ran");
    let phases = t.path().join("phases.json");
    let out = remote_call_env(
        &d,
        &ssh,
        &a,
        &["sh", "-c", &marking(&marker)],
        &[
            ("SLOT_NODE_WAIT", "2s"),
            ("SLOT_REMOTE_FALLBACK", "local"),
            ("SLOT_SPILLED", "1"),
            ("SLOT_PHASES_FILE", phases.to_str().unwrap()),
        ],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(!err.contains("slot: waiting for node"), "{err}");
    assert_eq!(proxies(t.path()), 1, "sent again: {err}");
    assert!(!marker.exists(), "the job ran: {err}");
    assert_eq!(d.logged(&["sh", "-c", &marking(&marker)]), 0, "{err}");
}

/// A wrapper in front of `slot` sets `SLOT_SPILLED` beside `SLOT_HOST` so
/// that a failed start never runs here; no daemon handed the job over
/// (no `SLOT_PHASES_FILE`). Its job waits for a server whose daemon drains
/// or restarts, as any other caller's does, and runs there once.
#[test]
fn a_job_a_wrapper_marks_spilled_waits_for_the_servers_daemon_and_runs_once() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let a = checkout(t.path());
    let ssh = forced_ssh(t.path(), &d, &no_socket(t.path(), &d, 2));
    let marker = t.path().join("ran");
    let out = remote_call_env(
        &d,
        &ssh,
        &a,
        &["sh", "-c", &marking(&marker)],
        &[
            ("SLOT_NODE_WAIT", "30s"),
            ("SLOT_REMOTE_FALLBACK", ""),
            ("SLOT_SPILLED", "1"),
        ],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(7), "the job's own exit code: {err}");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default(),
        "ran\n",
        "the job ran once: {err}"
    );
    assert_eq!(err.matches(WAITING).count(), 1, "{err}");
    assert_eq!(
        err.matches(DAEMON_AWAY).count(),
        1,
        "the server's line, once: {err}"
    );
    assert!(!err.contains("connection to daemon lost"), "{err}");
    assert_eq!(proxies(t.path()), 3, "sent three times: {err}");
    // The stand-in server's daemon is `d`: one job there, none elsewhere.
    assert_eq!(d.logged(&["sh", "-c", &marking(&marker)]), 1, "{err}");
}

/// The same wrapper's job, with a server whose daemon stays away: after
/// `SLOT_NODE_WAIT` it did not start, exit 3, and it ran nowhere.
#[test]
fn a_job_a_wrapper_marks_spilled_ends_after_the_wait_and_ran_nowhere() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let a = checkout(t.path());
    let ssh = forced_ssh(t.path(), &d, &no_socket(t.path(), &d, ALWAYS));
    let marker = t.path().join("ran");
    let t0 = Instant::now();
    let out = remote_call_env(
        &d,
        &ssh,
        &a,
        &["sh", "-c", &marking(&marker)],
        &[
            ("SLOT_NODE_WAIT", "2s"),
            ("SLOT_REMOTE_FALLBACK", "local"),
            ("SLOT_SPILLED", "1"),
        ],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(
        err.contains(
            "slot: remote build on node did not start (its daemon did not answer within 2s)"
        ),
        "{err}"
    );
    assert_eq!(err.matches(WAITING).count(), 1, "{err}");
    assert!(t0.elapsed() >= Duration::from_secs(2), "{:?}", t0.elapsed());
    assert!(!marker.exists(), "the job ran: {err}");
    assert_eq!(
        d.logged(&["sh", "-c", &marking(&marker)]),
        0,
        "the job ran: {err}"
    );
}

#[test]
fn a_signal_during_the_wait_ends_the_call_and_the_job_is_not_sent_again() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let a = checkout(t.path());
    let ssh = forced_ssh(t.path(), &d, &no_socket(t.path(), &d, ALWAYS));
    let mut child = remote_command(&d, &ssh, &a, &["echo", "ran"], &waiting("30s"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut err = String::new();
    let mut lines = BufReader::new(child.stderr.take().unwrap());
    while !err.contains(WAITING) {
        if lines.read_line(&mut err).unwrap() == 0 {
            break;
        }
    }
    let term = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    let status = child.wait().unwrap();
    assert!(err.contains(WAITING), "the call did not wait: {err}");
    assert!(term.success(), "the call had ended: {err}");
    assert_eq!(status.code(), Some(128 + 15), "{err}");
    assert_eq!(proxies(t.path()), 1, "sent again: {err}");
}

/// The step of `slot shell` whose daemon socket is missing or refuses:
/// one line with this prefix, and exit [`AWAY_EXIT`].
const DAEMON_AWAY: &str = "slot shell: daemon not answering: ";

/// The exit status of that step (sysexits' `EX_TEMPFAIL`).
const AWAY_EXIT: i32 = 75;

/// The proxy step of `slot shell` against `socket`, as sshd runs it.
fn forced_proxy(socket: &Path, state: &Path) -> std::process::Output {
    Command::new(SLOT)
        .arg("shell")
        .arg("--socket")
        .arg(socket)
        .arg("--state-dir")
        .arg(state.join("state"))
        .env("SSH_ORIGINAL_COMMAND", "slot proxy")
        .env("HOME", state)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// ssh passes on the exit status of the server's step and keeps 255 for a
/// failure of its own. The step without a daemon socket must not end with
/// that one, or a daemon that is away would read as a server not reached;
/// it names the daemon as not answering and exits with a code of its own.
#[test]
fn the_proxy_step_without_a_socket_does_not_exit_like_a_failed_ssh() {
    let t = tempfile::tempdir().unwrap();
    let gone = t.path().join("gone.sock");
    let direct = Command::new(SLOT)
        .arg("proxy")
        .arg("--socket")
        .arg(&gone)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        direct.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&direct.stderr)
    );
    let forced = forced_proxy(&gone, t.path());
    let err = String::from_utf8_lossy(&forced.stderr);
    assert_eq!(forced.status.code(), Some(AWAY_EXIT), "{err}");
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.starts_with(DAEMON_AWAY), "{err}");
    assert!(err.contains(&gone.display().to_string()), "{err}");
}

/// A socket file that no daemon listens on (a daemon that died without
/// removing it) refuses the connection: the same answer.
#[test]
fn the_proxy_step_with_a_socket_that_refuses_names_the_daemon_not_answering() {
    let t = tempfile::tempdir().unwrap();
    let dead = t.path().join("dead.sock");
    drop(std::os::unix::net::UnixListener::bind(&dead).unwrap());
    assert!(dead.exists());
    let forced = forced_proxy(&dead, t.path());
    let err = String::from_utf8_lossy(&forced.stderr);
    assert_eq!(forced.status.code(), Some(AWAY_EXIT), "{err}");
    assert!(err.starts_with(DAEMON_AWAY), "{err}");
}
