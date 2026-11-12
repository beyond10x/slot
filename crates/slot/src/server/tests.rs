use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use super::*;
use crate::proto::Workspace;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    server: Arc<Server>,
    socket: PathBuf,
}

fn start(tokens: usize) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cfg = Config {
        socket: root.join("s"),
        http: None,
        tokens,
        go_tokens: 4,
        state_dir: root.join("state"),
        ws_root: root.join("ws"),
        cache_root: root.join("cache"),
        memory_max: None,
        keep_jobs: 100,
        cpu_weight: None,
    };
    let server = Server::new(cfg.clone()).unwrap();
    let s = Arc::clone(&server);
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move { s.run().await })
    });
    for _ in 0..200 {
        if cfg.socket.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Fixture {
        _dir: dir,
        root,
        server,
        socket: cfg.socket,
    }
}

#[derive(Debug, Default)]
struct Res {
    out: String,
    err: String,
    info: String,
    code: i32,
}

fn send(sock: &Path, mut req: Request) -> UnixStream {
    if req.env.is_empty() && req.workspace.is_none() {
        req.env = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    }
    let mut c = UnixStream::connect(sock).unwrap();
    let mut b = serde_json::to_vec(&req).unwrap();
    b.push(b'\n');
    c.write_all(&b).unwrap();
    c
}

fn collect(c: UnixStream) -> Res {
    let mut r = Res {
        code: -999,
        ..Res::default()
    };
    for line in BufReader::new(c).lines() {
        let Ok(line) = line else { break };
        match serde_json::from_str::<Frame>(&line).unwrap() {
            Frame::Out { b } => r.out += &b,
            Frame::Err { b } => r.err += &b,
            Frame::Info { msg } => r.info += &msg,
            Frame::Exit { code } => {
                r.code = code;
                break;
            }
        }
    }
    r
}

fn run(f: &Fixture, req: Request) -> Res {
    collect(send(&f.socket, req))
}

fn sh(cwd: &Path, script: &str) -> Request {
    Request {
        argv: vec!["sh".into(), "-c".into(), script.into()],
        cwd: cwd.to_string_lossy().into_owned(),
        ..Request::default()
    }
}

#[test]
fn streams_output_and_exit_code() {
    let f = start(2);
    let r = run(&f, sh(&f.root, "pwd; echo oops >&2; exit 3"));
    assert_eq!(r.code, 3);
    assert_eq!(r.out.trim(), f.root.to_string_lossy());
    assert_eq!(r.err, "oops\n");
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    assert!(log.contains("\"exit\":3"), "{log}");
}

#[test]
fn jobserver_env_reaches_command_and_client_jobs_are_dropped() {
    let f = start(2);
    let mut req = sh(
        &f.root,
        "echo $CARGO_MAKEFLAGS; echo jobs=$CARGO_BUILD_JOBS; echo tag=$SLOT_JOB",
    );
    req.env = std::env::vars()
        .map(|(k, v)| format!("{k}={v}"))
        .chain(["CARGO_BUILD_JOBS=9".to_string()])
        .collect();
    let r = run(&f, req);
    let auth = format!("--jobserver-auth=fifo:{}", f.server.pool().path().display());
    assert!(r.out.contains(&auth), "{}", r.out);
    assert!(r.out.contains("jobs=\n"), "{}", r.out);
    assert!(r.out.contains("tag=1\n"), "{}", r.out);
}

#[test]
fn second_job_waits_for_the_slot() {
    let f = start(1);
    let first = send(&f.socket, sh(&f.root, "sleep 1"));
    std::thread::sleep(Duration::from_millis(150));
    let t0 = Instant::now();
    let r = run(&f, sh(&f.root, "true"));
    assert_eq!(r.code, 0);
    assert!(
        t0.elapsed() >= Duration::from_millis(700),
        "{:?}",
        t0.elapsed()
    );
    assert_eq!(collect(first).code, 0);
}

#[test]
fn client_disconnect_kills_job_and_returns_token() {
    let f = start(1);
    let marker = f.root.join("pid");
    let c = send(
        &f.socket,
        sh(
            &f.root,
            &format!("echo $$ > {}; sleep 30", marker.display()),
        ),
    );
    let mut pid = 0;
    for _ in 0..200 {
        if let Ok(s) = std::fs::read_to_string(&marker)
            && let Ok(p) = s.trim().parse::<i32>()
        {
            pid = p;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(pid > 0, "job did not start");
    drop(c);
    let mut gone = false;
    for _ in 0..200 {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(gone, "job still running after client disconnect");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.server.pool().free(), 1);
}

#[test]
fn workspace_is_confined_and_paths_rewritten() {
    let f = start(1);
    let bad = Request {
        argv: vec!["true".into()],
        workspace: Some(Workspace {
            id: "../x".into(),
            rel_cwd: ".".into(),
            client_root: String::new(),
        }),
        ..Request::default()
    };
    assert_eq!(run(&f, bad).code, 2);
    std::fs::create_dir_all(f.root.join("ws/abc")).unwrap();
    let req = Request {
        argv: vec!["pwd".into()],
        workspace: Some(Workspace {
            id: "abc".into(),
            rel_cwd: "../../..".into(),
            client_root: "/client".into(),
        }),
        ..Request::default()
    };
    let r = run(&f, req);
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out.trim(), "/client");
}

#[test]
fn remote_job_gets_server_path_and_client_flags() {
    let f = start(1);
    std::fs::create_dir_all(f.root.join("ws/w1")).unwrap();
    let req = Request {
        argv: vec![
            "sh".into(),
            "-c".into(),
            "echo path=$PATH; echo flags=$RUSTFLAGS".into(),
        ],
        env: vec!["RUSTFLAGS=-Cx".into()],
        workspace: Some(Workspace {
            id: "w1".into(),
            rel_cwd: ".".into(),
            client_root: "/c".into(),
        }),
        ..Request::default()
    };
    let r = run(&f, req);
    assert_eq!(r.code, 0, "{r:?}");
    assert!(!r.out.contains("path=\n"), "{}", r.out);
    assert!(r.out.contains("flags=-Cx"), "{}", r.out);
}

#[test]
fn go_job_holds_at_most_the_pool() {
    let f = start(2);
    let r = run(&f, sh(&f.root, "true"));
    assert_eq!(r.code, 0);
    let mut req = sh(&f.root, "x");
    req.argv = vec!["go".into(), "env".into(), "GOFLAGS".into()];
    let r = run(&f, req);
    if r.code == 127 {
        return; // go not installed
    }
    assert!(r.out.contains("-p=2"), "{r:?}");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(f.server.pool().free(), 2);
}

#[test]
fn history_survives_restart() {
    let f = start(1);
    assert_eq!(run(&f, sh(&f.root, "exit 4")).code, 4);
    let again = Server::new(Config {
        socket: f.root.join("s2"),
        http: None,
        tokens: 1,
        go_tokens: 4,
        state_dir: f.root.join("state"),
        ws_root: f.root.join("ws"),
        cache_root: f.root.join("cache"),
        memory_max: None,
        keep_jobs: 100,
        cpu_weight: None,
    })
    .unwrap();
    let st = again.state();
    assert_eq!(st.done.len(), 1);
    assert_eq!(st.done[0].exit, 4);
}
