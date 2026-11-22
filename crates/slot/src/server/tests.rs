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
    start_cfg(tokens, |c| c.admission = admission)
}

fn start_cfg(tokens: usize, change: impl FnOnce(&mut Config)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut cfg = config(&root, tokens);
    change(&mut cfg);
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

fn config(root: &Path, tokens: usize) -> Config {
    Config {
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
        admission: crate::admission::Limits::default(),
        upgrade_hold: Duration::from_secs(60),
        promote_after: Duration::from_secs(1800),
        clock: now_ms,
        target_poll: Duration::from_millis(40),
        target_settle: Duration::from_millis(250),
        token_watch: TOKEN_WATCH,
        resource_poll: RESOURCE_POLL,
        cpu_busy: None,
        host_cpus: None,
        cpu_hold_max: cpu_limit::HOLD_MAX,
        cache_budget: 0,
        evict: crate::evict::Settings {
            floor: 0,
            margin: 0,
            min_idle: Duration::from_secs(7200),
            dry_run: true,
        },
        disk_free: None,
        exec_shim: None,
        handover: true,
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
    assert_eq!(f.server.job_parallel_view().job_parallel, None);
}

/// What a job's own shell sees of the four variables the daemon owns, for a
/// client that set all four itself. `tokens` are the slots the job names.
fn owned_env(f: &Fixture, tokens: Option<u32>) -> String {
    let mut req = sh(
        &f.root,
        "echo jobs=${CARGO_BUILD_JOBS-unset}; echo make=${MAKEFLAGS-unset}; \
         echo mflags=${MFLAGS-unset}; echo cargo=${CARGO_MAKEFLAGS-unset}",
    );
    req.tokens = tokens;
    req.env = std::env::vars()
        .map(|(k, v)| format!("{k}={v}"))
        .chain(
            [
                "CARGO_BUILD_JOBS=9",
                "MAKEFLAGS=-j8",
                "MFLAGS=-j8",
                "CARGO_MAKEFLAGS=-j --jobserver-auth=fifo:/theirs",
            ]
            .map(String::from),
        )
        .collect();
    let r = run(f, req);
    assert_eq!(r.code, 0, "{r:?}");
    r.out
}

fn set_job_parallel(f: &Fixture, n: Option<usize>) -> Option<usize> {
    f.server
        .set_job_parallel(&job_parallel::Request { job_parallel: n })
        .unwrap()
        .job_parallel
}

/// With `slot job-parallel 3` a job that starts gets a bound of its own in
/// place of the pool's jobserver: `CARGO_BUILD_JOBS` and `MAKEFLAGS=-j` at 3,
/// or at the slots it holds if those are more, and nothing that names the
/// pool's FIFO. A client's own values never get through. Switched off, the
/// next job gets the pool's jobserver again.
#[test]
fn with_a_bound_per_job_a_job_gets_its_own_jobserver_and_not_the_pools() {
    let f = start(6);
    let pool = format!(
        "-j --jobserver-auth=fifo:{}",
        f.server.pool().path().display()
    );
    let off = format!("jobs=unset\nmake={pool}\nmflags=unset\ncargo={pool}\n");
    let own = |p: usize| format!("jobs={p}\nmake=-j{p}\nmflags=unset\ncargo=unset\n");
    assert_eq!(owned_env(&f, None), off, "off: the pool's jobserver");
    assert_eq!(set_job_parallel(&f, Some(3)), Some(3));
    assert_eq!(owned_env(&f, None), own(3), "a job of one slot");
    assert_eq!(owned_env(&f, Some(2)), own(3), "fewer slots than the bound");
    assert_eq!(owned_env(&f, Some(5)), own(5), "a job that holds 5 slots");
    assert_eq!(set_job_parallel(&f, None), None);
    assert_eq!(owned_env(&f, Some(5)), off, "off again: as before");
    wait_for_jobs(&f, "every slot back", || f.server.pool().free() == 6);
}

/// Status and text of the answer to `PUT /api/job-parallel` with `body`.
fn put_job_parallel(f: &Fixture, content_type: &str, body: &str) -> (u16, String) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        content_type.parse().unwrap(),
    );
    let state = axum::extract::State(Arc::clone(&f.server));
    let body = axum::body::Bytes::from(body.to_owned());
    answer(http::job_parallel_put(state, headers, body))
}

/// Status and text of the answer to `GET /api/job-parallel`.
fn get_job_parallel(f: &Fixture) -> (u16, String) {
    use axum::response::IntoResponse;
    let state = axum::extract::State(Arc::clone(&f.server));
    answer(async move { http::job_parallel_get(state).await.into_response() })
}

/// The bound set through `PUT /api/job-parallel`, the call `slot
/// job-parallel` makes, is saved beside the headroom, read back by `GET`,
/// shown in the state `slot status` reads and in force in the daemon that
/// starts next. A value under 1 is refused; `null` switches it off.
#[test]
fn the_bound_per_job_is_set_saved_and_read_back() {
    let f = start(2);
    let json = "application/json";
    let view = |(status, text): (u16, String)| {
        assert_eq!(status, 200, "{text}");
        let v: job_parallel::View = serde_json::from_str(&text).unwrap();
        v.job_parallel
    };
    let saved = || std::fs::read_to_string(f.root.join("state").join(limits::FILE)).unwrap();
    let restarted = |socket: &str| {
        Server::new(Config {
            socket: f.root.join(socket),
            ..config(&f.root, 2)
        })
        .unwrap()
    };
    assert_eq!(view(get_job_parallel(&f)), None);
    assert_eq!(f.server.state().job_parallel, None);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(Arc::clone(&f.server).set_headroom(50));
    assert!(!saved().contains("job_parallel"), "{}", saved());

    assert_eq!(
        view(put_job_parallel(&f, json, r#"{"job_parallel":4}"#)),
        Some(4)
    );
    assert_eq!(view(get_job_parallel(&f)), Some(4));
    assert!(saved().contains(r#""job_parallel":4"#), "{}", saved());
    assert!(saved().contains(r#""headroom_percent":50"#), "{}", saved());
    assert_eq!(f.server.state().job_parallel, Some(4));
    rt.block_on(Arc::clone(&f.server).set_headroom(25));
    assert!(
        saved().contains(r#""job_parallel":4"#),
        "a headroom change keeps the bound: {}",
        saved()
    );
    let next = restarted("s2");
    assert_eq!(next.job_parallel_view().job_parallel, Some(4));
    assert_eq!(next.limits().headroom_percent, 25);

    for bad in [
        "{}",
        r#"{"job_parallel":0}"#,
        r#"{"job_parallel":-2}"#,
        r#"{"job_parallel":"4"}"#,
        r#"{"job_parallel":1.5}"#,
        "not json",
        "",
    ] {
        let (status, text) = put_job_parallel(&f, json, bad);
        assert_eq!(status, 400, "{bad}: {text}");
        assert!(text.starts_with("slot: "), "{bad}: {text}");
        assert_eq!(
            f.server.job_parallel_view().job_parallel,
            Some(4),
            "{bad} changed nothing"
        );
    }
    let (status, _) = put_job_parallel(&f, "text/plain", r#"{"job_parallel":2}"#);
    assert_eq!(status, 415, "a body a page can post without a preflight");
    assert_eq!(f.server.job_parallel_view().job_parallel, Some(4));

    assert_eq!(
        view(put_job_parallel(&f, json, r#"{"job_parallel":null}"#)),
        None,
        "null switches it off"
    );
    assert!(!saved().contains("job_parallel"), "{}", saved());
    assert_eq!(f.server.state().job_parallel, None);
    assert_eq!(restarted("s3").job_parallel_view().job_parallel, None);
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
fn a_hold_that_runs_out_admits_the_queued_jobs() {
    let f = start(1);
    let running = send(&f.socket, sh(&f.root, "sleep 0.5"));
    std::thread::sleep(Duration::from_millis(150));
    f.server.hold_admission();
    let queued = send(&f.socket, sh(&f.root, "echo after"));
    std::thread::sleep(Duration::from_millis(100));
    assert!(!f.server.upgrade_step(upgrade::Step::Release));
    assert_eq!(collect(running).code, 0);
    let r = collect(queued);
    assert_eq!((r.code, r.started, r.out.as_str()), (0, true, "after\n"));
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

fn cancel(f: &Fixture, id: i64) -> Res {
    run(
        f,
        Request {
            cancel: Some(id),
            ..Request::default()
        },
    )
}

#[test]
fn slot_cancel_stops_a_queued_and_a_running_job() {
    let f = start(1);
    let marker = f.root.join("pid");
    let running = send(
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
    let queued = send(&f.socket, sh(&f.root, "true"));
    std::thread::sleep(Duration::from_millis(300));

    let r = cancel(&f, 2);
    assert_eq!(r.code, 0, "{r:?}");
    let q = collect(queued);
    assert_eq!(q.code, 130, "{q:?}");
    assert!(q.info.contains("cancelled by `slot cancel`"), "{q:?}");
    assert!(!q.started);

    let r = cancel(&f, 1);
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(collect(running).code, 130);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.server.pool().free(), 1);

    let r = cancel(&f, 99);
    assert_eq!(r.code, 1);
    assert!(r.info.contains("not queued or running"), "{r:?}");
}

#[test]
fn a_command_not_on_path_is_refused_without_queueing() {
    let f = start(1);
    let mut req = sh(&f.root, "x");
    req.argv = vec!["cancle".into(), "12".into()];
    let r = run(&f, req);
    assert_eq!(r.code, 127, "{r:?}");
    assert!(
        r.info.contains("cancle: not found in PATH; not queued"),
        "{r:?}"
    );
    assert_eq!(run(&f, sh(&f.root, "true")).code, 0);
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 1, "{log}");
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
fn requested_tokens_replace_the_toolchains() {
    let f = start(4);
    let mut req = sh(&f.root, "true");
    req.tokens = Some(3);
    let r = run(&f, req);
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(f.server.state().done[0].tokens, 3);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(f.server.pool().free(), 4, "all slots back");
}

#[test]
fn requested_tokens_are_capped_at_the_limit() {
    let f = start(4);
    let rt = tokio::runtime::Runtime::new().unwrap();
    assert_eq!(
        rt.block_on(Arc::clone(&f.server).set_headroom(50))
            .effective_tokens,
        2
    );
    let mut req = sh(&f.root, "true");
    req.tokens = Some(9);
    let r = run(&f, req);
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(f.server.state().done[0].tokens, 2);
    assert!(r.info.contains("capped at 2"), "{r:?}");
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
        upgrade_hold: Duration::from_secs(60),
        promote_after: Duration::from_secs(1800),
        clock: now_ms,
        target_poll: Duration::from_millis(40),
        target_settle: Duration::from_millis(250),
        token_watch: TOKEN_WATCH,
        resource_poll: RESOURCE_POLL,
        cpu_busy: None,
        host_cpus: None,
        cpu_hold_max: cpu_limit::HOLD_MAX,
        cache_budget: 0,
        evict: crate::evict::Settings {
            floor: 0,
            margin: 0,
            min_idle: Duration::from_secs(7200),
            dry_run: true,
        },
        disk_free: None,
        exec_shim: None,
        handover: true,
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
        headroom_base: None,
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
        headroom_base: None,
    }));
    assert_eq!((v.headroom_percent, v.call_headroom_percent), (10, 0));
    assert_eq!(
        limits::load(&f.root.join("state")).call_headroom_percent,
        Some(0)
    );
}

#[test]
fn the_headroom_base_is_set_at_runtime_saved_and_shown() {
    use crate::types::HeadroomBase;
    let f = start(4);
    let rt = tokio::runtime::Runtime::new().unwrap();
    assert_eq!(f.server.limits().headroom_base, HeadroomBase::Pool);
    rt.block_on(Arc::clone(&f.server).set_headroom(25));
    let v = rt.block_on(Arc::clone(&f.server).update_limits(limits::Request {
        headroom_base: Some(HeadroomBase::Machine),
        ..limits::Request::default()
    }));
    assert_eq!(v.headroom_base, HeadroomBase::Machine);
    assert_eq!((v.headroom_percent, v.effective_tokens), (25, 3));
    let saved = limits::load(&f.root.join("state"));
    assert_eq!(
        (saved.headroom_percent, saved.base),
        (25, HeadroomBase::Machine)
    );
    assert_eq!(f.server.state().headroom_base, HeadroomBase::Machine);
    let v = rt.block_on(Arc::clone(&f.server).update_limits(limits::Request {
        headroom_base: Some(HeadroomBase::Pool),
        ..limits::Request::default()
    }));
    assert_eq!(v.headroom_base, HeadroomBase::Pool);
    assert_eq!(limits::load(&f.root.join("state")).base, HeadroomBase::Pool);
    let st = f.server.state();
    assert_eq!((st.target_cores, st.outside.len()), (0.0, 0));
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

/// Eviction deletes only tagged target directories a job built into:
/// not their siblings, not a directory the log names without a Cargo tag,
/// not a symlink to a target directory, not a busy or recent one.
#[test]
fn eviction_touches_only_target_dirs_jobs_built_into() {
    let d = tempfile::tempdir().unwrap();
    let tag = |t: &Path| {
        std::fs::create_dir_all(t).unwrap();
        std::fs::write(
            t.join("CACHEDIR.TAG"),
            format!("{}\n", crate::evict::CACHEDIR_SIGNATURE),
        )
        .unwrap();
        std::fs::write(t.join("blob"), vec![0u8; 4096]).unwrap();
    };
    let w = d.path().join("w");
    let old = w.join("old/target");
    let untagged = w.join("untagged/target");
    let link = w.join("link/target");
    let busy = w.join("busy/target");
    let recent = w.join("recent/target");
    let unseen = w.join("unseen/target");
    for t in [&old, &busy, &recent, &unseen] {
        tag(t);
    }
    std::fs::create_dir_all(&untagged).unwrap();
    std::fs::write(untagged.join("keep"), b"x").unwrap();
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&unseen, &link).unwrap();
    let src = w.join("old/src/main.rs");
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    std::fs::write(&src, b"fn main() {}").unwrap();

    let h = 3_600_000;
    let now = 100 * h;
    let mut seen = HashMap::new();
    for (p, repo, ms) in [
        (&old, "old", now - 50 * h),
        (&untagged, "untagged", now - 60 * h),
        (&link, "link", now - 70 * h),
        (&busy, "busy", now - 80 * h),
        (&recent, "recent", now - h),
    ] {
        seen.insert(p.clone(), (repo.to_string(), ms));
    }
    let s = crate::evict::Settings {
        floor: u64::MAX / 4,
        margin: 0,
        min_idle: Duration::from_secs(7200),
        dry_run: false,
    };
    let busy_repos: HashSet<String> = ["busy".to_string()].into();
    let (_, planned) = evict_pass(&seen, &busy_repos, &HashSet::new(), now, s);
    let paths: Vec<&Path> = planned.iter().map(|(c, _)| c.path.as_path()).collect();
    assert_eq!(paths, [old.as_path()]);
    assert!(planned[0].1 >= 4096);
    for (c, _) in &planned {
        assert_eq!(crate::evict::remove(&c.path), Ok("deleted"));
    }
    assert!(!old.exists());
    assert!(src.exists(), "the source beside the target dir stays");
    for kept in [&untagged, &busy, &recent, &unseen] {
        assert!(kept.exists(), "{} kept", kept.display());
    }
    assert!(untagged.join("keep").exists());
    assert!(std::fs::symlink_metadata(&link).is_ok());
}

#[test]
fn a_job_logs_the_project_of_its_directory() {
    let f = start(2);
    let repo = f.root.join("api");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        &["init", "-q"][..],
        &[
            "remote",
            "add",
            "origin",
            "git@forge.example:team-a/api.git",
        ][..],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    // git runs off the request path; a job that lasts a second has its project.
    let r = run(&f, sh(&repo, "sleep 1"));
    assert_eq!(r.code, 0);
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    assert!(log.contains("\"project\":\"team-a\""), "{log}");
    let u = crate::usage::aggregate(
        log.as_bytes(),
        0,
        i64::MAX,
        crate::usage::By::Project,
        &mut |_| String::new(),
    );
    assert_eq!(u.rows.len(), 1);
    assert_eq!(u.rows[0].key, "team-a");
}

/// Polls `cond` for up to 5 s.
fn wait_for(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..500 {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// A job that holds its slot until the file `go` exists in `cwd`.
fn holder(f: &Fixture) -> UnixStream {
    let c = send(
        &f.socket,
        sh(&f.root, "while [ ! -e go ]; do sleep 0.02; done"),
    );
    wait_for("the holder to run", || {
        f.server
            .state()
            .active
            .iter()
            .any(|j| j.state == JobState::Running)
    });
    c
}

/// Queues a job of repository `repo` at its own level `level` that writes
/// `name` to the file `order`, and waits until it waits for a slot.
fn queue(f: &Fixture, name: &str, repo: &str, level: Option<Priority>) -> UnixStream {
    let before = f.server.pool().waiting();
    let mut r = sh(&f.root, &format!("echo {name} >> order"));
    r.repo = repo.into();
    r.priority = level;
    let c = send(&f.socket, r);
    wait_for("the job to queue", || {
        f.server.pool().waiting() == before + 1
    });
    c
}

/// Lets the holder go, waits for every job, and returns the names in the
/// order the jobs ran.
fn run_all(f: &Fixture, holder: UnixStream, queued: Vec<UnixStream>) -> Vec<String> {
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(holder).code, 0);
    for c in queued {
        assert_eq!(collect(c).code, 0);
    }
    std::fs::read_to_string(f.root.join("order"))
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

/// Level and promotion of the queued job of repository `repo`.
fn level_of(f: &Fixture, repo: &str) -> (Priority, bool) {
    let st = f.server.state();
    let j = st
        .active
        .iter()
        .find(|j| j.repo == repo)
        .expect("no such job");
    (j.priority, j.promoted)
}

#[test]
fn queued_jobs_start_by_level_then_arrival() {
    use Priority::{High, Low, Normal, Urgent};
    let f = start(1);
    let h = holder(&f);
    let queued = vec![
        queue(&f, "l1", "repo-a", Some(Low)),
        queue(&f, "n1", "repo-a", None),
        queue(&f, "h1", "repo-a", Some(High)),
        queue(&f, "n2", "repo-a", Some(Normal)),
        queue(&f, "h2", "repo-a", Some(High)),
        queue(&f, "u1", "repo-a", Some(Urgent)),
    ];
    assert_eq!(run_all(&f, h, queued), ["u1", "h1", "h2", "n1", "n2", "l1"]);
}

#[test]
fn a_rule_set_at_run_time_wins_over_the_jobs_own_level() {
    let f = start(1);
    let h = holder(&f);
    let queued = vec![
        queue(&f, "api", "api", Some(Priority::High)),
        queue(&f, "web", "web", None),
    ];
    let set = |pattern: &str, level| {
        f.server
            .set_priority(&priority::Request {
                pattern: Some(pattern.into()),
                level,
                promote_after_s: None,
            })
            .unwrap()
    };
    set("ap*", Some(Priority::Low));
    assert_eq!(
        level_of(&f, "api"),
        (Priority::Low, false),
        "queued jobs re-ranked"
    );
    assert_eq!(level_of(&f, "web"), (Priority::Normal, false));
    assert_eq!(run_all(&f, h, queued), ["web", "api"]);
}

#[test]
fn priority_rules_survive_a_daemon_restart() {
    let f = start(1);
    for (pattern, level) in [
        ("repo-a", Some(Priority::High)),
        ("web-*", Some(Priority::Low)),
        ("api", Some(Priority::Low)),
        ("api", None),
    ] {
        f.server
            .set_priority(&priority::Request {
                pattern: Some(pattern.into()),
                level,
                promote_after_s: None,
            })
            .unwrap();
    }
    assert!(f.root.join("state").join(priority::FILE).exists());
    let again = Server::new(Config {
        socket: f.root.join("s2"),
        ..config(&f.root, 1)
    })
    .unwrap();
    let rules: Vec<(String, Priority)> = again
        .priority_view()
        .rules
        .into_iter()
        .map(|r| (r.pattern, r.level))
        .collect();
    assert_eq!(
        rules,
        [
            ("repo-a".to_string(), Priority::High),
            ("web-*".to_string(), Priority::Low)
        ]
    );
}

/// The starvation bound: with no slot free and `high` jobs arriving before
/// and after, a `low` job that has waited past `--promote-after` (30 min,
/// measured on a clock the test moves) starts before every `high` job that
/// came after it.
#[test]
fn a_low_job_starts_after_30_minutes_despite_a_stream_of_high_jobs() {
    static SKEW_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    fn clock() -> i64 {
        now_ms() + SKEW_MS.load(Ordering::SeqCst)
    }
    let f = start_cfg(1, |c| c.clock = clock);
    let h = holder(&f);
    let mut queued = vec![
        queue(&f, "low", "repo-a", Some(Priority::Low)),
        queue(&f, "h1", "web", Some(Priority::High)),
    ];
    SKEW_MS.store(29 * 60_000, Ordering::SeqCst);
    f.server.rerank();
    assert_eq!(level_of(&f, "repo-a"), (Priority::Low, false), "29 min");
    SKEW_MS.store(31 * 60_000, Ordering::SeqCst);
    f.server.rerank();
    assert_eq!(level_of(&f, "repo-a"), (Priority::High, true), "31 min");
    queued.push(queue(&f, "h2", "api", Some(Priority::High)));
    assert_eq!(run_all(&f, h, queued), ["low", "h1", "h2"]);
}

/// Sets the promotion delay as `slot priority promote-after` does.
fn promote_after(f: &Fixture, to: priority::PromoteAfter) -> priority::View {
    f.server
        .set_priority(&priority::Request {
            promote_after_s: Some(to),
            ..priority::Request::default()
        })
        .unwrap()
}

/// The promotion delay changes while the daemon runs: a `normal` job that
/// has waited 400 s (on a clock the test moves) is `normal` under
/// `--promote-after` 1800, `high` from the moment the delay is 300, and
/// starts before the `high` jobs that came after it. No restart between.
#[test]
fn a_delay_set_at_run_time_promotes_a_job_that_already_waits() {
    use priority::PromoteAfter::{Default, Seconds};
    static SKEW_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    fn clock() -> i64 {
        now_ms() + SKEW_MS.load(Ordering::SeqCst)
    }
    let f = start_cfg(1, |c| c.clock = clock);
    let h = holder(&f);
    let mut queued = vec![
        queue(&f, "waited", "repo-a", None),
        queue(&f, "h1", "web", Some(Priority::High)),
    ];
    SKEW_MS.store(400_000, Ordering::SeqCst);
    f.server.rerank();
    assert_eq!(f.server.priority_view().promote_after_s, 1800);
    assert_eq!(level_of(&f, "repo-a"), (Priority::Normal, false), "of 1800");

    assert_eq!(promote_after(&f, Seconds(300)).promote_after_s, 300);
    assert_eq!(level_of(&f, "repo-a"), (Priority::High, true), "of 300");

    assert_eq!(promote_after(&f, Seconds(0)).promote_after_s, 0);
    assert_eq!(level_of(&f, "repo-a"), (Priority::Normal, false), "never");

    assert_eq!(promote_after(&f, Default).promote_after_s, 1800);
    assert_eq!(level_of(&f, "repo-a"), (Priority::Normal, false), "option");

    promote_after(&f, Seconds(300));
    assert_eq!(level_of(&f, "repo-a"), (Priority::High, true));
    queued.push(queue(&f, "h2", "api", Some(Priority::High)));
    assert_eq!(run_all(&f, h, queued), ["waited", "h1", "h2"]);
}

/// The delay is saved beside the rules: a change of one keeps the other,
/// the saved delay wins over the option of the daemon that starts next, and
/// `default` takes it out of the file again.
#[test]
fn the_promotion_delay_survives_a_daemon_restart_beside_the_rules() {
    use priority::PromoteAfter::{Default, Seconds};
    let f = start(1);
    let rule = |pattern: &str, level| {
        f.server
            .set_priority(&priority::Request {
                pattern: Some(pattern.into()),
                level: Some(level),
                promote_after_s: None,
            })
            .unwrap()
    };
    let saved = || std::fs::read_to_string(f.root.join("state").join(priority::FILE)).unwrap();
    let restarted = |socket: &str| {
        Server::new(Config {
            socket: f.root.join(socket),
            promote_after: Duration::from_secs(900),
            ..config(&f.root, 1)
        })
        .unwrap()
        .priority_view()
    };
    rule("repo-a", Priority::High);
    promote_after(&f, Seconds(300));
    let v = rule("web-*", Priority::Low);
    assert_eq!((v.rules.len(), v.promote_after_s), (2, 300));
    assert!(saved().contains(r#""promote_after_s":300"#), "{}", saved());
    let v = restarted("s2");
    assert_eq!(
        (v.rules.len(), v.promote_after_s),
        (2, 300),
        "the saved delay wins over --promote-after 900"
    );

    assert_eq!(promote_after(&f, Default).promote_after_s, 1800);
    assert!(!saved().contains("promote_after_s"), "{}", saved());
    let v = restarted("s3");
    assert_eq!(
        (v.rules.len(), v.promote_after_s),
        (2, 900),
        "no saved delay: the option of that daemon, and the rules are kept"
    );
}

/// Status and text of the answer to `PUT /api/priority` with `body`, sent
/// as `content_type`, from the handler the router calls.
fn put_priority(f: &Fixture, content_type: &str, body: &str) -> (u16, String) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        content_type.parse().unwrap(),
    );
    let state = axum::extract::State(Arc::clone(&f.server));
    let body = axum::body::Bytes::from(body.to_owned());
    answer(http::priority_put(state, headers, body))
}

/// Status and text of the answer to `GET /api/priority`.
fn get_priority(f: &Fixture) -> (u16, String) {
    use axum::response::IntoResponse;
    let state = axum::extract::State(Arc::clone(&f.server));
    answer(async move { http::priority_get(state).await.into_response() })
}

fn answer(r: impl Future<Output = axum::response::Response>) -> (u16, String) {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async move {
            let r = r.await;
            let status = r.status().as_u16();
            let text = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
            (status, String::from_utf8(text.to_vec()).unwrap())
        })
}

#[test]
fn put_priority_takes_a_rule_a_delay_or_both() {
    let f = start(1);
    let json = "application/json";
    let view = |(status, text): (u16, String)| {
        assert_eq!(status, 200, "{text}");
        let v: priority::View = serde_json::from_str(&text).unwrap();
        let rules: Vec<String> = v
            .rules
            .iter()
            .map(|r| format!("{} {}", r.pattern, r.level))
            .collect();
        (rules.join(", "), v.promote_after_s)
    };
    let put = |body: &str| put_priority(&f, json, body);
    assert_eq!(view(get_priority(&f)), (String::new(), 1800));
    assert_eq!(
        view(put(r#"{"pattern":"repo-a","level":"high"}"#)),
        ("repo-a high".to_string(), 1800),
        "the body of a client without the delay"
    );
    assert_eq!(
        view(put(r#"{"promote_after_s":300}"#)),
        ("repo-a high".to_string(), 300),
        "no pattern: only the delay"
    );
    assert_eq!(
        view(get_priority(&f)),
        ("repo-a high".to_string(), 300),
        "GET reports the delay in force"
    );
    assert_eq!(
        view(put(r#"{"pattern":"repo-a","level":null}"#)),
        (String::new(), 300),
        "removing a rule keeps the delay"
    );
    assert_eq!(
        view(put(
            r#"{"pattern":"web-*","level":"low","promote_after_s":0}"#
        )),
        ("web-* low".to_string(), 0),
        "both in one body"
    );
    assert_eq!(
        view(put(r#"{"promote_after_s":null}"#)),
        ("web-* low".to_string(), 1800),
        "null: the daemon's option again"
    );
}

#[test]
fn put_priority_refuses_a_body_with_neither_and_an_invalid_one() {
    let f = start(1);
    let json = "application/json";
    put_priority(&f, json, r#"{"pattern":"repo-a","level":"high"}"#);
    put_priority(&f, json, r#"{"promote_after_s":300}"#);
    let before = f.server.priority_view();
    assert_eq!((before.rules.len(), before.promote_after_s), (1, 300));
    for bad in [
        "{}",
        r#"{"level":"high"}"#,
        r#"{"level":"low","promote_after_s":60}"#,
        r#"{"pattern":""}"#,
        r#"{"pattern":" ","promote_after_s":60}"#,
        r#"{"promote_after_s":-5}"#,
        r#"{"promote_after_s":"60"}"#,
        r#"{"promote_after_s":1.5}"#,
        r#"{"pattern":"repo-a","level":"asap"}"#,
        "not json",
        "",
    ] {
        let (status, text) = put_priority(&f, json, bad);
        assert_eq!(status, 400, "{bad}: {text}");
        assert!(text.starts_with("slot: "), "{bad}: {text}");
        assert_eq!(f.server.priority_view(), before, "{bad} changed nothing");
    }
    let (status, _) = put_priority(&f, "text/plain", r#"{"promote_after_s":60}"#);
    assert_eq!(status, 415, "a body a page can post without a preflight");
    assert_eq!(f.server.priority_view(), before);
}

#[test]
fn a_running_job_is_not_preempted() {
    let f = start(1);
    let h = holder(&f);
    let high = queue(&f, "high", "web", Some(Priority::High));
    f.server.rerank();
    {
        let g = f.server.inner.lock().unwrap();
        let running: Vec<(i64, usize)> = g
            .active
            .values()
            .filter(|l| l.job.state == JobState::Running)
            .map(|l| (l.job.id, l.held))
            .collect();
        assert_eq!(running, [(1, 1)], "the holder keeps its slot");
    }
    assert_eq!(f.server.pool().free(), 0);
    assert_eq!(run_all(&f, h, vec![high]), ["high"]);
}

/// With 0 slots free, an `urgent` job runs at once on a slot the headroom
/// holds back, while a `normal` job queued before it keeps waiting.
#[test]
fn an_urgent_job_starts_with_no_slot_free() {
    let f = start(2);
    let rt = tokio::runtime::Runtime::new().unwrap();
    assert_eq!(
        rt.block_on(Arc::clone(&f.server).set_headroom(50))
            .effective_tokens,
        1
    );
    wait_for("the held-back token", || f.server.pool().held_back() == 1);
    let h = holder(&f);
    assert_eq!(f.server.pool().free(), 0);
    let normal = queue(&f, "normal", "web", None);
    let mut r = sh(&f.root, "echo urgent >> order");
    r.repo = "api".into();
    r.priority = Some(Priority::Urgent);
    let urgent = send(&f.socket, r);
    urgent
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let u = collect(urgent);
    assert_eq!(
        (u.code, u.started),
        (0, true),
        "the urgent job waited: {u:?}"
    );
    assert_eq!(level_of(&f, "web"), (Priority::Normal, false));
    assert_eq!(f.server.pool().waiting(), 1, "the normal job still waits");
    wait_for("the token back in the held-back set", || {
        f.server.pool().held_back() == 1
    });
    assert_eq!(run_all(&f, h, vec![normal]), ["urgent", "normal"]);
}

/// `cargo` as a link to the shell: a command the rust toolchain matches
/// that needs no Rust installation.
fn fake_cargo(dir: &Path, script: &str, env: &[&str]) -> Request {
    let cargo = dir.join("cargo");
    if !cargo.exists() {
        std::os::unix::fs::symlink("/bin/sh", &cargo).unwrap();
    }
    Request {
        argv: vec![
            cargo.to_string_lossy().into_owned(),
            "-c".into(),
            script.into(),
        ],
        cwd: dir.to_string_lossy().into_owned(),
        env: ["PATH=/usr/bin:/bin"]
            .iter()
            .chain(env)
            .map(|s| (*s).to_string())
            .collect(),
        ..Request::default()
    }
}

fn last_logged(f: &Fixture) -> Job {
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    serde_json::from_str(log.lines().last().unwrap()).unwrap()
}

#[test]
fn a_rust_job_logs_its_cache_environment_and_is_warned_when_the_cache_is_off() {
    let f = start(2);
    let req = fake_cargo(
        &f.root,
        "echo ran",
        &[
            "RUSTC_WRAPPER=",
            "CARGO_INCREMENTAL=0",
            "CARGO_PROFILE_DEV_DEBUG=line-tables-only",
            "SCCACHE_SERVER_PORT=4303",
            "API_TOKEN=secret",
        ],
    );
    let c = send(&f.socket, req);
    let mut before_start = Vec::new();
    let mut started = false;
    let mut code = None;
    let mut out = String::new();
    for line in BufReader::new(c).lines() {
        match serde_json::from_str::<Frame>(&line.unwrap()).unwrap() {
            Frame::Info { msg } if !started => before_start.push(msg),
            Frame::Info { .. } | Frame::Err { .. } => {}
            Frame::Out { b } => {
                assert!(!before_start.is_empty(), "output before the warnings");
                out += &b;
            }
            Frame::Start { .. } => started = true,
            Frame::Exit { code: c } => {
                code = Some(c);
                break;
            }
        }
    }
    assert_eq!(
        (code, out.as_str()),
        (Some(0), "ran\n"),
        "the job runs unchanged"
    );
    assert_eq!(
        before_start,
        [
            "slot: warning: compile cache off: RUSTC_WRAPPER is empty",
            "slot: warning: private sccache server (port 4303)",
        ]
    );
    let job = last_logged(&f);
    assert_eq!(job.toolchain, "rust");
    assert_eq!(
        job.warnings,
        [
            "compile cache off: RUSTC_WRAPPER is empty",
            "private sccache server (port 4303)",
        ]
    );
    let want: BTreeMap<String, String> = [
        ("RUSTC_WRAPPER", ""),
        ("CARGO_INCREMENTAL", "0"),
        ("CARGO_PROFILE_DEV_DEBUG", "line-tables-only"),
        ("SCCACHE_SERVER_PORT", "4303"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    .collect();
    assert_eq!(job.cache_env, Some(want));
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    assert!(
        !log.contains("API_TOKEN") && !log.contains("\"PATH\""),
        "{log}"
    );
}

#[test]
fn a_rust_job_with_the_usual_setup_and_a_job_of_another_toolchain_get_no_warning() {
    let f = start(2);
    let r = run(&f, fake_cargo(&f.root, "true", &["RUSTC_WRAPPER=sccache"]));
    assert_eq!((r.code, r.info.as_str()), (0, ""));
    let job = last_logged(&f);
    assert_eq!(
        job.cache_env,
        Some(BTreeMap::from([(
            "RUSTC_WRAPPER".to_string(),
            "sccache".to_string()
        )]))
    );
    assert_eq!(job.warnings, Vec::<String>::new());
    let r = run(&f, fake_cargo(&f.root, "true", &[]));
    assert_eq!((r.code, r.info.as_str()), (0, ""));
    assert_eq!(
        last_logged(&f).cache_env,
        Some(BTreeMap::new()),
        "a rust job with none of the variables set"
    );
    let mut other = sh(&f.root, "true");
    other.env = vec!["PATH=/usr/bin:/bin".into(), "RUSTC_WRAPPER=".into()];
    let r = run(&f, other);
    assert_eq!((r.code, r.info.as_str()), (0, ""));
    let job = last_logged(&f);
    assert_eq!((job.toolchain.as_str(), job.cache_env), ("other", None));
    assert_eq!(job.warnings, Vec::<String>::new());
    let line = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    let line = line.lines().last().unwrap();
    assert!(
        !line.contains("cache_env") && !line.contains("warnings"),
        "{line}"
    );
}

/// A cargo command that compiles, as far as its name says (`cargo-test`, a
/// link to the shell, runs `script`), with `target` as its target directory.
fn fake_build(dir: &Path, target: &Path, script: &str) -> Request {
    let cargo = dir.join("cargo-test");
    if !cargo.exists() {
        std::os::unix::fs::symlink("/bin/sh", &cargo).unwrap();
    }
    Request {
        argv: vec![
            cargo.to_string_lossy().into_owned(),
            "-c".into(),
            script.into(),
        ],
        cwd: dir.to_string_lossy().into_owned(),
        env: vec![
            "PATH=/usr/bin:/bin".into(),
            format!("CARGO_TARGET_DIR={}", target.display()),
        ],
        ..Request::default()
    }
}

/// A build into `target` that runs until the file `go` exists.
fn building(f: &Fixture, target: &Path, go: &str) -> (UnixStream, i64) {
    let script = format!("while [ ! -e {go} ]; do sleep 0.02; done");
    let (c, id) = submit(f, fake_build(&f.root, target, &script));
    wait_for("the build to run", || state_of(f, id) == JobState::Running);
    (c, id)
}

/// Holds cargo's lock on the profile directory `debug` of `target`, as a
/// cargo that compiles there does, until the file is dropped.
fn compile_lock(target: &Path) -> std::fs::File {
    let dir = target.join("debug");
    std::fs::create_dir_all(&dir).unwrap();
    let f = std::fs::File::create(dir.join(".cargo-build-lock")).unwrap();
    f.lock().unwrap();
    f
}

/// Sends `req` and returns its connection and the id of its job.
fn submit(f: &Fixture, req: Request) -> (UnixStream, i64) {
    let next = || f.server.inner.lock().unwrap().next_id;
    let before = next();
    let c = send(&f.socket, req);
    wait_for("the job to be listed", || next() > before);
    (c, before + 1)
}

fn active_job(f: &Fixture, id: i64) -> Option<Job> {
    f.server.state().active.into_iter().find(|j| j.id == id)
}

fn state_of(f: &Fixture, id: i64) -> JobState {
    active_job(f, id).map_or(JobState::Queued, |j| j.state)
}

fn reason_of(f: &Fixture, id: i64) -> String {
    active_job(f, id).map_or_else(String::new, |j| j.wait_reason)
}

fn done_job(f: &Fixture, id: i64) -> Job {
    let st = f.server.state();
    st.done.into_iter().find(|j| j.id == id).expect("not done")
}

fn in_use_by(id: i64) -> String {
    format!("target directory in use by job {id}")
}

/// Polls `cond` for up to 5 s like [`wait_for`]; a timeout names every
/// active job with its state and reason, and the pool's free slots and
/// queue.
fn wait_for_jobs(f: &Fixture, what: &str, cond: impl Fn() -> bool) {
    for _ in 0..500 {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let jobs: Vec<String> = {
        let g = f.server.inner.lock().unwrap();
        g.active
            .values()
            .map(|l| {
                format!(
                    "{} {:?} {:?} pid={:?} held={} queued={} gate={}",
                    l.job.id,
                    l.job.state,
                    l.job.wait_reason,
                    l.pid,
                    l.held,
                    l.waiter.is_some(),
                    l.target_wait
                )
            })
            .collect()
    };
    panic!(
        "timed out waiting for {what}: {jobs:#?}, {} slot(s) free, {} in the queue",
        f.server.pool().free(),
        f.server.pool().waiting()
    );
}

/// The lines of the file `order`, which jobs append their names to.
fn order(f: &Fixture) -> Vec<String> {
    std::fs::read_to_string(f.root.join("order"))
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

/// While a job compiles in a target directory, a second cargo build of the
/// same directory stays queued outside the queue for slots, holds none and
/// names the job it waits for. It starts when the lock is given back, while
/// the first job runs on (cargo runs tests without the lock).
#[test]
fn a_cargo_build_waits_while_another_job_compiles_in_its_target_directory() {
    let f = start(2);
    let target = f.root.join("target");
    let lock = compile_lock(&target);
    let (first, a) = building(&f, &target, "go");
    let (second, b) = submit(&f, fake_build(&f.root, &target, "echo built"));
    let info = read_info_until(&second, "waiting:");
    assert_eq!(info, format!("slot: job {b} waiting: {}", in_use_by(a)));
    // Past the settle time of the first job only the lock holds it back.
    std::thread::sleep(Duration::from_millis(400));
    let queued = active_job(&f, b).unwrap();
    assert_eq!(
        (queued.state, queued.wait_reason, queued.started_ms),
        (JobState::Queued, in_use_by(a), 0)
    );
    assert_eq!(f.server.pool().free(), 1, "the waiting job holds no slot");
    assert_eq!(f.server.pool().waiting(), 0, "nor a place in the queue");
    {
        let table = f.server.inner.lock().unwrap();
        let live = &table.active[&b];
        assert!(live.held == 0 && live.waiter.is_none() && live.target_wait);
    }
    let shown = crate::status::render(&f.server.state());
    assert!(
        shown.contains(&format!("waiting: {}", in_use_by(a))),
        "{shown}"
    );
    drop(lock);
    let built = collect(second);
    assert_eq!(
        (built.code, built.out.as_str(), built.started),
        (0, "built\n", true)
    );
    assert_eq!(state_of(&f, a), JobState::Running, "the first job runs on");
    assert_eq!(done_job(&f, b).wait_reason, "");
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(first).code, 0);
}

/// A build that waits for its target directory is not in the queue for
/// slots: the jobs behind it start, and one that asked for the whole pool
/// gathers no slot meanwhile. It starts when the other job has ended.
#[test]
fn jobs_behind_a_build_that_waits_for_its_target_directory_start() {
    let f = start(2);
    let target = f.root.join("target");
    let _lock = compile_lock(&target);
    let (first, a) = building(&f, &target, "go");
    let mut whole_pool = fake_build(&f.root, &target, "echo built");
    whole_pool.tokens = Some(2);
    let (second, b) = submit(&f, whole_pool);
    wait_for("the build to wait for its directory", || {
        reason_of(&f, b) == in_use_by(a)
    });
    for _ in 0..3 {
        let r = run(&f, sh(&f.root, "echo behind"));
        assert_eq!((r.code, r.out.as_str()), (0, "behind\n"));
        assert_eq!(r.info, "", "it found a free slot and nobody ahead");
    }
    assert_eq!(
        (state_of(&f, b), reason_of(&f, b)),
        (JobState::Queued, in_use_by(a))
    );
    assert_eq!(f.server.pool().free(), 1, "no slot gathered");
    assert_eq!(f.server.pool().waiting(), 0);
    // The first job ends. No job of the pool compiles in the directory any
    // more, so the build starts as it did before, whoever holds the lock.
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(first).code, 0);
    let r = collect(second);
    assert_eq!((r.code, r.out.as_str()), (0, "built\n"));
    assert_eq!(done_job(&f, b).tokens, 2);
}

/// Several builds that wait for one target directory start by level, then
/// age, one at a time: each next one waits for the one before it.
#[test]
fn builds_that_wait_for_one_target_directory_start_by_level_then_age() {
    let f = start(4);
    let target = f.root.join("target");
    let _lock = compile_lock(&target);
    let (first, a) = building(&f, &target, "go");
    let wait = |name: &str, level: Option<Priority>| {
        let script = format!("echo {name}-starts >> order; sleep 0.2; echo {name}-ends >> order");
        let mut req = fake_build(&f.root, &target, &script);
        req.priority = level;
        let (c, id) = submit(&f, req);
        wait_for("the build to wait for its directory", || {
            reason_of(&f, id) == in_use_by(a)
        });
        c
    };
    let waiting = [
        wait("n1", None),
        wait("n2", None),
        wait("low", Some(Priority::Low)),
        wait("high", Some(Priority::High)),
    ];
    assert_eq!(f.server.pool().free(), 3);
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(first).code, 0);
    for c in waiting {
        assert_eq!(collect(c).code, 0);
    }
    let want: Vec<String> = ["high", "n1", "n2", "low"]
        .iter()
        .flat_map(|n| [format!("{n}-starts"), format!("{n}-ends")])
        .collect();
    assert_eq!(order(&f), want, "three slots were free the whole time");
}

/// A build that waited for its target directory comes to the queue for
/// slots at the place its age gives it: ahead of a job that queued while it
/// waited.
#[test]
fn a_build_that_waited_for_its_target_directory_keeps_its_place_in_the_queue() {
    let f = start(2);
    let target = f.root.join("target");
    let lock = compile_lock(&target);
    let (first, a) = building(&f, &target, "go");
    let other = send(
        &f.socket,
        sh(&f.root, "while [ ! -e go-other ]; do sleep 0.02; done"),
    );
    wait_for("both slots to be taken", || f.server.pool().free() == 0);
    let (second, b) = submit(&f, fake_build(&f.root, &target, "echo build >> order"));
    wait_for("the build to wait for its directory", || {
        reason_of(&f, b) == in_use_by(a)
    });
    let later = queue(&f, "later", "repo-a", None);
    drop(lock);
    wait_for("the build to queue for a slot", || {
        f.server.pool().waiting() == 2
    });
    assert_eq!(reason_of(&f, b), "");
    std::fs::write(f.root.join("go-other"), "").unwrap();
    assert_eq!(collect(other).code, 0);
    assert_eq!(collect(second).code, 0);
    assert_eq!(collect(later).code, 0);
    assert_eq!(order(&f), ["build", "later"]);
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(first).code, 0);
}

/// A build in the queue for slots makes way when a build of its target
/// directory queues that is served first: it leaves the queue, the other
/// starts first, and it waits for that one to compile.
#[test]
fn a_queued_build_makes_way_for_one_of_its_target_directory_that_is_served_first() {
    let f = start(2);
    let target = f.root.join("target");
    let _lock = compile_lock(&target);
    let mut all = sh(&f.root, "while [ ! -e go ]; do sleep 0.02; done");
    all.tokens = Some(2);
    let holder = send(&f.socket, all);
    wait_for("both slots to be taken", || f.server.pool().free() == 0);
    let (normal, n) = submit(&f, fake_build(&f.root, &target, "echo normal >> order"));
    wait_for("the first build to queue for a slot", || {
        f.server.pool().waiting() == 1
    });
    assert_eq!(reason_of(&f, n), "");
    let mut req = fake_build(
        &f.root,
        &target,
        "echo high >> order; while [ ! -e go-high ]; do sleep 0.02; done",
    );
    req.priority = Some(Priority::High);
    let (high, h) = submit(&f, req);
    let goes_first = format!("target directory goes to job {h} first");
    wait_for("the first build to make way", || {
        reason_of(&f, n) == goes_first
    });
    wait_for("only the second build to stand in the queue", || {
        f.server.pool().waiting() == 1
    });
    assert_eq!(reason_of(&f, h), "");
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(holder).code, 0);
    wait_for_jobs(&f, "the first build to wait for the second", || {
        reason_of(&f, n) == in_use_by(h)
    });
    wait_for("the second build to run", || {
        state_of(&f, h) == JobState::Running
    });
    assert_eq!(reason_of(&f, n), in_use_by(h));
    assert_eq!(
        f.server.pool().free(),
        1,
        "the build that waits holds no slot"
    );
    std::fs::write(f.root.join("go-high"), "").unwrap();
    assert_eq!(collect(high).code, 0);
    let r = collect(normal);
    assert_eq!(r.code, 0);
    assert!(
        r.info
            .contains(&format!("slot: job {n} waiting: {goes_first}")),
        "{}",
        r.info
    );
    assert_eq!(order(&f), ["high", "normal"]);
    let (high, normal) = (done_job(&f, h), done_job(&f, n));
    assert!(
        normal.started_ms >= high.ended_ms,
        "the lock was held throughout, so one ran after the other: {normal:?} {high:?}"
    );
}

/// Only cargo builds of the same target directory wait: a script, a build
/// into another directory (also one whose lock a process outside the pool
/// holds), a cargo command that does not compile and a build on a synced
/// workspace start beside the job that compiles.
#[test]
fn the_target_directory_holds_back_only_builds_into_the_same_directory() {
    let f = start(2);
    let target = f.root.join("target");
    let _lock = compile_lock(&target);
    let (first, a) = building(&f, &target, "go");
    let elsewhere = f.root.join("elsewhere/target");
    let _outside = compile_lock(&elsewhere);
    let mut no_build = fake_cargo(&f.root, "echo ran", &[]);
    no_build
        .env
        .push(format!("CARGO_TARGET_DIR={}", target.display()));
    std::fs::create_dir_all(f.root.join("ws/w1")).unwrap();
    let synced = Request {
        cwd: String::new(),
        env: Vec::new(),
        workspace: Some(Workspace {
            id: "w1".into(),
            rel_cwd: ".".into(),
            client_root: "/c".into(),
        }),
        ..fake_build(&f.root, &target, "echo ran")
    };
    for (what, req) in [
        ("a script", sh(&f.root, "echo ran")),
        (
            "a build into another target directory",
            fake_build(&f.root, &elsewhere, "echo ran"),
        ),
        ("a cargo command that does not compile", no_build),
        ("a build on a synced workspace", synced),
    ] {
        let r = run(&f, req);
        assert_eq!((r.code, r.out.as_str()), (0, "ran\n"), "{what}: {r:?}");
        assert!(!r.info.contains("target directory"), "{what}: {}", r.info);
    }
    assert_eq!(state_of(&f, a), JobState::Running);
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    let dirs: Vec<String> = log
        .lines()
        .map(|l| serde_json::from_str::<Job>(l).unwrap().target_dir)
        .collect();
    let (here, there) = (
        target.display().to_string(),
        elsewhere.display().to_string(),
    );
    assert_eq!(dirs, ["", there.as_str(), here.as_str(), ""]);
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(first).code, 0);
}

/// Sets the run-time limit and the most a job may name, as `slot
/// run-limit` does; 0 turns the limit off.
fn set_run_limit(f: &Fixture, limit_s: Option<u64>, max_s: Option<u64>) -> run_limit::View {
    f.server
        .set_run_limit(&run_limit::Request {
            run_limit_s: limit_s,
            run_limit_max_s: max_s,
        })
        .unwrap()
}

/// Sends `req` and collects its frames, giving up after 20 s: a job that
/// nothing ends fails its test instead of holding it.
fn run_bounded(f: &Fixture, req: Request) -> Res {
    let c = send(&f.socket, req);
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    collect(c)
}

/// A `sleep 30` under a limit of 1 s: the daemon ends it, the client exits
/// 124 and is told why, the record says `timed_out`, the process is gone
/// and the slot is back. A job inside the limit is not marked.
#[test]
fn a_job_past_the_run_time_limit_is_ended_with_124_and_logged_as_timed_out() {
    let f = start(1);
    assert_eq!(set_run_limit(&f, Some(1), None).run_limit_s, Some(1));
    let marker = f.root.join("pid");
    let t0 = Instant::now();
    let r = run_bounded(
        &f,
        sh(
            &f.root,
            &format!("echo $$ > {}; sleep 30", marker.display()),
        ),
    );
    let ran = t0.elapsed();
    assert_eq!((r.code, r.started), (124, true), "{r:?}");
    assert!(
        ran >= Duration::from_secs(1) && ran < Duration::from_secs(15),
        "ended after {ran:?}"
    );
    assert!(r.info.contains("slot: job 1 ended after "), "{}", r.info);
    assert!(
        r.info.contains("over its run-time limit of 1s"),
        "{}",
        r.info
    );
    assert!(
        r.info.contains("`slot --timeout <DURATION>`, at most 1h"),
        "{}",
        r.info
    );
    let pid: i32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    let job = last_logged(&f);
    assert_eq!(
        (job.state, job.exit, job.timed_out, job.limit_s),
        (JobState::Failed, 124, true, Some(1))
    );
    assert!(job.ended_ms - job.started_ms >= 1000, "{job:?}");
    let shown = done_job(&f, 1);
    assert_eq!(
        (shown.state, shown.exit, shown.timed_out, shown.limit_s),
        (JobState::Failed, 124, true, Some(1)),
        "the web view gets the same record"
    );
    wait_for("the slot to come back", || f.server.pool().free() == 1);

    let r = run_bounded(&f, sh(&f.root, "echo quick"));
    assert_eq!((r.code, r.out.as_str()), (0, "quick\n"), "{r:?}");
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    let line = log.lines().last().unwrap();
    assert!(line.contains(r#""limit_s":1"#), "{line}");
    assert!(!line.contains("timed_out"), "{line}");
}

/// Under a limit of 1 s a job that names 60 s with `--timeout` runs for
/// 2.5 s and ends by itself; a job that names less than the limit is ended
/// at its own time.
#[test]
fn a_job_that_names_a_longer_time_is_not_ended_at_the_daemons_limit() {
    let f = start(1);
    set_run_limit(&f, Some(1), None);
    let mut own = sh(&f.root, "sleep 2.5; echo own");
    own.timeout_s = Some(60);
    let r = run_bounded(&f, own);
    assert_eq!((r.code, r.out.as_str()), (0, "own\n"), "{r:?}");
    assert!(!r.info.contains("run-time limit"), "{}", r.info);
    let job = last_logged(&f);
    assert_eq!((job.timed_out, job.limit_s), (false, Some(60)), "{job:?}");
    assert!(job.ended_ms - job.started_ms >= 2000, "{job:?}");
    set_run_limit(&f, Some(30), None);
    let mut short = sh(&f.root, "sleep 30");
    short.timeout_s = Some(1);
    let r = run_bounded(&f, short);
    assert_eq!(r.code, 124, "{r:?}");
    assert!(
        r.info.contains("over its run-time limit of 1s"),
        "{}",
        r.info
    );
}

/// A `--timeout` over the most a job may name is capped, and the client is
/// told once; with no daemon limit a job has none unless it names one.
#[test]
fn a_timeout_is_capped_at_the_most_a_job_may_name() {
    let f = start(1);
    assert_eq!(
        f.server.run_limit_view(),
        run_limit::View {
            run_limit_s: None,
            run_limit_max_s: 3600
        },
        "a fresh daemon has no limit"
    );
    let r = run_bounded(&f, sh(&f.root, "true"));
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(last_logged(&f).limit_s, None);

    let mut long = sh(&f.root, "true");
    long.timeout_s = Some(7200);
    let r = run_bounded(&f, long.clone());
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.info.matches("capped").count(), 1, "told once: {}", r.info);
    assert!(
        r.info.contains("slot: --timeout 2h capped at 1h"),
        "{}",
        r.info
    );
    assert_eq!(last_logged(&f).limit_s, Some(3600));

    set_run_limit(&f, None, Some(120));
    let r = run_bounded(&f, long);
    assert!(
        r.info.contains("slot: --timeout 2h capped at 2m"),
        "{}",
        r.info
    );
    assert_eq!(last_logged(&f).limit_s, Some(120));
}

/// `req` with `timeout <time>` in front of its command.
fn wrapped(req: Request, time: &str) -> Request {
    Request {
        argv: [vec!["timeout".into(), time.into()], req.argv].concat(),
        ..req
    }
}

/// A `timeout` in front of the command extends nothing: under a limit of
/// 1 s a `timeout 1800 sleep 30` is ended after 1 s, and its client is told
/// once, when the job is submitted, how to name the time. No line with no
/// limit, nor for a `timeout` inside the job's limit.
#[test]
fn a_wrapped_timeout_does_not_extend_a_job_and_its_client_is_told_once() {
    let f = start(1);
    let told = |r: &Res| r.info.matches("a wrapped `timeout`").count();
    let r = run_bounded(&f, wrapped(sh(&f.root, "true"), "1800"));
    assert_eq!((r.code, told(&r)), (0, 0), "no limit is set: {r:?}");
    assert_eq!(last_logged(&f).limit_s, None);

    set_run_limit(&f, Some(1), None);
    let t0 = Instant::now();
    let r = run_bounded(&f, wrapped(sh(&f.root, "sleep 30"), "1800"));
    let ran = t0.elapsed();
    assert_eq!((r.code, told(&r)), (124, 1), "{r:?}");
    assert!(ran < Duration::from_secs(15), "ended after {ran:?}");
    assert!(
        r.info.contains(
            "slot: this job ends after 1s; a wrapped `timeout` does not extend it, name the \
             time with `slot --timeout 30m` (at most 1h)"
        ),
        "{}",
        r.info
    );
    let before_start = r.info.find("a wrapped `timeout`").unwrap();
    let ended = r.info.find("ended after").unwrap();
    assert!(before_start < ended, "told at submit: {}", r.info);
    let job = last_logged(&f);
    assert_eq!(
        (job.exit, job.timed_out, job.limit_s),
        (124, true, Some(1)),
        "the daemon's limit, not the 1800 s of the `timeout`"
    );

    set_run_limit(&f, Some(30), Some(120));
    let r = run_bounded(&f, wrapped(sh(&f.root, "true"), "20"));
    assert_eq!((r.code, told(&r)), (0, 0), "inside the limit: {r:?}");
    let r = run_bounded(&f, wrapped(sh(&f.root, "true"), "1h"));
    assert_eq!((r.code, told(&r)), (0, 1), "{r:?}");
    assert!(
        r.info.contains("`slot --timeout 2m` (at most 2m)"),
        "the most it may name: {}",
        r.info
    );
    assert!(!r.info.contains("capped"), "it named nothing: {}", r.info);
    assert_eq!(last_logged(&f).limit_s, Some(30));
    let mut own = wrapped(sh(&f.root, "true"), "1h");
    own.timeout_s = Some(60);
    let r = run_bounded(&f, own);
    assert_eq!((r.code, told(&r)), (0, 1), "{r:?}");
    assert!(
        r.info.contains("slot: this job ends after 1m; "),
        "{}",
        r.info
    );
    assert_eq!(last_logged(&f).limit_s, Some(60));
}

/// Status and text of the answer to `PUT /api/run-limit` with `body`.
fn put_run_limit(f: &Fixture, content_type: &str, body: &str) -> (u16, String) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        content_type.parse().unwrap(),
    );
    let state = axum::extract::State(Arc::clone(&f.server));
    let body = axum::body::Bytes::from(body.to_owned());
    answer(http::run_limit_put(state, headers, body))
}

/// Status and text of the answer to `GET /api/run-limit`.
fn get_run_limit(f: &Fixture) -> (u16, String) {
    use axum::response::IntoResponse;
    let state = axum::extract::State(Arc::clone(&f.server));
    answer(async move { http::run_limit_get(state).await.into_response() })
}

/// The limit set through `PUT /api/run-limit`, the call `slot run-limit`
/// makes, is saved beside the headroom, read back by `GET`, shown in the
/// state `slot status` reads and in force in the daemon that starts next.
#[test]
fn the_run_time_limit_is_set_saved_and_read_back() {
    let f = start(2);
    let json = "application/json";
    let view = |(status, text): (u16, String)| {
        assert_eq!(status, 200, "{text}");
        let v: run_limit::View = serde_json::from_str(&text).unwrap();
        (v.run_limit_s, v.run_limit_max_s)
    };
    let saved = || std::fs::read_to_string(f.root.join("state").join(limits::FILE)).unwrap();
    let restarted = |socket: &str| {
        Server::new(Config {
            socket: f.root.join(socket),
            ..config(&f.root, 2)
        })
        .unwrap()
    };
    assert_eq!(view(get_run_limit(&f)), (None, 3600));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(Arc::clone(&f.server).set_headroom(50));
    assert!(!saved().contains("run_limit"), "{}", saved());

    assert_eq!(
        view(put_run_limit(&f, json, r#"{"run_limit_s":600}"#)),
        (Some(600), 3600)
    );
    assert_eq!(view(get_run_limit(&f)), (Some(600), 3600));
    assert!(saved().contains(r#""run_limit_s":600"#), "{}", saved());
    assert!(saved().contains(r#""headroom_percent":50"#), "{}", saved());
    let st = f.server.state();
    assert_eq!(
        (st.run_limit_s, st.run_limit_max_s),
        (Some(600), Some(3600))
    );
    assert_eq!(
        view(put_run_limit(&f, json, r#"{"run_limit_max_s":7200}"#)),
        (Some(600), 7200),
        "the maximum alone keeps the limit"
    );
    rt.block_on(Arc::clone(&f.server).set_headroom(25));
    assert!(
        saved().contains(r#""run_limit_s":600"#),
        "a headroom change keeps the limit: {}",
        saved()
    );
    let next = restarted("s2");
    assert_eq!(
        next.run_limit_view(),
        run_limit::View {
            run_limit_s: Some(600),
            run_limit_max_s: 7200
        }
    );
    assert_eq!(next.limits().headroom_percent, 25);

    let before = f.server.run_limit_view();
    for bad in [
        "{}",
        r#"{"run_limit_s":7201}"#,
        r#"{"run_limit_max_s":0}"#,
        r#"{"run_limit_max_s":300}"#,
        r#"{"run_limit_s":-5}"#,
        r#"{"run_limit_s":"10m"}"#,
        r#"{"run_limit_s":1.5}"#,
        "not json",
        "",
    ] {
        let (status, text) = put_run_limit(&f, json, bad);
        assert_eq!(status, 400, "{bad}: {text}");
        assert!(text.starts_with("slot: "), "{bad}: {text}");
        assert_eq!(f.server.run_limit_view(), before, "{bad} changed nothing");
    }
    let (status, _) = put_run_limit(&f, "text/plain", r#"{"run_limit_s":60}"#);
    assert_eq!(status, 415, "a body a page can post without a preflight");
    assert_eq!(f.server.run_limit_view(), before);

    assert_eq!(
        view(put_run_limit(&f, json, r#"{"run_limit_s":0}"#)),
        (None, 7200),
        "0 turns the limit off"
    );
    assert!(!saved().contains("run_limit_s"), "{}", saved());
    assert_eq!(restarted("s3").run_limit_view().run_limit_s, None);
}

/// A limit set while a job runs does not reach back: the job keeps the
/// limit it started with, here none.
#[test]
fn a_limit_set_while_a_job_runs_applies_to_the_jobs_that_start_after_it() {
    let f = start(2);
    let h = holder(&f);
    set_run_limit(&f, Some(1), None);
    std::thread::sleep(Duration::from_millis(2200));
    let st = f.server.state();
    assert_eq!(st.active.len(), 1, "the job from before runs on");
    assert_eq!(
        (st.active[0].limit_s, st.active[0].timed_out),
        (None, false)
    );
    std::fs::write(f.root.join("go"), "").unwrap();
    h.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    assert_eq!(collect(h).code, 0);
}

/// The interval of the token watch in the tests of it: the daemon's is
/// 15 s, so "within 45 s" is three of these.
const WATCH: Duration = Duration::from_millis(200);

/// A process outside the pool's jobs that has the pool's FIFO open as
/// descriptor 3, as a jobserver client has. It runs in its own process
/// group and is killed with its children when dropped.
struct Client(std::process::Child);

impl Client {
    /// Starts `sh -c script` with the FIFO's path in `$FIFO` and waits
    /// until the script made the file `<name>.ready`.
    fn start(f: &Fixture, name: &str, script: &str) -> Self {
        let child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("FIFO", f.server.pool().path())
            .current_dir(&f.root)
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let ready = f.root.join(format!("{name}.ready"));
        wait_for(&format!("the client {name}"), || ready.exists());
        Self(child)
    }

    /// A live client with `children` children that holds `tokens` tokens:
    /// it starts the children, which do not have the FIFO open, and then
    /// takes the tokens, as a build driver takes a token for each child
    /// after its first.
    fn holding(f: &Fixture, name: &str, tokens: usize, children: usize) -> Self {
        Self::start(
            f,
            name,
            &format!(
                "exec 3<>\"$FIFO\"; i=0; \
                 while [ $i -lt {children} ]; do sleep 600 3>&- & i=$((i+1)); done; \
                 dd bs=1 count={tokens} <&3 >/dev/null 2>&1; : > {name}.ready; wait"
            ),
        )
    }

    /// A client that took `tokens` tokens and has no child.
    fn took(f: &Fixture, name: &str, tokens: usize) -> Self {
        Self::start(
            f,
            name,
            &format!(
                "exec 3<>\"$FIFO\"; dd bs=1 count={tokens} <&3 >/dev/null 2>&1; \
                 : > {name}.ready; exec sleep 600"
            ),
        )
    }

    /// SIGKILL to the client and its children; the tokens it took are gone.
    fn kill(&mut self) {
        let _ = killpg(Pid::from_raw(self.0.id() as i32), Signal::SIGKILL);
        let _ = self.0.wait();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A job that holds `tokens` slots until its connection closes, once it runs.
fn holding_job(f: &Fixture, tokens: u32) -> (UnixStream, i64) {
    let mut req = sh(&f.root, "while [ ! -e go ]; do sleep 0.02; done");
    req.tokens = Some(tokens);
    let (c, id) = submit(f, req);
    wait_for_jobs(f, "the job to run", || state_of(f, id) == JobState::Running);
    (c, id)
}

/// A job that waits in the queue for one slot.
fn waiting_job(f: &Fixture) -> (UnixStream, i64) {
    let before = f.server.pool().waiting();
    let (c, id) = submit(f, sh(&f.root, "while [ ! -e go ]; do sleep 0.02; done"));
    wait_for_jobs(f, "the job to wait for a slot", || {
        f.server.pool().waiting() == before + 1
    });
    (c, id)
}

fn restored(f: &Fixture) -> u64 {
    f.server.restored.load(Ordering::Relaxed)
}

fn passes(f: &Fixture) -> u64 {
    f.server.watch_passes.load(Ordering::Relaxed)
}

/// Polls `cond` for up to 45 s: a pass of the token watch reads `/proc`,
/// which takes over a second on a loaded host. A timeout names what the
/// watch did.
fn wait_for_watch(f: &Fixture, what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "timed out waiting for {what}: {} pass(es), {} restored, {} removed, {} slot(s) free, \
         {} in the queue",
        passes(f),
        restored(f),
        f.server.removed.load(Ordering::Relaxed),
        f.server.pool().free(),
        f.server.pool().waiting()
    );
}

/// Waits until the token watch ran `n` whole passes after this moment: one
/// may be under way, so it waits for one more. 4 are a minute of the
/// daemon's.
fn after_passes(f: &Fixture, n: u64) {
    let until = passes(f) + n + 1;
    wait_for_watch(f, "passes of the token watch", || passes(f) >= until);
}

/// The lines of the census log with the event `event`.
fn census_events(f: &Fixture, event: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(f.root.join("state/census.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["event"] == event)
        .collect()
}

#[test]
fn lost_tokens_come_back_while_another_process_holds_the_pool_open() {
    let f = start_cfg(6, |c| c.token_watch = WATCH);
    let _live = Client::holding(&f, "live", 1, 2);
    let (_a, a) = holding_job(&f, 2);
    // Until it is killed it counts for its 3 tokens, like the live client.
    let mut lost = Client::holding(&f, "lost", 3, 4);
    assert_eq!(f.server.pool().free(), 0);
    let (_b, b) = waiting_job(&f);
    lost.kill();
    wait_for_watch(&f, "the lost slots to come back", || restored(&f) > 0);
    assert_eq!(restored(&f), 3, "3 were lost; the live client holds 1");
    wait_for_jobs(&f, "the waiting job to run", || {
        state_of(&f, b) == JobState::Running
    });
    // Two more passes would write back what the watch still took for lost.
    after_passes(&f, 2);
    assert_eq!(restored(&f), 3, "the live client's slot was written back");
    assert_eq!(state_of(&f, a), JobState::Running);
    assert_eq!(
        f.server.pool().free(),
        2,
        "6 slots: 2 and 1 with the jobs, 1 with the live client"
    );
    let events = census_events(&f, "tokens_restored");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["tokens"], 3, "{events:?}");
}

#[test]
fn slots_that_live_processes_hold_are_not_written_back() {
    let f = start_cfg(6, |c| c.token_watch = WATCH);
    let _one = Client::holding(&f, "one", 1, 2);
    let _two = Client::holding(&f, "two", 2, 3);
    let (_a, _) = holding_job(&f, 3);
    assert_eq!(f.server.pool().free(), 0);
    let (_b, b) = waiting_job(&f);
    after_passes(&f, 4);
    assert_eq!(restored(&f), 0);
    assert_eq!(f.server.pool().free(), 0);
    assert_eq!(state_of(&f, b), JobState::Queued);
    assert_eq!(
        census_events(&f, "tokens_restored"),
        Vec::<serde_json::Value>::new()
    );
    assert_eq!(f.server.state().tokens_unaccounted, 0);
}

#[test]
fn a_surplus_token_is_taken_out_while_jobs_run() {
    let f = start_cfg(6, |c| c.token_watch = WATCH);
    let (_a, a) = holding_job(&f, 1);
    assert_eq!(f.server.pool().free(), 5);
    // A slot released twice: 7 tokens for a pool of 6.
    f.server.pool().release(1);
    wait_for_watch(&f, "the surplus to be taken out", || {
        f.server.pool().free() == 5
    });
    assert_eq!(state_of(&f, a), JobState::Running);
    after_passes(&f, 4);
    assert_eq!(f.server.pool().free(), 5, "more than the surplus taken out");
    assert_eq!(f.server.removed.load(Ordering::Relaxed), 1);
    assert_eq!(restored(&f), 0);
    let events = census_events(&f, "tokens_removed");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["tokens"], 1, "{events:?}");
}

#[test]
fn the_state_counts_slots_no_job_and_no_live_process_holds_and_since_when() {
    // The watch of this daemon runs once at its start; the test runs the
    // passes itself.
    let f = start_cfg(6, |c| c.token_watch = Duration::from_secs(3600));
    wait_for_watch(&f, "the first pass", || passes(&f) >= 1);
    let (_a, _) = holding_job(&f, 3);
    let mut lost = Client::took(&f, "lost", 3);
    lost.kill();
    let before = now_ms();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut memo = TokenMemo::default();
    rt.block_on(f.server.token_pass(&mut memo));
    let st = f.server.state();
    assert_eq!(st.tokens_unaccounted, 3);
    assert!(
        (before..=now_ms()).contains(&st.tokens_unaccounted_since_ms),
        "{st:?}"
    );
    assert_eq!(restored(&f), 0, "one count is not acted on");
    assert!(
        f.server
            .metrics_text()
            .contains("slot_tokens_unaccounted 3\n"),
        "{}",
        f.server.metrics_text()
    );
    let shown = crate::status::render(&State {
        now_ms: st.tokens_unaccounted_since_ms + 20_000,
        ..st.clone()
    });
    assert!(
        shown.contains("lost slots: 3 that no job and no live process holds, oldest for 20s\n"),
        "{shown}"
    );
    rt.block_on(f.server.token_pass(&mut memo));
    assert_eq!(restored(&f), 3);
    let st = f.server.state();
    assert_eq!(
        (st.tokens_unaccounted, st.tokens_unaccounted_since_ms),
        (0, 0)
    );
    assert!(!crate::status::render(&st).contains("lost slots"));
    assert!(
        f.server
            .metrics_text()
            .contains("slot_tokens_unaccounted 0\n"),
        "{}",
        f.server.metrics_text()
    );
}

/// Declares, inside one test, the CPUs its daemon reads as busy in the pool,
/// in tenths of a CPU (negative: no figure), with nothing in the compile
/// cache or outside, and the function that reads it. [`Config::cpu_busy`]
/// is a plain function and the tests of this file run side by side in one
/// process, so each test has a figure of its own.
macro_rules! host_busy {
    ($cell:ident, $read:ident, $tenths:expr) => {
        static $cell: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new($tenths);
        fn $read() -> Option<cpu_limit::Parts> {
            let v = $cell.load(std::sync::atomic::Ordering::SeqCst);
            (v >= 0).then(|| cpu_limit::Parts {
                pool: v as f64 / 10.0,
                ..cpu_limit::Parts::default()
            })
        }
    };
}

/// A daemon on a host of 20 CPUs whose CPU limit reads `busy` and whose
/// held jobs look again every 20 ms.
fn start_busy(
    tokens: usize,
    busy: fn() -> Option<cpu_limit::Parts>,
    change: impl FnOnce(&mut Config),
) -> Fixture {
    start_cfg(tokens, |c| {
        c.cpu_busy = Some(busy);
        c.host_cpus = Some(20);
        c.resource_poll = Duration::from_millis(20);
        change(c);
    })
}

fn set_cpu_limit(f: &Fixture, mark: Option<f64>, floor: Option<usize>) -> cpu_limit::View {
    f.server
        .set_cpu_limit(&cpu_limit::Request {
            cpu_busy_max: Some(mark),
            cpu_busy_floor: floor,
        })
        .unwrap()
}

/// Submits a job that uses no CPU and runs until its client goes away.
fn quiet(f: &Fixture) -> (UnixStream, i64) {
    submit(f, sh(&f.root, "sleep 60"))
}

/// The ids of the running jobs, oldest first.
fn running(f: &Fixture) -> Vec<i64> {
    let mut ids: Vec<i64> = f
        .server
        .state()
        .active
        .iter()
        .filter(|j| j.state == JobState::Running)
        .map(|j| j.id)
        .collect();
    ids.sort_unstable();
    ids
}

/// Closes the clients and waits until the daemon has ended their jobs.
fn end_all(f: &Fixture, clients: Vec<UnixStream>) {
    drop(clients);
    wait_for_jobs(f, "every job to end with its client", || {
        f.server.state().active.is_empty()
    });
}

/// With `slot cpu-limit 17 --floor 1` on a host that is 18.4 CPUs busy the
/// first job starts, because the floor always runs, and the next one is
/// held without a slot: its wait reason says why, its client is told, the
/// census log has the event and the view counts it. When the figure falls
/// under the mark the job starts.
#[test]
fn a_start_is_held_while_the_host_is_busier_than_the_mark_and_let_go_when_it_falls() {
    use std::sync::atomic::Ordering::SeqCst;
    host_busy!(BUSY, busy, 184);
    let f = start_busy(4, busy, |_| {});
    let view = set_cpu_limit(&f, Some(17.0), Some(1));
    assert_eq!(
        (
            view.cpu_busy_max,
            view.cpu_busy_floor,
            view.cpus,
            view.busy,
            view.held
        ),
        (Some(17.0), Some(1), 20, Some(18.4), 0)
    );
    let (first, a) = quiet(&f);
    wait_for_jobs(&f, "the first job to run: it is the floor", || {
        state_of(&f, a) == JobState::Running
    });
    let (second, b) = submit(&f, sh(&f.root, "echo second"));
    let held = "host busy: pool 18.4 + compile cache 0.0 of 20 CPUs + 1.0 for jobs that just started, mark 17.0";
    wait_for_jobs(&f, "the second job to be held", || reason_of(&f, b) == held);
    assert_eq!(state_of(&f, b), JobState::Queued);
    let told = read_info_until(&second, "waiting");
    assert!(
        told.contains(&format!("slot: job {b} waiting: {held}")),
        "{told}"
    );
    let events = census_events(&f, "cpu_hold");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        (events[0]["job"].as_i64(), events[0]["reason"].as_str()),
        (Some(b), Some(held))
    );
    assert_eq!(f.server.cpu_limit_view().held, 1);
    let st = f.server.state();
    assert_eq!((st.cpu_busy_max, st.cpu_busy_floor), (Some(17.0), Some(1)));
    assert_eq!(f.server.pool().free(), 3, "a held job holds no slot");

    BUSY.store(100, SeqCst);
    let res = collect(second);
    assert_eq!((res.code, res.out.as_str()), (0, "second\n"), "{res:?}");
    assert_eq!(f.server.cpu_limit_view().held, 0);
    assert_eq!(census_events(&f, "cpu_hold_bound").len(), 0);
    end_all(&f, vec![first]);
}

/// Under the floor a job starts on a saturated host, and under the mark
/// jobs start however many run.
#[test]
fn nothing_is_held_under_the_floor_or_under_the_mark() {
    use std::sync::atomic::Ordering::SeqCst;
    host_busy!(BUSY, busy, 200);
    let f = start_busy(8, busy, |_| {});
    set_cpu_limit(&f, Some(17.0), Some(2));
    let (c1, j1) = quiet(&f);
    let (c2, j2) = quiet(&f);
    wait_for_jobs(&f, "two jobs to run on a saturated host: the floor", || {
        running(&f) == [j1, j2]
    });
    let (c3, j3) = quiet(&f);
    wait_for_jobs(&f, "the third to be held: the floor runs", || {
        reason_of(&f, j3).starts_with("host busy: pool 20.0 + compile cache 0.0 of 20 CPUs")
    });
    assert_eq!(running(&f), [j1, j2]);

    // 14.0 busy and 1.0 for each of the two that just started: 16.0.
    BUSY.store(140, SeqCst);
    wait_for_jobs(&f, "the third to run under the mark", || {
        running(&f) == [j1, j2, j3]
    });
    BUSY.store(20, SeqCst);
    let (c4, j4) = quiet(&f);
    let (c5, j5) = quiet(&f);
    wait_for_jobs(&f, "five jobs to run on an idle host", || {
        running(&f) == [j1, j2, j3, j4, j5]
    });
    assert_eq!(
        census_events(&f, "cpu_hold").len(),
        1,
        "only the third was ever held"
    );
    end_all(&f, vec![c1, c2, c3, c4, c5]);
}

/// Ten jobs are held on a busy host. When the figure dips under the mark
/// they do not all start: each job that starts counts with the CPUs it is
/// expected to use until the figure shows it, so only as many start as fit
/// under the mark, the oldest first, and the others stay held.
#[test]
fn a_burst_of_held_jobs_starts_only_as_many_as_fit_under_the_mark_oldest_first() {
    use std::sync::atomic::Ordering::SeqCst;
    host_busy!(BUSY, busy, 184);
    let f = start_busy(16, busy, |_| {});
    set_cpu_limit(&f, Some(17.0), Some(1));
    let (first, a) = quiet(&f);
    wait_for_jobs(&f, "the first job to run", || running(&f) == [a]);
    let mut clients = vec![first];
    let mut ids = Vec::new();
    for _ in 0..10 {
        let (c, id) = quiet(&f);
        clients.push(c);
        ids.push(id);
    }
    wait_for_jobs(&f, "ten jobs to be held", || {
        ids.iter().all(|id| {
            reason_of(&f, *id).starts_with("host busy: pool 18.4 + compile cache 0.0 of 20 CPUs")
        })
    });
    assert_eq!(running(&f), [a]);
    assert_eq!(f.server.cpu_limit_view().held, 10);

    // A dip to 14.5. The job that runs counts 1.0, so two more fit under
    // 17 (15.5, then 16.5); a third would make 17.5. Every job that stays
    // held has looked at the new figure, with the three counted.
    BUSY.store(145, SeqCst);
    let held_at =
        |ids: &[i64], text: &str| ids.iter().all(|id| reason_of(&f, *id).starts_with(text));
    wait_for_jobs(&f, "the two oldest to start and eight to stay held", || {
        running(&f) == [a, ids[0], ids[1]]
            && held_at(
                &ids[2..],
                "host busy: pool 14.5 + compile cache 0.0 of 20 CPUs + 3.0 for jobs that just started",
            )
    });
    assert_eq!(f.server.cpu_limit_view().held, 8);
    assert_eq!(f.server.pool().free(), 13, "held jobs hold no slots");

    // Two CPUs less: two more.
    BUSY.store(125, SeqCst);
    wait_for_jobs(&f, "the next two to start and six to stay held", || {
        running(&f) == [a, ids[0], ids[1], ids[2], ids[3]]
            && held_at(
                &ids[4..],
                "host busy: pool 12.5 + compile cache 0.0 of 20 CPUs + 5.0 for jobs that just started",
            )
    });
    assert_eq!(f.server.cpu_limit_view().held, 6);
    end_all(&f, clients);
}

/// A job is not held for the host's CPUs for ever: after the bound (10
/// minutes outside tests, 300 ms here) it starts although the figure never
/// fell, its client is told and the census log has the event.
#[test]
fn a_job_held_for_the_bound_starts_on_a_host_that_stays_busy() {
    use std::sync::atomic::Ordering::SeqCst;
    host_busy!(BUSY, busy, 184);
    let f = start_busy(4, busy, |c| c.cpu_hold_max = Duration::from_millis(300));
    set_cpu_limit(&f, Some(17.0), Some(1));
    let (first, a) = quiet(&f);
    wait_for_jobs(&f, "the first job to run", || running(&f) == [a]);
    let t0 = Instant::now();
    let (second, b) = submit(&f, sh(&f.root, "echo second"));
    let r = collect(second);
    assert_eq!((r.code, r.out.as_str()), (0, "second\n"), "{r:?}");
    assert!(
        t0.elapsed() >= Duration::from_millis(300),
        "started after {:?}",
        t0.elapsed()
    );
    assert!(
        r.info.contains(&format!(
            "slot: job {b} waiting: host busy: pool 18.4 + compile cache 0.0 of 20 CPUs"
        )),
        "{}",
        r.info
    );
    assert!(
        r.info.contains(&format!("slot: job {b} was held"))
            && r.info
                .contains("for the host's CPUs, as long as a job is: it starts"),
        "{}",
        r.info
    );
    let bound = census_events(&f, "cpu_hold_bound");
    assert_eq!(bound.len(), 1, "{bound:?}");
    assert_eq!(bound[0]["job"].as_i64(), Some(b));
    assert!(bound[0]["held_ms"].as_i64() >= Some(300), "{bound:?}");
    assert_eq!(BUSY.load(SeqCst), 184, "the figure never fell");
    end_all(&f, vec![first]);
}

/// Without a mark a saturated host holds nothing and `limits.json` is what
/// a daemon without the setting writes. A mark set while jobs are queued
/// holds the next one; switched off, the held job starts and the file is
/// byte for byte what it was.
#[test]
fn without_a_mark_a_saturated_host_holds_nothing_and_the_file_is_unchanged() {
    host_busy!(BUSY, busy, 200);
    let f = start_busy(8, busy, |_| {});
    let saved = || std::fs::read_to_string(f.root.join("state").join(limits::FILE)).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(Arc::clone(&f.server).set_headroom(25));
    let before = saved();
    assert_eq!(before, r#"{"headroom_percent":25,"base":"pool"}"#);
    let (c1, j1) = quiet(&f);
    let (c2, j2) = quiet(&f);
    let (c3, j3) = quiet(&f);
    wait_for_jobs(&f, "three jobs to run on a saturated host", || {
        running(&f) == [j1, j2, j3]
    });
    assert_eq!(census_events(&f, "cpu_hold").len(), 0);
    assert_eq!(f.server.state().cpu_busy_max, None);

    let view = set_cpu_limit(&f, Some(17.0), None);
    assert_eq!(
        (view.cpu_busy_max, view.cpu_busy_floor),
        (Some(17.0), Some(4))
    );
    assert_eq!(
        saved(),
        r#"{"headroom_percent":25,"base":"pool","cpu_busy_max":17.0}"#
    );
    let (c4, j4) = quiet(&f);
    wait_for_jobs(&f, "the fourth to run: the floor of 4", || {
        running(&f) == [j1, j2, j3, j4]
    });
    let (c5, j5) = quiet(&f);
    wait_for_jobs(&f, "the fifth to be held", || {
        reason_of(&f, j5).starts_with("host busy: pool 20.0 + compile cache 0.0 of 20 CPUs")
    });
    let view = set_cpu_limit(&f, None, None);
    assert_eq!((view.cpu_busy_max, view.cpu_busy_floor), (None, None));
    wait_for_jobs(&f, "the held job to start once the limit is off", || {
        running(&f) == [j1, j2, j3, j4, j5]
    });
    assert_eq!(saved(), before, "off: the file as before the setting");
    assert_eq!(BUSY.load(std::sync::atomic::Ordering::SeqCst), 200);
    end_all(&f, vec![c1, c2, c3, c4, c5]);
}

/// Status and text of the answer to `PUT /api/cpu-limit` with `body`.
fn put_cpu_limit(f: &Fixture, content_type: &str, body: &str) -> (u16, String) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        content_type.parse().unwrap(),
    );
    let state = axum::extract::State(Arc::clone(&f.server));
    let body = axum::body::Bytes::from(body.to_owned());
    answer(http::cpu_limit_put(state, headers, body))
}

/// Status and text of the answer to `GET /api/cpu-limit`.
fn get_cpu_limit(f: &Fixture) -> (u16, String) {
    use axum::response::IntoResponse;
    let state = axum::extract::State(Arc::clone(&f.server));
    answer(async move { http::cpu_limit_get(state).await.into_response() })
}

/// The mark and the floor set through `PUT /api/cpu-limit`, the call `slot
/// cpu-limit` makes, are saved beside the headroom, read back by `GET`,
/// shown in the state `slot status` reads and in force in the daemon that
/// starts next. A mark under 1, a floor of 0 and a floor without a mark are
/// refused; a mark above the host's CPUs is taken; `null` switches it off.
#[test]
fn the_cpu_limit_is_set_saved_and_read_back() {
    let f = start_cfg(2, |c| c.host_cpus = Some(20));
    let json = "application/json";
    let view = |(status, text): (u16, String)| {
        assert_eq!(status, 200, "{text}");
        let v: cpu_limit::View = serde_json::from_str(&text).unwrap();
        (v.cpu_busy_max, v.cpu_busy_floor, v.cpus)
    };
    let in_force = || {
        let v = f.server.cpu_limit_view();
        (v.cpu_busy_max, v.cpu_busy_floor)
    };
    let saved = || std::fs::read_to_string(f.root.join("state").join(limits::FILE)).unwrap();
    let restarted = |socket: &str| {
        let next = Server::new(Config {
            socket: f.root.join(socket),
            host_cpus: Some(20),
            ..config(&f.root, 2)
        })
        .unwrap();
        let v = next.cpu_limit_view();
        (
            v.cpu_busy_max,
            v.cpu_busy_floor,
            next.limits().headroom_percent,
        )
    };
    assert_eq!(view(get_cpu_limit(&f)), (None, None, 20));
    let st = f.server.state();
    assert_eq!((st.cpu_busy_max, st.cpu_busy_floor), (None, None));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(Arc::clone(&f.server).set_headroom(50));
    assert!(!saved().contains("cpu_busy"), "{}", saved());

    assert_eq!(
        view(put_cpu_limit(&f, json, r#"{"cpu_busy_max":17}"#)),
        (Some(17.0), Some(4), 20),
        "the floor nobody named is 4"
    );
    assert_eq!(view(get_cpu_limit(&f)), (Some(17.0), Some(4), 20));
    assert!(saved().ends_with(r#","cpu_busy_max":17.0}"#), "{}", saved());
    assert!(saved().contains(r#""headroom_percent":50"#), "{}", saved());
    let st = f.server.state();
    assert_eq!((st.cpu_busy_max, st.cpu_busy_floor), (Some(17.0), Some(4)));
    assert_eq!(
        view(put_cpu_limit(&f, json, r#"{"cpu_busy_floor":6}"#)),
        (Some(17.0), Some(6), 20),
        "the floor alone keeps the mark"
    );
    assert!(
        saved().ends_with(r#","cpu_busy_max":17.0,"cpu_busy_floor":6}"#),
        "{}",
        saved()
    );
    rt.block_on(Arc::clone(&f.server).set_headroom(25));
    assert!(
        saved().contains(r#""cpu_busy_max":17.0,"cpu_busy_floor":6"#),
        "a headroom change keeps the limit: {}",
        saved()
    );
    assert_eq!(restarted("s2"), (Some(17.0), Some(6), 25));

    for bad in [
        "{}",
        r#"{"cpu_busy_max":0}"#,
        r#"{"cpu_busy_max":0.5}"#,
        r#"{"cpu_busy_max":-2}"#,
        r#"{"cpu_busy_max":"17"}"#,
        r#"{"cpu_busy_max":17,"cpu_busy_floor":0}"#,
        r#"{"cpu_busy_floor":-1}"#,
        r#"{"cpu_busy_max":null,"cpu_busy_floor":2}"#,
        "not json",
        "",
    ] {
        let (status, text) = put_cpu_limit(&f, json, bad);
        assert_eq!(status, 400, "{bad}: {text}");
        assert!(text.starts_with("slot: "), "{bad}: {text}");
        assert_eq!(in_force(), (Some(17.0), Some(6)), "{bad} changed nothing");
    }
    let (status, _) = put_cpu_limit(&f, "text/plain", r#"{"cpu_busy_max":2}"#);
    assert_eq!(status, 415, "a body a page can post without a preflight");
    assert_eq!(in_force(), (Some(17.0), Some(6)));

    assert_eq!(
        view(put_cpu_limit(&f, json, r#"{"cpu_busy_max":25}"#)),
        (Some(25.0), Some(6), 20),
        "a mark above the host's 20 CPUs is taken: it never holds"
    );
    assert_eq!(
        view(put_cpu_limit(&f, json, r#"{"cpu_busy_max":null}"#)),
        (None, None, 20),
        "null switches it off and forgets the floor"
    );
    assert!(!saved().contains("cpu_busy"), "{}", saved());
    assert_eq!(f.server.state().cpu_busy_max, None);
    assert_eq!(restarted("s3"), (None, None, 25));
}

// wave w1 u4

/// A Cargo target directory under `root` with a tag and a little content,
/// that a job of repository `repo` built into long ago.
fn old_target(f: &Fixture, repo: &str) -> PathBuf {
    let t = f.root.join("w").join(repo).join("target");
    std::fs::create_dir_all(&t).unwrap();
    std::fs::write(
        t.join("CACHEDIR.TAG"),
        format!("{}\n", crate::evict::CACHEDIR_SIGNATURE),
    )
    .unwrap();
    std::fs::write(t.join("blob"), vec![0u8; 4096]).unwrap();
    f.server
        .inner
        .lock()
        .unwrap()
        .targets
        .insert(t.clone(), (repo.to_string(), 1));
    t
}

/// With `--min-free-disk 50` and `--evict-below 40`, a filesystem at 45 GB
/// free holds a job for disk; eviction runs at the larger mark, 50 GB, as
/// soon as the job is held, and the job starts once the pass freed enough.
#[test]
fn eviction_starts_at_the_disk_holds_mark_and_the_held_job_starts() {
    use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
    const GB: u64 = crate::admission::GB;
    static FREE: AtomicU64 = AtomicU64::new(60 * GB);
    static TARGET: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    /// `FREE`, and 20 GB more once the target directory is gone.
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        let gone = TARGET.get().is_some_and(|t| !t.exists());
        Some(FREE.load(SeqCst) + if gone { 20 * GB } else { 0 })
    }
    let f = start_cfg(2, |c| {
        c.disk_free = Some(free);
        c.admission.min_free_disk = 50 * GB;
        c.evict = crate::evict::Settings {
            floor: 40 * GB,
            margin: 10 * GB,
            min_idle: Duration::ZERO,
            dry_run: false,
        };
        c.resource_poll = Duration::from_millis(20);
    });
    let target = old_target(&f, "old");
    TARGET.set(target.clone()).unwrap();
    wait_for_jobs(&f, "the watch's first look", || {
        f.server.state().evict.checked_ms > 0
    });
    assert!(target.exists(), "60 GB free: nothing to delete");

    FREE.store(45 * GB, SeqCst);
    let (c, id) = submit(&f, sh(&f.root, "echo ran"));
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let r = collect(c);
    assert!(
        r.info.contains(&format!(
            "slot: job {id} waiting: 45.0 GB free on the filesystem of"
        )),
        "held for disk first: {r:?}"
    );
    assert!(r.info.contains("builds need 50.0 GB"), "{r:?}");
    assert_eq!((r.code, r.out.as_str()), (0, "ran\n"), "{r:?}");
    assert!(!target.exists(), "evicted at 45 GB, under the mark of 50");
    let e = f.server.state().evict;
    assert_eq!(e.floor, 50 * GB, "the mark is the larger value");
    assert_eq!(e.raised_from, 40 * GB, "and it says it was raised");
    assert_eq!(e.last.len(), 1, "{e:?}");
    assert_eq!(e.last[0].action, "deleted", "{e:?}");
}

/// `--evict-below 0` keeps eviction off, whatever `--min-free-disk` is: a
/// job held for disk deletes nothing.
#[test]
fn evict_below_0_stays_off_whatever_the_disk_hold_is() {
    const GB: u64 = crate::admission::GB;
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        Some(45 * GB)
    }
    let f = start_cfg(2, |c| {
        c.disk_free = Some(free);
        c.admission.min_free_disk = 50 * GB;
        c.evict = crate::evict::Settings {
            floor: 0,
            margin: 10 * GB,
            min_idle: Duration::ZERO,
            dry_run: false,
        };
        c.resource_poll = Duration::from_millis(20);
    });
    let target = old_target(&f, "old");
    let (c, id) = submit(&f, sh(&f.root, "echo ran"));
    wait_for_jobs(&f, "the job to be held for disk", || {
        reason_of(&f, id).contains("builds need 50.0 GB")
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(target.exists(), "eviction is off");
    let e = f.server.state().evict;
    assert_eq!((e.floor, e.checked_ms), (0, 0), "{e:?}");
    drop(c);
}

/// Declares, inside one test, the three parts of the CPUs its daemon reads
/// as busy, each in tenths of a CPU, and the function that reads them.
macro_rules! cpu_parts {
    ($pool:ident, $cache:ident, $outside:ident, $read:ident, $p:expr, $c:expr, $o:expr) => {
        static $pool: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new($p);
        static $cache: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new($c);
        static $outside: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new($o);
        fn $read() -> Option<cpu_limit::Parts> {
            let tenths = |c: &std::sync::atomic::AtomicI64| {
                c.load(std::sync::atomic::Ordering::SeqCst) as f64 / 10.0
            };
            Some(cpu_limit::Parts {
                pool: tenths(&$pool),
                compile_cache: tenths(&$cache),
                outside: tenths(&$outside),
            })
        }
    };
}

/// With the mark 17 and the floor 6 on a host of 20 CPUs, 10 CPUs busy
/// outside the pool and the compile cache and 5 inside them do not hold a
/// queued job; 12 in the pool and 6 in the compile cache do. The wait
/// reason names the two parts, and `GET /api/cpu-limit` answers the parts
/// and the CPUs outside both.
#[test]
fn the_cpu_limit_counts_the_pool_and_the_compile_cache_not_work_outside_them() {
    use std::sync::atomic::Ordering::SeqCst;
    cpu_parts!(POOL, CACHE, OUTSIDE, busy, 30, 20, 100);
    let f = start_busy(16, busy, |_| {});
    set_cpu_limit(&f, Some(17.0), Some(6));
    let mut clients = Vec::new();
    for _ in 0..6 {
        clients.push(quiet(&f).0);
    }
    wait_for_jobs(&f, "the floor of 6 to run", || running(&f).len() == 6);

    // 3 + 2 inside, 6.0 for the six that just started: 11 of 17. The whole
    // host, 15 + 6, would be over the mark.
    let (c, seventh) = quiet(&f);
    clients.push(c);
    wait_for_jobs(
        &f,
        "the seventh to start: work outside is not counted",
        || state_of(&f, seventh) == JobState::Running,
    );
    assert_eq!(census_events(&f, "cpu_hold").len(), 0);

    POOL.store(120, SeqCst);
    CACHE.store(60, SeqCst);
    OUTSIDE.store(15, SeqCst);
    let (c, eighth) = quiet(&f);
    clients.push(c);
    wait_for_jobs(&f, "the eighth to be held for the pool's 18 CPUs", || {
        reason_of(&f, eighth).starts_with("host busy: pool 12.0 + compile cache 6.0 of 20 CPUs")
    });
    let reason = reason_of(&f, eighth);
    assert!(reason.ends_with(", mark 17.0"), "{reason}");
    assert_eq!(state_of(&f, eighth), JobState::Queued);

    let v = f.server.cpu_limit_view();
    assert_eq!(
        (v.pool, v.compile_cache, v.outside, v.busy, v.held),
        (Some(12.0), Some(6.0), Some(1.5), Some(19.5), 1)
    );
    let (status, text) = get_cpu_limit(&f);
    assert_eq!(status, 200, "{text}");
    assert!(
        text.contains(r#""pool":12.0,"compile_cache":6.0,"outside":1.5"#),
        "{text}"
    );
    end_all(&f, clients);
}

/// A stand-in for `slot __exec` in `dir`: the lib tests run in the test
/// harness, not in the slot binary. It takes the same arguments
/// (`__exec --nofile S:H --arg0 A -- program args`), sets the limit and
/// execs the program; each call appends its arguments to `dir/calls`.
/// `slot-cli/tests/handover.rs` runs the real one.
fn exec_shim(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let calls = dir.join("calls");
    let script = dir.join("exec-shim");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             echo \"$*\" >> '{}'\n\
             soft=${{3%%:*}}; hard=${{3##*:}}\n\
             ulimit -S -n \"$soft\" 2>/dev/null\n\
             ulimit -H -n \"$hard\" && ulimit -S -n \"$soft\" || exit 126\n\
             shift 6\n\
             exec \"$@\"\n",
            calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// What `ulimit -Sn; ulimit -Hn` prints in a job of a client whose
/// open-file limit is `nofile`.
fn job_nofile(f: &Fixture, nofile: Option<crate::proto::FileLimit>) -> Res {
    let mut req = sh(&f.root, "ulimit -Sn; ulimit -Hn");
    req.nofile = nofile;
    run(f, req)
}

fn limit_line(v: u64) -> String {
    if v == u64::MAX {
        "unlimited".into()
    } else {
        v.to_string()
    }
}

/// A job runs with the open-file limit its client sends, not with the
/// daemon's, from its first instruction; above the daemon's hard limit it
/// gets that, and its client is warned once. A request without a limit, or
/// with the daemon's own, runs with the daemon's and without the shim. The
/// job record keeps the job's own argv.
#[test]
fn a_job_runs_with_the_open_file_limit_of_its_client() {
    use crate::proto::FileLimit;
    let shim_dir = tempfile::tempdir().unwrap();
    let shim = exec_shim(shim_dir.path());
    let f = start_cfg(2, |c| c.exec_shim = Some(shim));
    let calls = || std::fs::read_to_string(shim_dir.path().join("calls")).unwrap_or_default();
    let daemon = FileLimit::of_this_process();
    for soft in [
        daemon.soft / 2,
        daemon.soft.saturating_mul(2).min(daemon.hard),
    ] {
        if soft == daemon.soft {
            continue;
        }
        let r = job_nofile(
            &f,
            Some(FileLimit {
                soft,
                hard: daemon.hard,
            }),
        );
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(
            r.out,
            format!("{}\n{}\n", limit_line(soft), limit_line(daemon.hard)),
            "{r:?}"
        );
        assert!(!r.info.contains("open files"), "{r:?}");
    }
    assert!(
        calls().contains(&format!(
            "__exec --nofile {}:{} --arg0 sh -- ",
            limit_line(daemon.soft / 2),
            limit_line(daemon.hard)
        )),
        "{}",
        calls()
    );
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    for line in log.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            v["argv"],
            serde_json::json!(["sh", "-c", "ulimit -Sn; ulimit -Hn"]),
            "{line}"
        );
    }

    let before = calls();
    for nofile in [None, Some(daemon)] {
        let r = job_nofile(&f, nofile);
        assert_eq!(
            r.out,
            format!("{}\n{}\n", limit_line(daemon.soft), limit_line(daemon.hard)),
            "the daemon's limit: {r:?}"
        );
    }
    assert_eq!(calls(), before, "no shim when nothing changes");

    if daemon.hard < u64::MAX {
        let r = job_nofile(
            &f,
            Some(FileLimit {
                soft: daemon.hard + 1,
                hard: u64::MAX,
            }),
        );
        let hard = limit_line(daemon.hard);
        assert_eq!(r.out, format!("{hard}\n{hard}\n"), "{r:?}");
        assert_eq!(
            r.info.matches("slot: warning: open files:").count(),
            1,
            "{r:?}"
        );
        assert!(
            r.info.contains(&format!(
                "the caller's limit is {}, the daemon's hard limit is {hard}: the job gets {hard}",
                daemon.hard + 1
            )),
            "{r:?}"
        );
    }
}
