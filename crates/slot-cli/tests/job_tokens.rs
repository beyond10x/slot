//! `slot --tokens N`, `slot run --tokens N` and `SLOT_TOKENS` reach the
//! request the client sends; `--local` runs without the daemon.

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
fn local_ignores_tokens() {
    assert!(request(&["--local", "--tokens", "3", "true"], &[]).is_none());
}
