//! `slot --tokens N`, `slot run --tokens N` and `SLOT_TOKENS`, and the same
//! for `--priority` and `SLOT_PRIORITY`, reach the request the client sends;
//! `--local` runs without the daemon. A remote job that cannot start runs
//! here only with `SLOT_REMOTE_FALLBACK=local`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use slot::proto::Request;

const SLOT: &str = env!("CARGO_BIN_EXE_slot");

/// Runs `slot args…` against a fake daemon that answers exit 0, and
/// returns the request it got, or `None` when none came.
fn request(args: &[&str], env: &[(&str, &str)]) -> Option<Request> {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("sock");
    let l = UnixListener::bind(&socket).unwrap();
    l.set_nonblocking(true).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..300 {
            if let Ok((c, _)) = l.accept() {
                c.set_nonblocking(false).unwrap();
                let mut line = String::new();
                BufReader::new(&c).read_line(&mut line).unwrap();
                tx.send(serde_json::from_str::<Request>(&line).unwrap())
                    .unwrap();
                (&c).write_all(b"{\"t\":\"exit\",\"code\":0}\n").unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let mut cmd = Command::new(SLOT);
    cmd.args(args)
        .env("SLOT_SOCKET", &socket)
        .env_remove("SLOT_JOB")
        .env_remove("SLOT_HOST")
        .env_remove("SLOT_TOKENS")
        .env_remove("SLOT_PRIORITY")
        .current_dir(dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let st = cmd.status().unwrap();
    assert!(st.success(), "slot {args:?}: {st}");
    rx.recv_timeout(Duration::from_millis(100)).ok()
}

#[test]
fn the_flag_and_the_variable_reach_the_request() {
    let tokens =
        |args: &[&str], env: &[(&str, &str)]| request(args, env).expect("no request").tokens;
    assert_eq!(tokens(&["--tokens", "3", "true"], &[]), Some(3));
    assert_eq!(tokens(&["run", "--tokens", "4", "true"], &[]), Some(4));
    assert_eq!(tokens(&["true"], &[("SLOT_TOKENS", "2")]), Some(2));
    assert_eq!(tokens(&["run", "true"], &[("SLOT_TOKENS", "2")]), Some(2));
    assert_eq!(
        tokens(&["--tokens", "5", "true"], &[("SLOT_TOKENS", "2")]),
        Some(5),
        "the flag wins over the variable"
    );
    assert_eq!(
        tokens(&["run", "--tokens", "6", "true"], &[("SLOT_TOKENS", "2")]),
        Some(6)
    );
    assert_eq!(tokens(&["true"], &[]), None);
}

#[test]
fn the_priority_flag_and_variable_reach_the_request() {
    use slot::types::Priority;
    let level =
        |args: &[&str], env: &[(&str, &str)]| request(args, env).expect("no request").priority;
    assert_eq!(
        level(&["--priority", "high", "true"], &[]),
        Some(Priority::High)
    );
    assert_eq!(
        level(&["run", "--priority", "low", "true"], &[]),
        Some(Priority::Low)
    );
    assert_eq!(
        level(&["true"], &[("SLOT_PRIORITY", "low")]),
        Some(Priority::Low)
    );
    assert_eq!(
        level(&["run", "true"], &[("SLOT_PRIORITY", "high")]),
        Some(Priority::High)
    );
    assert_eq!(
        level(
            &["--priority", "normal", "true"],
            &[("SLOT_PRIORITY", "low")]
        ),
        Some(Priority::Normal),
        "the flag wins over the variable"
    );
    assert_eq!(
        level(
            &["run", "--priority", "high", "true"],
            &[("SLOT_PRIORITY", "low")]
        ),
        Some(Priority::High)
    );
    assert_eq!(level(&["true"], &[]), None);
}

#[test]
fn local_ignores_tokens() {
    assert!(request(&["--local", "--tokens", "3", "true"], &[]).is_none());
}

/// Runs `slot true` with `SLOT_HOST` set to a host whose ssh always fails,
/// in a git checkout, against a fake daemon. Returns the exit code, stderr
/// and the request the daemon got, if any.
fn unreachable_host(env: &[(&str, &str)]) -> (i32, String, Option<Request>) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&tree).unwrap();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&tree)
        .status()
        .unwrap();
    assert!(git.success());
    let socket = dir.path().join("sock");
    let l = UnixListener::bind(&socket).unwrap();
    l.set_nonblocking(true).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..1000 {
            if let Ok((c, _)) = l.accept() {
                c.set_nonblocking(false).unwrap();
                let mut line = String::new();
                BufReader::new(&c).read_line(&mut line).unwrap();
                tx.send(serde_json::from_str::<Request>(&line).unwrap())
                    .unwrap();
                (&c).write_all(b"{\"t\":\"exit\",\"code\":0}\n").unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let mut cmd = Command::new(SLOT);
    cmd.arg("true")
        .env("SLOT_SOCKET", &socket)
        .env("SLOT_HOST", "unreachable.invalid")
        .env("SLOT_SSH", "false")
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", dir.path())
        .env_remove("SLOT_JOB")
        .env_remove("SLOT_SPILLED")
        .env_remove("SLOT_REMOTE_FALLBACK")
        .current_dir(&tree)
        .stdout(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    let got = rx.recv_timeout(Duration::from_millis(200)).ok();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        got,
    )
}

#[test]
fn a_remote_job_that_cannot_start_does_not_run_here() {
    let (code, err, got) = unreachable_host(&[]);
    assert_eq!(code, 3, "stderr: {err}");
    assert!(
        got.is_none(),
        "the job went to this machine's daemon: {err}"
    );
    let line = err
        .lines()
        .find(|l| l.contains("did not start"))
        .unwrap_or_else(|| panic!("no refusal line: {err}"));
    assert!(line.contains("unreachable.invalid"), "{line}");
    assert!(line.contains("the job did not run"), "{line}");
    assert!(line.contains("SLOT_REMOTE_FALLBACK=local"), "{line}");
}

#[test]
fn the_fallback_to_this_machine_is_opt_in() {
    let (code, err, got) = unreachable_host(&[("SLOT_REMOTE_FALLBACK", "local")]);
    assert_eq!(code, 0, "stderr: {err}");
    let req = got.unwrap_or_else(|| panic!("no request reached the daemon: {err}"));
    assert!(
        req.local_reason.starts_with("sync failed"),
        "{}",
        req.local_reason
    );
    assert!(err.contains("running it on this machine's daemon"), "{err}");
}
