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
    start_with(tokens, crate::admission::Limits::default())
}

fn start_with(tokens: usize, admission: crate::admission::Limits) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cfg = Config {
        socket: root.join("s"),
        http: None,
        tokens,
        go_tokens: 4,
        test_threads: 2,
        cpu_quota: false,
        state_dir: root.join("state"),
        ws_root: root.join("ws"),
        cache_root: root.join("cache"),
        memory_max: None,
        keep_jobs: 100,
        cpu_weight: None,
        call_headroom: 0,
        call_off_after: Duration::from_secs(120),
        call_ignore: Vec::new(),
        sccache_watch: false,
        admission,
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
    started: bool,
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
            Frame::Start { .. } => r.started = true,
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
fn a_pending_upgrade_holds_admission() {
    let f = start(1);
    let running = send(&f.socket, sh(&f.root, "sleep 1"));
    std::thread::sleep(Duration::from_millis(150));
    let queued_before = send(&f.socket, sh(&f.root, "echo before"));
    std::thread::sleep(Duration::from_millis(100));
    f.server.hold_admission();
    let queued_after = send(&f.socket, sh(&f.root, "echo after"));
    let r = collect(running);
    assert_eq!((r.code, r.started), (0, true), "the running job finishes");
    std::thread::sleep(Duration::from_millis(500));
    assert!(f.server.nothing_running(), "the moment to re-execute");
    let states: Vec<JobState> = {
        let g = f.server.inner.lock().unwrap();
        g.active.values().map(|l| l.job.state).collect()
    };
    assert_eq!(states, [JobState::Queued, JobState::Queued]);
    assert_eq!(f.server.pool().free(), 1, "the slot went back to the pool");
    for c in [queued_before, queued_after] {
        c.set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let r = collect(c);
        assert!(!r.started, "admitted during an upgrade: {r:?}");
        assert_eq!(r.out, "");
    }
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
        test_threads: 2,
        cpu_quota: false,
        state_dir: f.root.join("state"),
        ws_root: f.root.join("ws"),
        cache_root: f.root.join("cache"),
        memory_max: None,
        keep_jobs: 100,
        cpu_weight: None,
        call_headroom: 0,
        call_off_after: Duration::from_secs(120),
        call_ignore: Vec::new(),
        sccache_watch: false,
        admission: crate::admission::Limits::default(),
    })
    .unwrap();
    let st = again.state();
    assert_eq!(st.done.len(), 1);
    assert_eq!(st.done[0].exit, 4);
}

#[test]
fn headroom_lowers_the_limit_and_is_saved() {
    let f = start(2);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let v = rt.block_on(Arc::clone(&f.server).set_headroom(50));
    assert_eq!(v.effective_tokens, 1);
    assert_eq!(v.headroom_percent, 50);
    assert_eq!(v.cpu_quota_percent, None, "cpu quota is off in tests");
    assert_eq!(f.server.state().limit, 1);
    let first = send(&f.socket, sh(&f.root, "sleep 1"));
    std::thread::sleep(Duration::from_millis(150));
    let t0 = Instant::now();
    let r = run(&f, sh(&f.root, "true"));
    assert_eq!(r.code, 0);
    assert!(
        t0.elapsed() >= Duration::from_millis(700),
        "second job ran beside the first with 1 of 2 tokens circulating: {:?}",
        t0.elapsed()
    );
    assert_eq!(collect(first).code, 0);
    assert_eq!(limits::load(&f.root.join("state")).headroom_percent, 50);
    let v = rt.block_on(Arc::clone(&f.server).set_headroom(0));
    assert_eq!(v.effective_tokens, 2);
}

/// Reads frames until an `info` frame contains `needle`.
fn read_info_until(c: &UnixStream, needle: &str) -> String {
    let mut seen = String::new();
    for line in BufReader::new(c.try_clone().unwrap()).lines() {
        let Ok(line) = line else { break };
        if let Frame::Info { msg } = serde_json::from_str::<Frame>(&line).unwrap() {
            seen += &msg;
            if msg.contains(needle) {
                break;
            }
        }
    }
    seen
}

#[test]
fn a_job_waits_while_the_disk_is_short_and_can_be_cancelled() {
    let f = start_with(
        2,
        crate::admission::Limits {
            min_free_memory: 0,
            min_free_disk: u64::MAX,
        },
    );
    let c = send(&f.socket, sh(&f.root, "echo ran"));
    let info = read_info_until(&c, "waiting:");
    assert!(info.contains("free on the filesystem of"), "{info}");
    let st = f.server.state();
    assert_eq!(st.active.len(), 1);
    assert_eq!(st.active[0].state, JobState::Queued);
    assert!(st.active[0].wait_reason.contains("builds need"), "{st:?}");
    drop(c);
    for _ in 0..100 {
        if f.server.state().active.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let st = f.server.state();
    assert!(st.active.is_empty(), "{st:?}");
    assert_eq!(st.done.last().unwrap().state, JobState::Cancelled);
}

#[test]
fn memory_short_holds_a_job_only_while_another_runs() {
    let f = start_with(
        2,
        crate::admission::Limits {
            min_free_memory: u64::MAX / 4,
            min_free_disk: 0,
        },
    );
    let alone = run(&f, sh(&f.root, "echo alone"));
    assert_eq!(
        (alone.code, alone.out.as_str()),
        (0, "alone\n"),
        "alone: runs"
    );
    let first = send(&f.socket, sh(&f.root, "sleep 1.5"));
    std::thread::sleep(Duration::from_millis(300));
    let t0 = Instant::now();
    let second = run(&f, sh(&f.root, "echo second"));
    assert_eq!(second.code, 0);
    assert!(second.info.contains("memory available"), "{}", second.info);
    assert!(
        t0.elapsed() >= Duration::from_millis(900),
        "ran beside the first: {:?}",
        t0.elapsed()
    );
    assert_eq!(collect(first).code, 0);
}

#[test]
fn the_call_headroom_is_saved_beside_the_headroom() {
    let f = start(2);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(Arc::clone(&f.server).set_headroom(20));
    let v = rt.block_on(Arc::clone(&f.server).update_limits(limits::Request {
        headroom_percent: None,
        call_headroom_percent: Some(30),
    }));
    assert_eq!(v.call_headroom_percent, 30);
    assert_eq!(v.headroom_percent, 20, "no call: the headroom stays");
    let saved = limits::load(&f.root.join("state"));
    assert_eq!(
        (saved.headroom_percent, saved.call_headroom_percent),
        (20, Some(30))
    );
    let v = rt.block_on(Arc::clone(&f.server).update_limits(limits::Request {
        headroom_percent: Some(10),
        call_headroom_percent: Some(0),
    }));
    assert_eq!((v.headroom_percent, v.call_headroom_percent), (10, 0));
    assert_eq!(
        limits::load(&f.root.join("state")).call_headroom_percent,
        Some(0)
    );
}

#[test]
fn settings_change_needs_json_from_the_daemons_own_origin() {
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    assert_eq!(
        http::refuse_change(&h, 7411).map(|r| r.0),
        Some(StatusCode::UNSUPPORTED_MEDIA_TYPE)
    );
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    assert_eq!(http::refuse_change(&h, 7411), None);
    h.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://example.com"),
    );
    assert_eq!(
        http::refuse_change(&h, 7411).map(|r| r.0),
        Some(StatusCode::FORBIDDEN)
    );
    h.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://127.0.0.1:7411"),
    );
    assert_eq!(http::refuse_change(&h, 7411), None);
}
