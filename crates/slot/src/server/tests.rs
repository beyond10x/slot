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
        // The poll interval of a bounded wait.
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
        pass_for: Duration::from_secs(300),
        clock: now_ms,
        target_poll: Duration::from_millis(40),
        target_settle: Duration::from_millis(250),
        token_watch: TOKEN_WATCH,
        resource_poll: RESOURCE_POLL,
        cpu_busy: None,
        host_cpus: None,
        cpu_hold_max: cpu_limit::HOLD_MAX,
        cpu_bound_gap: Duration::from_millis(cpu_limit::STARTING_MS as u64),
        cpu_pressure: None,
        cache_budget: 0,
        seed_copy: cache::SeedCopy::Reflink,
        evict: crate::evict::Settings {
            floor: 0,
            margin: 0,
            min_idle: Duration::from_secs(7200),
            dry_run: true,
        },
        disk_free: None,
        exec_shim: None,
        handover: true,
        sandbox: None,
        cluster_file: None,
        same_path_roots: Vec::new(),
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
            Frame::Info { msg } | Frame::Fallback { msg } => r.info += &msg,
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

/// `GET /api/nodes` answers a bare array with the local machine as the one
/// node: its CPUs, the slots in circulation, no deadline; every job of
/// `/api/state` names that node.
#[test]
fn nodes_answers_the_local_machine_and_jobs_name_it() {
    let f = start(2);
    assert_eq!(run(&f, sh(&f.root, "true")).code, 0);
    assert_eq!(run(&f, sh(&f.root, "exit 3")).code, 3);
    let st = f.server.state();
    assert_eq!(st.done.len(), 2);
    assert!(
        st.done.iter().all(|j| j.node == LOCAL_NODE),
        "{:?}",
        st.done
    );
    let server = Arc::clone(&f.server);
    let (status, text) = answer(async move {
        use axum::response::IntoResponse;
        http::nodes(axum::extract::State(server))
            .await
            .into_response()
    });
    assert_eq!(status, 200, "{text}");
    assert!(text.starts_with('['), "a bare array: {text}");
    let v: Vec<Node> = serde_json::from_str(&text).unwrap();
    assert_eq!(v.len(), 1, "{text}");
    let n = &v[0];
    assert_eq!(
        (n.node_id.as_str(), n.kind, n.state),
        ("local", NodeKind::Local, NodeState::Ready)
    );
    assert_eq!(
        (n.slots, n.slots_in_use, n.running_jobs, n.queued_jobs),
        (2, 0, 0, 0)
    );
    assert!(n.cpus >= 1);
    assert_eq!(n.deadline, None);
    assert!(
        n.joined_at.as_deref().is_some_and(|t| t.ends_with('Z')),
        "{text}"
    );
    assert_eq!(n.slot_version, env!("SLOT_VERSION"));
    assert!(!text.contains("\"deadline\""), "{text}");
}

/// The local node counts the slots of its running jobs and its running and
/// queued jobs, gives sizes in whole GiB and the CPU in thousandths; a
/// job-log line without a node is shown on the local node.
#[test]
fn the_local_node_counts_jobs_slots_and_sizes() {
    let job = |state: JobState, tokens: u32| Job {
        tokens,
        state,
        ..serde_json::from_str::<Job>(
            r#"{"id":1,"argv":["true"],"cwd":"/w","repo":"r","toolchain":"other",
            "remote":false,"tokens":1,"state":"queued","enqueued_ms":1,"started_ms":0,
            "ended_ms":0,"exit":0,"cpu_s":0.0,"compile_server_cpu_s":0.0,"peak_rss":0}"#,
        )
        .unwrap()
    };
    let a = job(JobState::Running, 4);
    let b = job(JobState::Queued, 2);
    let c = job(JobState::Running, 1);
    let jobs = [&a, &b, &c];
    let n = local_node(&LocalFacts {
        cluster: "host",
        cpus: 20,
        slots: 16,
        memory: 32 << 30,
        available: 20 << 30,
        disk_total: 1800 << 30,
        disk_free: 604 << 30,
        joined_ms: 1_763_899_200_000,
        cpu_busy: 3.4567,
        jobs: &jobs,
        host: HostFacts {
            booted_ms: None,
            poweroff_ms: None,
            busy: 0.0,
            load1: None,
        },
        ended: &Tally::default(),
        now: 1_763_899_200_000,
    });
    assert_eq!((n.slots_in_use, n.running_jobs, n.queued_jobs), (5, 2, 1));
    assert_eq!((n.memory_gib, n.disk_gib), (32, 1800));
    assert_eq!(n.memory_used_mib, Some(12 * 1024));
    assert_eq!(n.disk_free_gib, Some(604));
    assert_eq!(n.cpu_busy_milli, Some(3457));
    assert_eq!(n.joined_at.as_deref(), Some("2025-11-23T12:00:00Z"));
    assert_eq!(n.cluster_id, "host");
    assert_eq!(on_local_node(a.clone()).node, LOCAL_NODE);
}

/// The local node reports the machine's boot, its scheduled poweroff, the
/// CPUs busy on the whole host and its load, beside the pool's own CPUs.
#[test]
fn the_local_node_reports_boot_poweroff_and_the_whole_host() {
    let tally = Tally::default();
    let n = local_node(&LocalFacts {
        cluster: "host",
        cpus: 20,
        slots: 16,
        memory: 32 << 30,
        available: 20 << 30,
        disk_total: 1800 << 30,
        disk_free: 604 << 30,
        joined_ms: 1_763_899_200_000,
        cpu_busy: 0.0,
        jobs: &[],
        host: HostFacts {
            booted_ms: Some(1_763_895_600_000),
            poweroff_ms: Some(1_763_942_400_000),
            busy: 4.1234,
            load1: Some(5.31),
        },
        ended: &tally,
        now: 1_763_899_200_000,
    });
    assert_eq!(n.cpu_busy_milli, Some(0));
    assert_eq!(n.host_cpu_milli, Some(4123));
    assert_eq!(n.load1, Some(5.31));
    assert_eq!(n.booted_at.as_deref(), Some("2025-11-23T11:00:00Z"));
    assert_eq!(n.poweroff_at.as_deref(), Some("2025-11-24T00:00:00Z"));
}

/// A job-log line of a finished job, `ended_ms` and `exit` given.
fn log_line(id: i64, ended_ms: i64, state: &str, exit: i32, node: &str) -> String {
    format!(
        r#"{{"id":{id},"argv":["true"],"cwd":"/w","repo":"r","toolchain":"other","remote":false,"tokens":1,"state":"{state}","enqueued_ms":{s},"started_ms":{s},"ended_ms":{ended_ms},"exit":{exit},"cpu_s":0.0,"compile_server_cpu_s":0.0,"peak_rss":0,"node":"{node}"}}"#,
        s = ended_ms - 1000
    )
}

/// The node's counts of finished jobs come from its whole job log, not
/// from the finished jobs it keeps for the page: the last 60 minutes and
/// each clock hour of the last 25, failures split by cause; a job this
/// daemon handed to a node counts on that node, not here.
#[test]
fn the_local_node_counts_ended_jobs_from_the_job_log_by_cause() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut cfg = config(&root, 2);
    cfg.keep_jobs = 3;
    std::fs::create_dir_all(&cfg.state_dir).unwrap();
    let now = now_ms();
    let min = 60_000;
    let mut lines = Vec::new();
    let mut id = 0;
    let mut add = |ago_min: i64, state: &str, exit: i32, node: &str| {
        id += 1;
        lines.push(log_line(id, now - ago_min * min, state, exit, node));
    };
    for _ in 0..20 {
        add(10, "ok", 0, "local");
    }
    add(11, "failed", 101, "local");
    add(12, "failed", 1, "");
    add(13, "failed", 124, "local");
    add(14, "failed", 127, "local");
    add(15, "failed", 3, "local");
    add(16, "failed", 125, "local");
    add(17, "cancelled", -1, "local");
    add(20, "failed", 101, "build-1");
    add(180, "ok", 0, "local");
    add(30 * 60, "ok", 0, "local");
    std::fs::write(cfg.state_dir.join("jobs.jsonl"), lines.join("\n") + "\n").unwrap();
    let server = Server::new(cfg).unwrap();
    let n = server.nodes().remove(0);
    assert_eq!(
        n.ended_hour,
        Some(crate::types::Outcomes {
            ok: 20,
            test: 2,
            timeout: 1,
            infra: 3,
            cancelled: 1,
        })
    );
    let hours = n.ended_hours.unwrap_or_default();
    let ok: u32 = hours.iter().map(|h| h.n.ok).sum();
    assert_eq!(ok, 21, "the job of 3 h ago counts, the one of 30 h ago not");
    assert!(hours.windows(2).all(|w| w[0].t < w[1].t), "oldest first");
    assert!(hours.iter().all(|h| h.t % 3_600_000 == 0), "whole hours");
}

/// A job that ends is counted at once, by its cause.
#[test]
fn an_ended_job_is_counted_by_its_cause() {
    let f = start(2);
    assert_eq!(run(&f, sh(&f.root, "exit 127")).code, 127);
    assert_eq!(run(&f, sh(&f.root, "exit 0")).code, 0);
    let n = f.server.nodes().remove(0);
    let h = n.ended_hour.unwrap_or_default();
    assert_eq!((h.ok, h.infra, h.test), (1, 1, 0));
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

/// A job that runs until the file `go` exists in the fixture's root, then
/// makes the file `first-done`; returned once it runs.
fn first_until_go(f: &Fixture) -> UnixStream {
    // The job polls for the file `go` the test writes.
    let (c, id) = submit(
        f,
        sh(
            &f.root,
            "while [ ! -e go ]; do sleep 0.02; done; : > first-done",
        ),
    );
    wait_for("the first job to run", || {
        state_of(f, id) == JobState::Running
    });
    c
}

/// A job that prints `after` when the file `first-done` exists as it runs,
/// else `beside`.
fn second_after_first(f: &Fixture) -> Request {
    sh(&f.root, "test -e first-done && echo after || echo beside")
}

#[test]
fn a_pending_upgrade_holds_admission() {
    let f = start(1);
    let running = first_until_go(&f);
    let (queued_before, _) = submit(&f, sh(&f.root, "echo before"));
    wait_for("the job to wait for the slot", || {
        f.server.pool().waiting() == 1
    });
    f.server.hold_admission();
    let (queued_after, _) = submit(&f, sh(&f.root, "echo after"));
    std::fs::write(f.root.join("go"), "").unwrap();
    let r = collect(running);
    assert_eq!((r.code, r.started), (0, true), "the running job finishes");
    wait_for("the moment to re-execute", || f.server.nothing_running());
    let states: Vec<JobState> = {
        let g = f.server.inner.lock().unwrap();
        g.active.values().map(|l| l.job.state).collect()
    };
    assert_eq!(states, [JobState::Queued, JobState::Queued]);
    wait_for("the slot to go back to the pool", || {
        f.server.pool().free() == 1
    });
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
    let running = first_until_go(&f);
    f.server.hold_admission();
    let (queued, _) = submit(&f, sh(&f.root, "echo after"));
    assert!(!f.server.upgrade_step(upgrade::Step::Release));
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(running).code, 0);
    let r = collect(queued);
    assert_eq!((r.code, r.started, r.out.as_str()), (0, true, "after\n"));
}

#[test]
fn second_job_waits_for_the_slot() {
    let f = start(1);
    let first = first_until_go(&f);
    let (second, id) = submit(&f, second_after_first(&f));
    wait_for("the second job to wait for the slot", || {
        f.server.pool().waiting() == 1
    });
    assert_eq!(state_of(&f, id), JobState::Queued);
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(first).code, 0);
    let r = collect(second);
    assert_eq!((r.code, r.out.as_str()), (0, "after\n"), "{r:?}");
}

#[test]
fn client_disconnect_kills_job_and_returns_token() {
    let f = start(1);
    let marker = f.root.join("pid");
    // `sleep 30` is the job's run; the client leaving ends it.
    let c = send(
        &f.socket,
        sh(
            &f.root,
            &format!("echo $$ > {}; sleep 30", marker.display()),
        ),
    );
    let pid = pid_in(&marker);
    drop(c);
    wait_for("the job to end after its client left", || {
        !Path::new(&format!("/proc/{pid}")).exists()
    });
    wait_for("the slot to come back", || f.server.pool().free() == 1);
}

/// The process id a job wrote to `marker`, polled for up to [`WAIT`].
fn pid_in(marker: &Path) -> i32 {
    let read = || {
        std::fs::read_to_string(marker)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
    };
    wait_for("the job to start", || read().is_some());
    read().unwrap()
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
    // `sleep 30` is the job's run; the test cancels it.
    let running = send(
        &f.socket,
        sh(
            &f.root,
            &format!("echo $$ > {}; sleep 30", marker.display()),
        ),
    );
    let pid = pid_in(&marker);
    let (queued, id) = submit(&f, sh(&f.root, "true"));
    assert_eq!(id, 2);

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
    wait_for("the slot to come back", || f.server.pool().free() == 1);

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
            same_path: false,
            ..Workspace::default()
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
            same_path: false,
            client_root: "/client".into(),
            ..Workspace::default()
        }),
        ..Request::default()
    };
    let r = run(&f, req);
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out.trim(), "/client");
}

/// On an idle node of 64 CPUs a remote job gets the whole node as its
/// width, past the 16 test threads of the workstation rule, and raises a
/// lower count its caller sent; a second job started within the window
/// gets half. A local job keeps the workstation rule.
#[test]
fn a_remote_job_on_an_idle_64_cpu_node_gets_the_free_cores() {
    let f = start_cfg(64, |c| {
        c.host_cpus = Some(64);
        c.test_threads = 16;
        c.cpu_busy = Some(|| Some(cpu_limit::Parts::default()));
    });
    std::fs::create_dir_all(f.root.join("ws/w1")).unwrap();
    let show = "echo $RUST_TEST_THREADS $NEXTEST_TEST_THREADS w=$SLOT_WIDTH j=$CARGO_BUILD_JOBS";
    let remote = || Request {
        argv: vec!["sh".into(), "-c".into(), show.into()],
        env: vec![
            "RUST_TEST_THREADS=1".into(),
            "NEXTEST_TEST_THREADS=4".into(),
        ],
        workspace: Some(Workspace {
            id: "w1".into(),
            rel_cwd: ".".into(),
            client_root: "/c".into(),
            ..Workspace::default()
        }),
        ..Request::default()
    };
    let r = run(&f, remote());
    assert_eq!(r.out.trim(), "64 64 w=64 j=", "{r:?}");
    let r = run(&f, remote());
    assert_eq!(r.out.trim(), "32 32 w=32 j=", "{r:?}");
    // A bound per job lower than the width is raised to it: the third
    // start within the window gets a third of the free CPUs.
    assert_eq!(set_job_parallel(&f, Some(4)), Some(4));
    let r = run(&f, remote());
    assert_eq!(r.out.trim(), "21 21 w=21 j=21", "{r:?}");
    let widths: Vec<_> = f.server.state().done.iter().map(|j| j.width).collect();
    assert_eq!(widths, [Some(64), Some(32), Some(21)], "oldest first");

    let mut local = sh(&f.root, show);
    local.env = std::env::vars()
        .filter(|(k, _)| k == "PATH")
        .map(|(k, v)| format!("{k}={v}"))
        .chain(["NEXTEST_TEST_THREADS=4".to_string()])
        .collect();
    let r = run(&f, local);
    assert_eq!(r.out.trim(), "16 4 w= j=4", "{r:?}");
    assert_eq!(f.server.state().done.last().unwrap().width, None);
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
            same_path: false,
            rel_cwd: ".".into(),
            client_root: "/c".into(),
            ..Workspace::default()
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
    wait_for("every slot back", || f.server.pool().free() == 2);
}

#[test]
fn requested_tokens_replace_the_toolchains() {
    let f = start(4);
    let mut req = sh(&f.root, "true");
    req.tokens = Some(3);
    let r = run(&f, req);
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(f.server.state().done[0].tokens, 3);
    wait_for("all slots back", || f.server.pool().free() == 4);
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
        pass_for: Duration::from_secs(300),
        clock: now_ms,
        target_poll: Duration::from_millis(40),
        target_settle: Duration::from_millis(250),
        token_watch: TOKEN_WATCH,
        resource_poll: RESOURCE_POLL,
        cpu_busy: None,
        host_cpus: None,
        cpu_hold_max: cpu_limit::HOLD_MAX,
        cpu_bound_gap: Duration::from_millis(cpu_limit::STARTING_MS as u64),
        cpu_pressure: None,
        cache_budget: 0,
        seed_copy: cache::SeedCopy::Reflink,
        evict: crate::evict::Settings {
            floor: 0,
            margin: 0,
            min_idle: Duration::from_secs(7200),
            dry_run: true,
        },
        disk_free: None,
        exec_shim: None,
        handover: true,
        sandbox: None,
        cluster_file: None,
        same_path_roots: Vec::new(),
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
    let first = first_until_go(&f);
    let (second, id) = submit(&f, second_after_first(&f));
    wait_for("the second job to wait for the slot", || {
        f.server.pool().waiting() == 1
    });
    assert_eq!(state_of(&f, id), JobState::Queued);
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(first).code, 0);
    let r = collect(second);
    assert_eq!(
        (r.code, r.out.as_str()),
        (0, "after\n"),
        "second job ran beside the first with 1 of 2 tokens circulating"
    );
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
            min_free_disk_light: 0,
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
    wait_for("the job to leave the table", || {
        f.server.state().active.is_empty()
    });
    let st = f.server.state();
    assert_eq!(st.done.last().unwrap().state, JobState::Cancelled);
}

#[test]
fn memory_short_holds_a_job_only_while_another_runs() {
    let f = start_with(
        2,
        crate::admission::Limits {
            min_free_memory: u64::MAX / 4,
            min_free_disk: 0,
            min_free_disk_light: 0,
        },
    );
    let alone = run(&f, sh(&f.root, "echo alone"));
    assert_eq!(
        (alone.code, alone.out.as_str()),
        (0, "alone\n"),
        "alone: runs"
    );
    let first = first_until_go(&f);
    let (second, b) = submit(&f, second_after_first(&f));
    wait_for("the second job to wait for memory", || {
        reason_of(&f, b).contains("memory available")
    });
    assert_eq!(state_of(&f, b), JobState::Queued);
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(first).code, 0);
    let second = collect(second);
    assert_eq!(
        (second.code, second.out.as_str()),
        (0, "after\n"),
        "ran beside the first"
    );
    assert!(second.info.contains("memory available"), "{}", second.info);
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
    let busy_cwds: HashSet<PathBuf> = [w.join("busy")].into();
    let (_, planned) = evict_pass(&seen, &HashSet::new(), &busy_cwds, now, s);
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
    // git runs off the request path: the job polls for the file `go`, which
    // the test writes once the job has its project.
    let (c, id) = submit(&f, sh(&repo, "while [ ! -e go ]; do sleep 0.02; done"));
    wait_for("the job's project", || {
        active_job(&f, id).is_some_and(|j| j.project == "team-a")
    });
    std::fs::write(repo.join("go"), "").unwrap();
    assert_eq!(collect(c).code, 0);
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

/// The bound of [`wait_for`] and [`wait_for_jobs`].
const WAIT: Duration = Duration::from_secs(10);

/// Polls `cond` for up to [`WAIT`].
fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        // The poll interval of a bounded wait.
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A job that holds its slot until the file `go` exists in `cwd`.
fn holder(f: &Fixture) -> UnixStream {
    // The job polls for the file `go` the test writes.
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
            Frame::Info { .. } | Frame::Err { .. } | Frame::Fallback { .. } => {}
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
    // The job polls for the file `go` the test writes.
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

/// Polls `cond` for up to [`WAIT`] like [`wait_for`]; a timeout names every
/// active job with its state and reason, and the pool's free slots and
/// queue.
fn wait_for_jobs(f: &Fixture, what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        // The poll interval of a bounded wait.
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
    // Past the settle time of the first job, and two polls of the target
    // directories after it, only the lock holds it back.
    let started = active_job(&f, a).unwrap().started_ms;
    let past = {
        let c = &f.server.cfg;
        i64::try_from((c.target_settle + 2 * c.target_poll).as_millis()).unwrap()
    };
    wait_for("the settle time of the first job to pass", || {
        now_ms() > started + past
    });
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
        // `sleep 0.2` widens each run, so two that overlapped would show in
        // `order`; nothing waits on it.
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
    // The job polls for the file `go-other` the test writes.
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
    // The job polls for the file `go` the test writes.
    let mut all = sh(&f.root, "while [ ! -e go ]; do sleep 0.02; done");
    all.tokens = Some(2);
    let holder = send(&f.socket, all);
    wait_for("both slots to be taken", || f.server.pool().free() == 0);
    let (normal, n) = submit(&f, fake_build(&f.root, &target, "echo normal >> order"));
    wait_for("the first build to queue for a slot", || {
        f.server.pool().waiting() == 1
    });
    assert_eq!(reason_of(&f, n), "");
    // The job polls for the file `go-high` the test writes.
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
            same_path: false,
            id: "w1".into(),
            rel_cwd: ".".into(),
            client_root: "/c".into(),
            ..Workspace::default()
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
    // `sleep 30` runs past the limit of 1 s; the daemon ends it.
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
    // `sleep 2.5` runs past the daemon's limit of 1 s: that is the case tested.
    let mut own = sh(&f.root, "sleep 2.5; echo own");
    own.timeout_s = Some(60);
    let r = run_bounded(&f, own);
    assert_eq!((r.code, r.out.as_str()), (0, "own\n"), "{r:?}");
    assert!(!r.info.contains("run-time limit"), "{}", r.info);
    let job = last_logged(&f);
    assert_eq!((job.timed_out, job.limit_s), (false, Some(60)), "{job:?}");
    assert!(job.ended_ms - job.started_ms >= 2000, "{job:?}");
    set_run_limit(&f, Some(30), None);
    // `sleep 30` runs past the job's own 1 s; the daemon ends it.
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
    // `sleep 30` runs past the limit of 1 s; the daemon ends it.
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
    let set = now_ms();
    // The check is that nothing happens: let the new limit of 1 s pass
    // twice over, then look.
    wait_for("2.2 s after the limit was set", || now_ms() > set + 2200);
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
            // The children's `sleep 600` keeps them alive; `kill` ends them.
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
            // `sleep 600` keeps the client alive; `kill` ends it.
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
    // The job polls for the file `go` the test writes.
    let mut req = sh(&f.root, "while [ ! -e go ]; do sleep 0.02; done");
    req.tokens = Some(tokens);
    let (c, id) = submit(f, req);
    wait_for_jobs(f, "the job to run", || state_of(f, id) == JobState::Running);
    (c, id)
}

/// A job that waits in the queue for one slot.
fn waiting_job(f: &Fixture) -> (UnixStream, i64) {
    let before = f.server.pool().waiting();
    // The job polls for the file `go` the test writes.
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
        // The poll interval of a bounded wait.
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
            ..cpu_limit::Request::default()
        })
        .unwrap()
}

/// Submits a job that uses no CPU and runs until its client goes away.
fn quiet(f: &Fixture) -> (UnixStream, i64) {
    // `sleep 60` is the job's run; the test ends it.
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

// wave w1 u5

/// Waits until the job ends, then appends `$NAME` to the file `order`: a
/// job predicted short once it has history. Jobs of this command share
/// their history whatever their `NAME`.
const SHORT_CMD: &str = "while [ ! -e go-$NAME ]; do sleep 0.02; done; echo $NAME >> order";

/// Appends `$NAME` to the file `order`: a command without history.
const LONG_CMD: &str = "echo $NAME >> order";

/// A job of repository `repo` that runs `cmd` with `NAME=name` in its
/// environment and asks for `tokens` slots.
fn named(f: &Fixture, cmd: &str, repo: &str, name: &str, tokens: Option<u32>) -> Request {
    let mut r = sh(&f.root, cmd);
    r.repo = repo.into();
    r.tokens = tokens;
    r.env = std::env::vars()
        .map(|(k, v)| format!("{k}={v}"))
        .chain([format!("NAME={name}")])
        .collect();
    r
}

/// Lets the job named `name` of [`SHORT_CMD`] end.
fn go(f: &Fixture, name: &str) {
    std::fs::write(f.root.join(format!("go-{name}")), "").unwrap();
}

/// Sends `req` and waits until it waits for slots.
fn queued_job(f: &Fixture, req: Request) -> (UnixStream, i64) {
    let before = f.server.pool().waiting();
    let (c, id) = submit(f, req);
    wait_for_jobs(f, "the job to wait for slots", || {
        f.server.pool().waiting() == before + 1
    });
    (c, id)
}

/// Runs [`SHORT_CMD`] three times in `repo`, so the daemon predicts its run
/// time, and empties the file `order`.
fn give_history(f: &Fixture, repo: &str) {
    go(f, "history");
    for _ in 0..3 {
        let r = run(f, named(f, SHORT_CMD, repo, "history", None));
        assert_eq!(r.code, 0, "{r:?}");
    }
    std::fs::remove_file(f.root.join("order")).unwrap();
}

/// Slots job `id` has collected while it waits, as `slot status` reads it.
fn collected(f: &Fixture, id: i64) -> u32 {
    active_job(f, id).map_or(0, |j| j.collected)
}

/// Whether job `id` has its slots (it claimed them, or it already ended)
/// within `within`. Measured at the claim, not at the start of its
/// process: on a loaded host starting `sh` alone took over 500 ms.
fn has_slots_within(f: &Fixture, id: i64, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        let has = {
            let g = f.server.inner.lock().unwrap();
            g.active.get(&id).is_none_or(|l| l.held > 0)
        };
        if has {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A job that asks for every slot does not keep freed slots idle: with 4
/// slots, one job running on 1 and a 4-slot job queued, three 1-slot jobs
/// that queue behind it each get their slot within 1 s and run, each
/// logged as moved ahead. The
/// 4-slot job shows the slots it collected, and the token count books
/// them: none is missing. Then it starts once the running job ends.
#[test]
fn one_slot_jobs_start_while_a_job_that_asks_for_every_slot_waits() {
    let f = start(4);
    let (h, _) = holding_job(&f, 1);
    let (big, big_id) = queued_job(&f, named(&f, LONG_CMD, "big", "big", Some(4)));
    wait_for_jobs(&f, "the 4-slot job to collect 3", || {
        collected(&f, big_id) == 3
    });
    assert_eq!(
        f.server.token_count(),
        (1, 0, 0),
        "granted, missing, surplus"
    );
    let mut small = Vec::new();
    for i in 0..3 {
        let (c, id) = submit(&f, named(&f, LONG_CMD, "small", &format!("s{i}"), None));
        assert!(
            has_slots_within(&f, id, Duration::from_secs(1)),
            "1-slot job {i} did not get its slot within 1 s"
        );
        small.push((c, id));
    }
    let mut ids = Vec::new();
    for (c, id) in small {
        let r = collect(c);
        assert_eq!((r.code, r.started), (0, true), "{r:?}");
        ids.push(id);
    }
    for id in ids {
        assert!(
            done_job(&f, id).ahead,
            "job {id} is not logged as moved ahead"
        );
    }
    wait_for_jobs(&f, "the 4-slot job to collect 3 again", || {
        collected(&f, big_id) == 3
    });
    assert_eq!(f.server.token_count(), (1, 0, 0));
    assert_eq!(state_of(&f, big_id), JobState::Queued);
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(h).code, 0);
    assert_eq!(collect(big).code, 0);
    let ran = order(&f);
    assert_eq!(
        (ran.len(), ran.last().map(String::as_str)),
        (4, Some("big"))
    );
    assert!(!done_job(&f, big_id).ahead);
}

/// The bound of passing: a 1-slot job passes a 2-slot job that waits, but
/// once the 2-slot job has waited `--pass-for` (300 s, on a clock the test
/// moves) a 1-slot job that queues after it waits, and the 2-slot job
/// starts first.
#[test]
fn once_a_job_that_asks_for_every_slot_has_waited_the_bound_it_starts_first() {
    static SKEW_MS: AtomicI64 = AtomicI64::new(0);
    fn clock() -> i64 {
        now_ms() + SKEW_MS.load(Ordering::SeqCst)
    }
    let f = start_cfg(2, |c| c.clock = clock);
    let (h, _) = holding_job(&f, 1);
    let (big, big_id) = queued_job(&f, named(&f, LONG_CMD, "big", "big", Some(2)));
    wait_for_jobs(&f, "the 2-slot job to collect 1", || {
        collected(&f, big_id) == 1
    });
    let s1 = run(&f, named(&f, LONG_CMD, "small", "s1", None));
    assert_eq!((s1.code, s1.started), (0, true), "passed it: {s1:?}");
    SKEW_MS.store(301_000, Ordering::SeqCst);
    f.server.rerank();
    let (s2, s2_id) = queued_job(&f, named(&f, LONG_CMD, "small", "s2", None));
    wait_for_jobs(&f, "the 2-slot job to collect 1 again", || {
        collected(&f, big_id) == 1
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        state_of(&f, s2_id),
        JobState::Queued,
        "started ahead of a job that waited the bound"
    );
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(h).code, 0);
    assert_eq!(collect(big).code, 0);
    assert_eq!(collect(s2).code, 0);
    assert_eq!(order(&f), ["s1", "big", "s2"]);
}

/// Short jobs first. A pool of 1 slot, one level: a job without history
/// queues, then one whose last runs predict 5 s or less. When the slot
/// frees, the short job starts first. Its record carries the prediction
/// and says it was moved ahead, also in the job log; the other job has
/// neither.
#[test]
fn a_job_predicted_short_starts_ahead_of_a_longer_one_of_its_level() {
    let f = start(1);
    give_history(&f, "app");
    let h = holder(&f);
    let (long, long_id) = queued_job(&f, named(&f, LONG_CMD, "app", "long", None));
    go(&f, "short");
    let (short, short_id) = queued_job(&f, named(&f, SHORT_CMD, "app", "short", None));
    let waiting = active_job(&f, short_id).unwrap();
    assert!(
        waiting
            .predicted_ms
            .is_some_and(|ms| ms <= priority::SHORT_MS),
        "{waiting:?}"
    );
    wait_for_jobs(&f, "the short job to show as moved ahead", || {
        active_job(&f, short_id).is_some_and(|j| j.ahead)
    });
    assert_eq!(run_all(&f, h, vec![long, short]), ["short", "long"]);
    let (short_job, long_job) = (done_job(&f, short_id), done_job(&f, long_id));
    assert!(
        short_job.ahead && short_job.predicted_ms.is_some(),
        "{short_job:?}"
    );
    assert_eq!(
        (long_job.ahead, long_job.predicted_ms),
        (false, None),
        "{long_job:?}"
    );
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    let line = |id: i64| {
        log.lines()
            .find(|row| row.contains(&format!("\"id\":{id},")))
            .unwrap()
            .to_string()
    };
    assert!(line(short_id).contains("\"predicted_ms\":"), "{log}");
    assert!(line(short_id).contains("\"ahead\":true"), "{log}");
    assert!(!line(long_id).contains("predicted_ms"), "{log}");
    assert!(!line(long_id).contains("ahead"), "{log}");
}

/// A stream of short jobs passes a longer one only until it has waited
/// `--pass-for` (300 s, on a clock the test moves): two short jobs start
/// ahead of it one after the other, the third, queued after the bound,
/// starts after it.
#[test]
fn a_stream_of_short_jobs_does_not_hold_a_longer_job_past_the_bound() {
    static SKEW_MS: AtomicI64 = AtomicI64::new(0);
    fn clock() -> i64 {
        now_ms() + SKEW_MS.load(Ordering::SeqCst)
    }
    let f = start_cfg(1, |c| c.clock = clock);
    give_history(&f, "app");
    let h = holder(&f);
    let (long, long_id) = queued_job(&f, named(&f, LONG_CMD, "app", "long", None));
    let (s0, s0_id) = queued_job(&f, named(&f, SHORT_CMD, "app", "s0", None));
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(h).code, 0);
    wait_for_jobs(&f, "s0 to run", || state_of(&f, s0_id) == JobState::Running);
    let (s1, s1_id) = queued_job(&f, named(&f, SHORT_CMD, "app", "s1", None));
    go(&f, "s0");
    wait_for_jobs(&f, "s1 to run", || state_of(&f, s1_id) == JobState::Running);
    assert_eq!(state_of(&f, long_id), JobState::Queued);
    SKEW_MS.store(301_000, Ordering::SeqCst);
    f.server.rerank();
    go(&f, "s2");
    let (s2, _) = queued_job(&f, named(&f, SHORT_CMD, "app", "s2", None));
    go(&f, "s1");
    for c in [s0, s1, long, s2] {
        assert_eq!(collect(c).code, 0);
    }
    assert_eq!(order(&f), ["s0", "s1", "long", "s2"]);
}

/// The order between levels is unchanged: a short `normal` job does not
/// pass a `high` job without history.
#[test]
fn a_short_job_does_not_pass_a_job_of_a_higher_level() {
    let f = start(1);
    give_history(&f, "app");
    let h = holder(&f);
    let mut high = named(&f, LONG_CMD, "app", "high", None);
    high.priority = Some(Priority::High);
    let (high, _) = queued_job(&f, high);
    go(&f, "short");
    let (short, short_id) = queued_job(&f, named(&f, SHORT_CMD, "app", "short", None));
    assert_eq!(run_all(&f, h, vec![high, short]), ["high", "short"]);
    assert!(!done_job(&f, short_id).ahead);
}

/// With `--pass-for 0` the queue is served by level, then arrival: a job
/// predicted short waits behind an earlier one without history, and is
/// not moved ahead. The prediction is still recorded.
#[test]
fn with_passing_off_a_short_job_waits_its_turn() {
    let f = start_cfg(1, |c| c.pass_for = Duration::ZERO);
    give_history(&f, "app");
    let h = holder(&f);
    let (long, _) = queued_job(&f, named(&f, LONG_CMD, "app", "long", None));
    go(&f, "short");
    let (short, short_id) = queued_job(&f, named(&f, SHORT_CMD, "app", "short", None));
    assert_eq!(run_all(&f, h, vec![long, short]), ["long", "short"]);
    let s = done_job(&f, short_id);
    assert_eq!((s.ahead, s.predicted_ms.is_some()), (false, true), "{s:?}");
}

// adversary u5

/// Design § 25: once a job has waited `--pass-for`, "from then on it
/// starts before every job that came after it". The daemon's clock steps
/// back an hour (an NTP step, a manual `date`) after the 2-slot job became
/// unpassable: the wait reads as 0 again, the job is passable again, and a
/// 1-slot job that queues after it starts ahead of it.
#[test]
fn adversary_u5_a_job_that_waited_the_bound_stays_unpassable_when_the_clock_steps_back() {
    static SKEW_MS: AtomicI64 = AtomicI64::new(0);
    fn clock() -> i64 {
        now_ms() + SKEW_MS.load(Ordering::SeqCst)
    }
    let f = start_cfg(2, |c| c.clock = clock);
    let (h, _) = holding_job(&f, 1);
    let (big, big_id) = queued_job(&f, named(&f, LONG_CMD, "big", "big", Some(2)));
    wait_for_jobs(&f, "the 2-slot job to collect 1", || {
        collected(&f, big_id) == 1
    });
    SKEW_MS.store(301_000, Ordering::SeqCst);
    f.server.rerank();
    SKEW_MS.store(-3_600_000, Ordering::SeqCst);
    f.server.rerank();
    let (s2, s2_id) = submit(&f, named(&f, LONG_CMD, "small", "s2", None));
    let passed = has_slots_within(&f, s2_id, Duration::from_millis(500));
    std::fs::write(f.root.join("go"), "").unwrap();
    assert_eq!(collect(h).code, 0);
    assert_eq!(collect(big).code, 0);
    assert_eq!(collect(s2).code, 0);
    assert!(
        !passed,
        "started ahead of a job that had waited the bound; order {:?}",
        order(&f)
    );
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
    // The job works in another tree: its own would keep the target.
    let elsewhere = f.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let (c, id) = submit(&f, sh(&elsewhere, "echo ran"));
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

/// A daemon with a disk hold's mark that knows no target directory looks
/// and measures nothing: its report says so and `slot status` names no free
/// space, where it read `0.0 GB free, under 50.0 GB`. Once a known target
/// directory exists, the next look measures its filesystem.
#[test]
fn a_look_without_a_known_target_directory_claims_no_free_space() {
    const GB: u64 = crate::admission::GB;
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        Some(300 * GB)
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
    });
    wait_for_jobs(&f, "the watch's first look", || {
        f.server.state().evict.checked_ms > 0
    });
    let st = f.server.state();
    let text = crate::status::render(&st);
    assert!(!text.contains("0.0 GB free"), "{text}");
    assert!(
        text.contains(
            "disk: no target dir known, nothing measured (eviction below 50.0 GB, raised \
             from 40.0 GB to the disk hold's mark)\n"
        ),
        "{text}"
    );
    let e = &st.evict;
    assert_eq!((e.unmeasured, e.free, e.known), (true, 0, 0), "{e:?}");

    old_target(&f, "old");
    evict_view(put_evict_below(&f, r#"{"evict_below_gb":40}"#));
    wait_for_jobs(&f, "the look at the known target directory", || {
        f.server.state().evict.known == 1
    });
    let st = f.server.state();
    let e = &st.evict;
    assert_eq!((e.unmeasured, e.free), (false, 300 * GB), "{e:?}");
    let text = crate::status::render(&st);
    assert!(
        text.contains(
            "disk: 300.0 GB free, eviction below 50.0 GB, raised from 40.0 GB to the disk \
             hold's mark (1 target dir(s) known)\n"
        ),
        "{text}"
    );
}

/// A cargo job that arrived with a same-path workspace of id `ws`, mirrored
/// at `tree`: it runs `script` there. `target` names its target directory
/// when that is not `target` in the tree.
fn node_build(f: &Fixture, ws: &str, tree: &Path, target: Option<&Path>, script: &str) -> Request {
    std::fs::create_dir_all(tree).unwrap();
    Request {
        cwd: String::new(),
        repo: "r".into(),
        workspace: Some(Workspace {
            id: ws.into(),
            rel_cwd: ".".into(),
            client_root: tree.to_string_lossy().into_owned(),
            same_path: true,
            target_dir: target.map_or_else(String::new, |t| t.to_string_lossy().into_owned()),
            target_default: false,
        }),
        ..fake_build(&f.root, &f.root.join("unused"), script)
    }
}

/// On a build node every job arrives with a workspace and builds into the
/// target directory its record names (`node_target_dir`). That directory is
/// known to eviction like a local job's: idle, it is deleted when free
/// space is under the mark; one a running job builds into is kept, also
/// when it lies outside that job's tree.
#[test]
fn a_node_evicts_the_idle_target_directory_of_a_job_it_ran_and_keeps_one_in_use() {
    use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
    const GB: u64 = crate::admission::GB;
    static FREE: AtomicU64 = AtomicU64::new(60 * GB);
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        Some(FREE.load(SeqCst))
    }
    let f = start_cfg(2, |c| {
        c.same_path_roots = vec![c.ws_root.with_file_name("home")];
        c.disk_free = Some(free);
        c.evict = crate::evict::Settings {
            floor: 40 * GB,
            margin: 10 * GB,
            min_idle: Duration::ZERO,
            dry_run: false,
        };
    });
    let home = f.root.join("home");
    let idle_tree = home.join("trees/idle");
    let idle = idle_tree.join("target");
    let used_tree = home.join("trees/used");
    let used = home.join("targets/used");
    // What cargo leaves in a target directory it made.
    let build = format!(
        "t=${{CARGO_TARGET_DIR:-target}}; mkdir -p \"$t/debug\"; \
         echo '{}' > \"$t/CACHEDIR.TAG\"; echo built > \"$t/debug/blob\"",
        crate::evict::CACHEDIR_SIGNATURE
    );
    let r = run(&f, node_build(&f, "a1", &idle_tree, None, &build));
    assert_eq!(r.code, 0, "{r:?}");
    let r = run(&f, node_build(&f, "b1", &used_tree, Some(&used), &build));
    assert_eq!(r.code, 0, "{r:?}");
    assert!(crate::evict::is_target_dir(&idle) && crate::evict::is_target_dir(&used));
    let known = || -> Vec<PathBuf> {
        let g = f.server.inner.lock().unwrap();
        let mut v: Vec<PathBuf> = g.targets.keys().cloned().collect();
        v.sort();
        v
    };
    assert_eq!(known(), [used.clone(), idle.clone()], "both are known");

    // Both jobs ended long ago.
    {
        let mut g = f.server.inner.lock().unwrap();
        g.targets.values_mut().for_each(|t| t.1 = 1);
        g.ran_in.values_mut().for_each(|at| *at = 1);
    }
    // A job of the second tree runs: it builds into `used`.
    let go = f.root.join("go-used");
    let wait = format!("while [ ! -e {} ]; do sleep 0.02; done", go.display());
    let (c, id) = submit(&f, node_build(&f, "b1", &used_tree, Some(&used), &wait));
    wait_for_jobs(&f, "the job to run", || {
        state_of(&f, id) == JobState::Running
    });

    FREE.store(30 * GB, SeqCst);
    evict_view(put_evict_below(&f, r#"{"evict_below_gb":40}"#));
    wait_for_jobs(&f, "the look under the mark", || {
        !f.server.state().evict.last.is_empty()
    });
    let e = f.server.state().evict;
    assert!(!idle.exists(), "idle and under the mark: {e:?}");
    assert!(used.exists(), "a running job builds into it: {e:?}");
    assert_eq!((e.known, e.free), (2, 30 * GB), "{e:?}");
    assert_eq!(e.last.len(), 1, "{e:?}");
    assert_eq!(
        (e.last[0].path.as_str(), e.last[0].action.as_str()),
        (idle.to_str().unwrap(), "deleted"),
        "{e:?}"
    );
    std::fs::write(&go, "").unwrap();
    assert_eq!(collect(c).code, 0);
}

/// The record of a job this daemon handed to a node names the directory it
/// built into on that node: no directory of this machine, so eviction here
/// does not learn it. The record of a job that arrived with a workspace
/// names one of this machine.
#[test]
fn a_job_handed_to_a_node_names_no_target_directory_of_this_machine() {
    let job = |remote: bool, node: &str| -> Job {
        serde_json::from_value(serde_json::json!({
            "id": 1, "argv": ["cargo", "build"], "cwd": "/w/r", "repo": "r",
            "toolchain": "rust", "remote": remote, "tokens": 1, "state": "ok",
            "enqueued_ms": 1, "started_ms": 2, "ended_ms": 3, "exit": 0, "cpu_s": 0.0,
            "compile_server_cpu_s": 0.0, "peak_rss": 0, "node": node,
            "node_target_dir": "/w/r/target",
        }))
        .unwrap()
    };
    let noted = |j: &Job| {
        let mut targets = HashMap::new();
        record_target(&mut targets, j, Path::new("/srv/cache"));
        targets.into_keys().collect::<Vec<PathBuf>>()
    };
    let here = [PathBuf::from("/w/r/target")];
    let none: [PathBuf; 0] = [];
    assert_eq!(noted(&job(true, LOCAL_NODE)), here);
    assert_eq!(noted(&job(true, "")), here, "a line older than nodes");
    assert_eq!(noted(&job(false, "build-1")), none);
    assert_eq!(noted(&job(true, "build-1")), none);
    let mut local = job(false, "build-1");
    local.target_dir = "/w/r/own".into();
    assert_eq!(noted(&local), [PathBuf::from("/w/r/own")], "as before");
}

/// A job whose workspace is mirrored under the workspace root builds into
/// `<cache-root>/target/<id>`. That directory is the cache LRU's alone,
/// which keeps its size in a record: eviction does not learn it, so the
/// record cannot go stale. A same-path job's directory, which the LRU never
/// deletes, is known.
#[test]
fn a_target_directory_under_the_cache_root_is_not_known_to_eviction() {
    let f = start_cfg(2, |c| {
        c.same_path_roots = vec![c.ws_root.with_file_name("home")];
    });
    let build = format!(
        "t=${{CARGO_TARGET_DIR:-target}}; mkdir -p \"$t/debug\"; \
         echo '{}' > \"$t/CACHEDIR.TAG\"; echo built > \"$t/debug/blob\"",
        crate::evict::CACHEDIR_SIGNATURE
    );
    let tree = f.root.join("home/trees/own");
    let r = run(&f, node_build(&f, "a1", &tree, None, &build));
    assert_eq!(r.code, 0, "{r:?}");
    std::fs::create_dir_all(f.root.join("ws/c1")).unwrap();
    let mut mirrored = node_build(&f, "c1", &f.root.join("client"), None, &build);
    if let Some(ws) = &mut mirrored.workspace {
        ws.same_path = false;
    }
    let r = run(&f, mirrored);
    assert_eq!(r.code, 0, "{r:?}");
    let cached = f.root.join("cache/target/c1");
    assert!(crate::evict::is_target_dir(&cached), "the job built there");
    let done = f.server.state().done;
    let ran: Vec<Option<String>> = done.iter().map(|j| j.node_target_dir.clone()).collect();
    assert!(
        ran.contains(&Some(cached.to_string_lossy().into_owned())),
        "its record names the directory: {ran:?}"
    );
    let known: Vec<PathBuf> = {
        let g = f.server.inner.lock().unwrap();
        g.targets.keys().cloned().collect()
    };
    assert_eq!(known, [tree.join("target")], "only the same-path one");

    // The same at a start: the job log names both directories.
    let mut targets = HashMap::new();
    for j in &done {
        record_target(&mut targets, j, &f.root.join("cache"));
    }
    assert_eq!(
        targets.into_keys().collect::<Vec<PathBuf>>(),
        [tree.join("target")]
    );
}

/// What a compile cache keys of a job's environment: its `CARGO_*`
/// variables, sorted. `CARGO_MAKEFLAGS` names the jobserver and is no part
/// of a key.
const CARGO_VARS: &str = "env | grep '^CARGO_' | grep -v '^CARGO_MAKEFLAGS=' | LC_ALL=C sort";

/// A `cargo` under `dir` whose `fetch` succeeds and whose every other
/// subcommand prints [`CARGO_VARS`].
fn printing_cargo(dir: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cargo = bin.join("cargo");
    if !cargo.exists() {
        std::fs::write(
            &cargo,
            format!("#!/bin/sh\n[ \"$1\" = fetch ] && exit 0\n{CARGO_VARS}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    cargo.to_string_lossy().into_owned()
}

/// A job as a client sends it for a tree mirrored at its own path, with a
/// `Cargo.lock`, so a cargo job gets its fetch first. `target` is the
/// target directory the client names and whether cargo resolves it by
/// itself; `env` is what the caller's environment adds.
fn same_path_job(
    tree: &Path,
    id: &str,
    argv: &[&str],
    target: Option<(&Path, bool)>,
    env: &[&str],
) -> Request {
    std::fs::create_dir_all(tree).unwrap();
    std::fs::write(tree.join("Cargo.lock"), "version = 3\n").unwrap();
    let argv: Vec<String> = argv.iter().map(|a| (*a).to_string()).collect();
    Request {
        fetch_once: crate::cargo_fetch::applies(&argv),
        argv,
        repo: "r".into(),
        env: env.iter().map(|e| (*e).to_string()).collect(),
        workspace: Some(Workspace {
            id: id.into(),
            rel_cwd: ".".into(),
            client_root: tree.to_string_lossy().into_owned(),
            same_path: true,
            target_dir: target.map_or_else(String::new, |(t, _)| t.to_string_lossy().into_owned()),
            target_default: target.is_some_and(|(_, default)| default),
        }),
        ..Request::default()
    }
}

/// The compile cache keys every `CARGO_*` variable of a compiler's
/// environment. A cargo job of a tree mirrored at its own path, cargo inside
/// a generic job of that tree, and a cargo job of a second tree get the same
/// ones, byte for byte: the daemon adds no `CARGO_TARGET_DIR` for the
/// directory cargo resolves by itself, and no `CARGO_NET_OFFLINE`.
#[test]
fn a_cargo_job_a_generic_job_and_a_second_tree_get_the_same_cargo_variables() {
    let f = start_cfg(2, |c| {
        c.same_path_roots = vec![c.ws_root.with_file_name("home")];
    });
    let cargo = printing_cargo(&f.root);
    let a = f.root.join("home/trees/a");
    let b = f.root.join("home/trees/b");
    let vars = |req: Request| {
        let r = run(&f, req);
        assert_eq!(r.code, 0, "{r:?}");
        r.out
    };
    let own = |tree: &Path| tree.join("target");
    let in_a = vars(same_path_job(
        &a,
        "a1",
        &[&cargo, "build"],
        Some((&own(&a), true)),
        &[],
    ));
    let inner = format!("{cargo} build");
    let generic = vars(same_path_job(&a, "a1", &["sh", "-c", &inner], None, &[]));
    let in_b = vars(same_path_job(
        &b,
        "b1",
        &[&cargo, "build"],
        Some((&own(&b), true)),
        &[],
    ));
    assert!(
        !in_a.contains("CARGO_TARGET_DIR="),
        "cargo resolves the directory by itself: {in_a}"
    );
    assert_eq!(in_a, generic, "a cargo job and cargo inside a generic job");
    assert_eq!(in_a, in_b, "two trees");
    // The record still names where each job built.
    let recorded: Vec<Option<String>> = f
        .server
        .state()
        .done
        .iter()
        .filter(|j| j.toolchain == "rust")
        .map(|j| j.node_target_dir.clone())
        .collect();
    for t in [own(&a), own(&b)] {
        assert!(
            recorded.contains(&Some(t.to_string_lossy().into_owned())),
            "{recorded:?}"
        );
    }
}

/// Where cargo resolves the directory by itself, no `CARGO_BUILD_TARGET_DIR`
/// reaches the job either: the daemon's own environment (the base of a remote
/// job) would move the build away from the recorded directory. The test puts
/// it in the request, which the job's environment is built from the same way.
#[test]
fn a_default_target_directory_gets_no_cargo_build_target_dir() {
    let f = start_cfg(2, |c| {
        c.same_path_roots = vec![c.ws_root.with_file_name("home")];
    });
    let cargo = printing_cargo(&f.root);
    let tree = f.root.join("home/trees/a");
    let r = run(
        &f,
        same_path_job(
            &tree,
            "a1",
            &[&cargo, "build"],
            Some((&tree.join("target"), true)),
            &["CARGO_BUILD_TARGET_DIR=/elsewhere"],
        ),
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert!(
        !r.out.contains("CARGO_BUILD_TARGET_DIR="),
        "cargo resolves the directory by itself: {}",
        r.out
    );
}

/// A tree mirrored below the workspace root: a cargo job and cargo inside a
/// generic job get the same `CARGO_*` variables too, with the target
/// directory the daemon gives that workspace in both.
#[test]
fn in_a_mirror_below_the_workspace_root_both_job_kinds_get_the_same_cargo_variables() {
    let f = start(2);
    let cargo = printing_cargo(&f.root);
    let mirror = f.root.join("ws/m1");
    let job = |argv: &[&str]| {
        let mut req = same_path_job(&mirror, "m1", argv, None, &[]);
        if let Some(ws) = &mut req.workspace {
            ws.same_path = false;
            ws.client_root = "/client/r".into();
        }
        let r = run(&f, req);
        assert_eq!(r.code, 0, "{r:?}");
        r.out
    };
    let build = job(&[&cargo, "build"]);
    let inner = format!("{cargo} build");
    let generic = job(&["sh", "-c", &inner]);
    assert_eq!(build, generic);
    let target = format!(
        "CARGO_TARGET_DIR={}",
        f.root.join("cache/target/m1").display()
    );
    assert!(build.lines().any(|l| l == target), "{build}");
}

/// A caller that set `CARGO_TARGET_DIR` builds into that directory with the
/// variable exported, a cargo job and a generic job alike, and a
/// `CARGO_NET_OFFLINE` of the caller's own reaches the job as the caller
/// set it.
#[test]
fn the_callers_own_target_directory_and_offline_setting_reach_the_job() {
    let f = start_cfg(2, |c| {
        c.same_path_roots = vec![c.ws_root.with_file_name("home")];
    });
    let cargo = printing_cargo(&f.root);
    let tree = f.root.join("home/trees/a");
    let named = f.root.join("home/targets/named");
    let caller = &["CARGO_NET_OFFLINE=false"];
    let build = same_path_job(
        &tree,
        "a1",
        &[&cargo, "build"],
        Some((&named, false)),
        caller,
    );
    let r = run(&f, build);
    assert_eq!(r.code, 0, "{r:?}");
    let lines: Vec<&str> = r.out.lines().collect();
    let target = format!("CARGO_TARGET_DIR={}", named.display());
    assert!(lines.contains(&target.as_str()), "{lines:?}");
    let offline: Vec<&&str> = lines
        .iter()
        .filter(|l| l.starts_with("CARGO_NET_OFFLINE="))
        .collect();
    assert_eq!(offline, [&"CARGO_NET_OFFLINE=false"], "the caller's own");
    let inner = format!("{cargo} build");
    let generic = run(
        &f,
        same_path_job(
            &tree,
            "a1",
            &["sh", "-c", &inner],
            Some((&named, false)),
            caller,
        ),
    );
    assert_eq!(generic.out, r.out, "{generic:?}");
    let done = f.server.state().done;
    assert_eq!(
        done.iter()
            .find(|j| j.toolchain == "rust")
            .and_then(|j| j.node_target_dir.clone()),
        Some(named.to_string_lossy().into_owned())
    );
}

/// A job that gets no `CARGO_TARGET_DIR` is recorded and evicted as before:
/// its record names the directory it built into, eviction knows that
/// directory, deletes it when it is idle and free space is under the mark,
/// and keeps the one a running job builds into.
#[test]
fn a_job_without_the_target_variable_is_recorded_and_evicted_as_before() {
    use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
    const GB: u64 = crate::admission::GB;
    static FREE: AtomicU64 = AtomicU64::new(60 * GB);
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        Some(FREE.load(SeqCst))
    }
    let f = start_cfg(2, |c| {
        c.same_path_roots = vec![c.ws_root.with_file_name("home")];
        c.disk_free = Some(free);
        c.evict = crate::evict::Settings {
            floor: 40 * GB,
            margin: 10 * GB,
            min_idle: Duration::ZERO,
            dry_run: false,
        };
    });
    let home = f.root.join("home");
    let idle_tree = home.join("trees/idle");
    let used_tree = home.join("trees/used");
    let (idle, used) = (idle_tree.join("target"), used_tree.join("target"));
    // What a new client sends: the directory by name, and that cargo
    // resolves it by itself.
    let job = |ws: &str, tree: &Path, script: &str| {
        let mut req = node_build(&f, ws, tree, Some(&tree.join("target")), script);
        if let Some(w) = &mut req.workspace {
            w.target_default = true;
        }
        req
    };
    // What cargo leaves in a target directory it made, and what the job
    // saw of the variable.
    let build = format!(
        "echo \"${{CARGO_TARGET_DIR-none}}\" > seen; mkdir -p target/debug; \
         echo '{}' > target/CACHEDIR.TAG; echo built > target/debug/blob",
        crate::evict::CACHEDIR_SIGNATURE
    );
    for (ws, tree) in [("a1", &idle_tree), ("b1", &used_tree)] {
        let r = run(&f, job(ws, tree, &build));
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(
            std::fs::read_to_string(tree.join("seen")).unwrap().trim(),
            "none",
            "no variable in the job"
        );
    }
    let recorded: Vec<Option<String>> = f
        .server
        .state()
        .done
        .iter()
        .map(|j| j.node_target_dir.clone())
        .collect();
    for t in [&idle, &used] {
        assert!(
            recorded.contains(&Some(t.to_string_lossy().into_owned())),
            "{recorded:?}"
        );
    }
    let known = || -> Vec<PathBuf> {
        let g = f.server.inner.lock().unwrap();
        let mut v: Vec<PathBuf> = g.targets.keys().cloned().collect();
        v.sort();
        v
    };
    assert_eq!(known(), [idle.clone(), used.clone()], "both are known");

    // Both jobs ended long ago; a job of the second tree runs.
    {
        let mut g = f.server.inner.lock().unwrap();
        g.targets.values_mut().for_each(|t| t.1 = 1);
        g.ran_in.values_mut().for_each(|at| *at = 1);
    }
    let go = f.root.join("go-used");
    let wait = format!("while [ ! -e {} ]; do sleep 0.02; done", go.display());
    let (c, id) = submit(&f, job("b1", &used_tree, &wait));
    wait_for_jobs(&f, "the job to run", || {
        state_of(&f, id) == JobState::Running
    });
    FREE.store(30 * GB, SeqCst);
    evict_view(put_evict_below(&f, r#"{"evict_below_gb":40}"#));
    wait_for_jobs(&f, "the look under the mark", || {
        !f.server.state().evict.last.is_empty()
    });
    let e = f.server.state().evict;
    assert!(!idle.exists(), "idle and under the mark: {e:?}");
    assert!(used.exists(), "a running job builds into it: {e:?}");
    assert_eq!(
        (e.last[0].path.as_str(), e.last[0].action.as_str()),
        (idle.to_str().unwrap(), "deleted"),
        "{e:?}"
    );
    std::fs::write(&go, "").unwrap();
    assert_eq!(collect(c).code, 0);
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

/// Records five ended runs of `sh -c echo light` in `repo`; the run at
/// index `compiled` used a compile server for 3 s, the one at index
/// `targeted` built into a target directory. Returns the command's request.
fn give_light_history(
    f: &Fixture,
    repo: &str,
    compiled: Option<usize>,
    targeted: Option<usize>,
) -> Request {
    let mut req = sh(&f.root, "echo light");
    req.repo = repo.into();
    let mut j = serde_json::from_str::<Job>(
        r#"{"id":1,"argv":[],"cwd":"/w","repo":"","toolchain":"other",
        "remote":false,"tokens":1,"state":"ok","enqueued_ms":1,"started_ms":0,
        "ended_ms":0,"exit":0,"cpu_s":1.0,"compile_server_cpu_s":0.0,"peak_rss":0}"#,
    )
    .unwrap();
    j.repo = repo.into();
    j.argv.clone_from(&req.argv);
    let now = now_ms();
    for i in 0..crate::admission::LIGHT_RUNS {
        j.started_ms = now - 10_000 * (5 - i as i64);
        j.ended_ms = j.started_ms + 2_000;
        j.compile_server_cpu_s = if compiled == Some(i) { 3.0 } else { 0.0 };
        j.target_dir = if targeted == Some(i) {
            f.root.join("target").to_string_lossy().into_owned()
        } else {
            String::new()
        };
        f.server.inner.lock().unwrap().memory.record(&j);
    }
    req
}

/// With 20 GB free, `--min-free-disk 40` and `--min-free-disk-light 10`: a
/// command whose last 5 runs compiled nothing and built into no target
/// directory starts; the same command in a repository where it never ran
/// waits at the build floor, and so does one with a compile or a target
/// directory in its last 5 runs.
#[test]
fn a_job_that_compiled_nothing_in_its_last_5_runs_starts_above_the_light_floor() {
    const GB: u64 = crate::admission::GB;
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        Some(20 * GB)
    }
    let f = start_cfg(2, |c| {
        c.disk_free = Some(free);
        c.admission.min_free_disk = 40 * GB;
        c.admission.min_free_disk_light = 10 * GB;
        c.resource_poll = Duration::from_millis(20);
    });
    let light = give_light_history(&f, "quiet", None, None);
    let compiled = give_light_history(&f, "built-once", Some(4), None);
    let targeted = give_light_history(&f, "targeted-once", None, Some(0));
    let mut first = sh(&f.root, "echo light");
    first.repo = "never-ran".into();

    let mut held = Vec::new();
    for req in [first, compiled, targeted] {
        let (c, id) = submit(&f, req);
        wait_for_jobs(&f, "the job to be held at the build floor", || {
            reason_of(&f, id).contains("builds need 40.0 GB")
        });
        held.push((c, id));
    }
    let (c, id) = submit(&f, light);
    // A held job sends nothing more: the read ends after 5 s.
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let r = collect(c);
    assert_eq!(
        (r.code, r.out.as_str()),
        (0, "light\n"),
        "job {id}: {r:?}, reason {:?}",
        reason_of(&f, id)
    );
    assert!(!r.info.contains("waiting"), "started at once: {r:?}");
    std::thread::sleep(Duration::from_millis(100));
    for (_, id) in &held {
        assert_eq!(state_of(&f, *id), JobState::Queued, "job {id}");
    }
    drop(held);
}

/// Status and text of the answer to `PUT /api/evict-below` with `body`.
fn put_evict_below(f: &Fixture, body: &str) -> (u16, String) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        "application/json".parse().unwrap(),
    );
    let state = axum::extract::State(Arc::clone(&f.server));
    let body = axum::body::Bytes::from(body.to_owned());
    answer(http::evict_below_put(state, headers, body))
}

/// `PUT /api/evict-below`, the call `slot evict-below` makes, turns
/// eviction on at run time over `--evict-below 0`: the watch looks at once
/// at the new mark and deletes, the mark is saved beside the headroom and
/// in force in the daemon that starts next, and `null` forgets it. The disk
/// hold keeps its own mark. A negative or too large mark is refused.
#[test]
fn the_eviction_mark_is_set_at_run_time_saved_and_looked_at_once() {
    const GB: u64 = crate::admission::GB;
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        Some(45 * GB)
    }
    let f = start_cfg(2, |c| {
        c.disk_free = Some(free);
        c.admission.min_free_disk = 25 * GB;
        c.evict = crate::evict::Settings {
            floor: 0,
            margin: 10 * GB,
            min_idle: Duration::ZERO,
            dry_run: false,
        };
    });
    let target = old_target(&f, "old");
    let view = |(status, text): (u16, String)| {
        assert_eq!(status, 200, "{text}");
        serde_json::from_str::<crate::evict::BelowView>(&text).unwrap()
    };
    let get = || {
        use axum::response::IntoResponse;
        let state = axum::extract::State(Arc::clone(&f.server));
        view(answer(async move {
            http::evict_below_get(state).await.into_response()
        }))
    };
    assert_eq!((get().evict_below_gb, get().floor), (None, 0));

    for bad in [
        "{}",
        r#"{"evict_below_gb":-1}"#,
        r#"{"evict_below_gb":100001}"#,
        "",
    ] {
        let (status, text) = put_evict_below(&f, bad);
        assert_eq!(status, 400, "{bad}: {text}");
        assert!(text.starts_with("slot: "), "{bad}: {text}");
    }
    assert_eq!(get().evict_below_gb, None, "a refused body changed nothing");

    let v = view(put_evict_below(&f, r#"{"evict_below_gb":50}"#));
    assert_eq!((v.evict_below_gb, v.floor), (Some(50.0), 50 * GB));
    wait_for_jobs(&f, "the look at the new mark", || !target.exists());
    let e = f.server.state().evict;
    assert_eq!((e.floor, e.raised_from), (50 * GB, 0), "{e:?}");
    assert_eq!(e.last.len(), 1, "{e:?}");
    let saved = std::fs::read_to_string(f.root.join("state").join(limits::FILE)).unwrap();
    assert!(saved.contains(r#""evict_below_gb":50.0"#), "{saved}");
    let next = Server::new(Config {
        socket: f.root.join("s2"),
        evict: crate::evict::Settings {
            floor: 0,
            margin: 10 * GB,
            min_idle: Duration::ZERO,
            dry_run: false,
        },
        ..config(&f.root, 2)
    })
    .unwrap();
    assert_eq!(next.evict_below_view().evict_below_gb, Some(50.0));

    let v = view(put_evict_below(&f, r#"{"evict_below_gb":null}"#));
    assert_eq!(
        (v.evict_below_gb, v.floor),
        (None, 0),
        "the option again: off"
    );
    wait_for_jobs(&f, "eviction off again", || {
        f.server.state().evict.floor == 0
    });
    let saved = std::fs::read_to_string(f.root.join("state").join(limits::FILE)).unwrap();
    assert!(!saved.contains("evict_below_gb"), "{saved}");
}

/// The view a `PUT /api/evict-below` answers with.
fn evict_view((status, text): (u16, String)) -> crate::evict::BelowView {
    assert_eq!(status, 200, "{text}");
    serde_json::from_str(&text).unwrap()
}

/// `PUT /api/evict-below` with `evict_idle_s` replaces the 2 h idle keep at
/// run time: a target directory built 40 min ago is kept at 2 h and
/// deleted at the next look after `{"evict_idle_s":1800}`. The mark is left
/// as it is, the keep is saved, and `null` forgets it.
#[test]
fn the_idle_keep_is_set_at_run_time_and_applies_at_the_next_look() {
    const GB: u64 = crate::admission::GB;
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        Some(45 * GB)
    }
    let f = start_cfg(2, |c| {
        c.disk_free = Some(free);
        c.admission.min_free_disk = 25 * GB;
        c.evict = crate::evict::Settings {
            floor: 0,
            margin: 10 * GB,
            min_idle: crate::evict::DEFAULT_MIN_IDLE,
            dry_run: false,
        };
    });
    let target = old_target(&f, "recent");
    f.server
        .inner
        .lock()
        .unwrap()
        .targets
        .insert(target.clone(), ("recent".into(), now_ms() - 90 * 60_000));
    let v = evict_view(put_evict_below(&f, r#"{"evict_below_gb":50}"#));
    assert_eq!((v.evict_idle_s, v.idle_s), (None, 7200), "the default");
    wait_for_jobs(&f, "the look at the new mark", || {
        f.server.state().evict.checked_ms > 0
    });
    assert!(target.exists(), "built 90 min ago: kept at 2 h");

    let v = evict_view(put_evict_below(&f, r#"{"evict_idle_s":1800}"#));
    assert_eq!((v.evict_idle_s, v.idle_s), (Some(1800), 1800));
    assert_eq!(v.evict_below_gb, Some(50.0), "the mark is left as it is");
    wait_for_jobs(&f, "the look at the new idle keep", || !target.exists());
    let saved = std::fs::read_to_string(f.root.join("state").join(limits::FILE)).unwrap();
    assert!(saved.contains(r#""evict_idle_s":1800"#), "{saved}");

    let (status, text) = put_evict_below(&f, r#"{"evict_idle_s":60}"#);
    assert_eq!(status, 400, "{text}");
    assert!(text.starts_with("slot: the idle keep"), "{text}");
    let v = evict_view(put_evict_below(&f, r#"{"evict_idle_s":null}"#));
    assert_eq!((v.evict_idle_s, v.idle_s), (None, 7200), "forgotten");
    assert_eq!(v.evict_below_gb, Some(50.0));
    let saved = std::fs::read_to_string(f.root.join("state").join(limits::FILE)).unwrap();
    assert!(!saved.contains("evict_idle_s"), "{saved}");
}

/// A target directory whose removal fails is left out of the next pass:
/// the pass plans and deletes the next candidate instead of failing on the
/// same one every minute.
#[test]
fn a_target_dir_whose_removal_failed_is_skipped_and_the_next_one_deleted() {
    use std::os::unix::fs::PermissionsExt as _;
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        Some(1000)
    }
    let f = start_cfg(2, |c| {
        c.disk_free = Some(free);
        c.admission.min_free_disk = 0;
        c.evict = crate::evict::Settings {
            floor: 0,
            margin: 0,
            min_idle: Duration::ZERO,
            dry_run: false,
        };
    });
    let stuck = old_target(&f, "stuck");
    let next = old_target(&f, "next");
    for (t, last) in [(&stuck, 1), (&next, 2)] {
        std::fs::write(t.join("blob"), vec![1u8; 1 << 16]).unwrap();
        let repo = t.parent().unwrap().file_name().unwrap();
        f.server
            .inner
            .lock()
            .unwrap()
            .targets
            .insert(t.clone(), (repo.to_string_lossy().into_owned(), last));
    }
    let mode = |m| std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o555);
    if std::fs::write(stuck.join("probe"), "").is_ok() {
        // Run as root: a read-only directory does not stop a removal.
        mode(0o755);
        return;
    }
    // 5000 bytes: the oldest directory alone would free enough.
    let body = r#"{"evict_below_gb":0.000005}"#;
    evict_view(put_evict_below(&f, body));
    wait_for_jobs(&f, "the failed removal", || {
        f.server
            .state()
            .evict
            .last
            .iter()
            .any(|e| e.action.starts_with("failed"))
    });
    let e = f.server.state().evict;
    assert_eq!(e.last.len(), 1, "{e:?}");
    assert_eq!(e.last[0].path, stuck.to_string_lossy(), "{e:?}");
    assert!(next.exists());

    evict_view(put_evict_below(&f, body));
    let next_text = next.to_string_lossy().into_owned();
    wait_for_jobs(&f, "the next candidate deleted", || {
        f.server
            .state()
            .evict
            .last
            .iter()
            .any(|e| e.path == next_text)
    });
    mode(0o755);
    assert!(stuck.exists() && !next.exists());
    let e = f.server.state().evict;
    assert_eq!(
        e.last.len(),
        1,
        "the failed one is not planned again: {e:?}"
    );
    assert_eq!(e.last[0].action, "deleted", "{e:?}");
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

// wave w1 u3

/// A handover asks each client writer for its unsent bytes after the output
/// pumps stopped. Frames a pump put into the writer's channel just before
/// that are unsent too: the bytes the client read plus the bytes handed on
/// hold every frame once, in order.
#[test]
fn a_handover_passes_on_the_frames_still_in_a_writers_channel() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (read, handed_on) = rt.block_on(async {
        let (ours, theirs) = tokio::net::UnixStream::pair().unwrap();
        let (_rd, wr) = ours.into_split();
        let (frames, rx) = mpsc::unbounded_channel();
        let (ctl, ctl_rx) = mpsc::unbounded_channel();
        let mut unsent = serde_json::to_vec(&Frame::Out {
            b: "unsent-before\n".into(),
        })
        .unwrap();
        unsent.push(b'\n');
        for i in 0..100 {
            frames
                .send(Frame::Out {
                    b: format!("line{i}\n"),
                })
                .unwrap();
        }
        let (reply, answer) = oneshot::channel();
        ctl.send(reply).unwrap();
        let writer = tokio::spawn(client_writer(wr, rx, ctl_rx, unsent));
        let handed_on = tokio::time::timeout(Duration::from_secs(10), answer)
            .await
            .expect("the writer answers within 10 s")
            .expect("the writer answers");
        writer.abort();
        let mut read = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match theirs.try_read(&mut buf) {
                Ok(0) => break,
                Ok(n) => read.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => panic!("read: {e}"),
            }
        }
        (read, handed_on)
    });
    let mut out = String::new();
    for line in [read, handed_on].concat().split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Frame>(line).unwrap() {
            Frame::Out { b } => out += &b,
            other => panic!("unexpected frame {other:?}"),
        }
    }
    let want: String = std::iter::once("unsent-before\n".to_string())
        .chain((0..100).map(|i| format!("line{i}\n")))
        .collect();
    assert_eq!(out, want, "a frame was lost or doubled");
}

#[test]
fn a_cpu_sample_covers_the_time_between_two_scans_not_two_ticks() {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut scans = ScanInterval(at(100));
    // Ticks at 1 s and 2 s; the scan of the first ran 0.9 s late.
    let late = scans.next(at(2_000));
    assert!((late - 1.9).abs() < 1e-9, "{late}");
    let on_time = scans.next(at(2_100));
    assert!((on_time - 0.1).abs() < 1e-9, "{on_time}");
    // A scan that failed did not call `next`: the sample after it covers both seconds.
    let after_a_failed_scan = scans.next(at(4_100));
    assert!(
        (after_a_failed_scan - 2.0).abs() < 1e-9,
        "{after_a_failed_scan}"
    );
    // A clock that reads earlier than the last scan gives 0, never a negative interval.
    assert_eq!(scans.next(at(4_000)), 0.0);
}

// Admission by share

/// A CPU pressure of 0 %: no brake.
fn no_pressure() -> f64 {
    0.0
}

/// Declares, inside one test, the CPUs its daemon reads as busy in the
/// pool in hundredths of a CPU, and the function that reads it, like
/// [`host_busy!`] with a finer figure.
macro_rules! pool_busy {
    ($cell:ident, $read:ident, $hundredths:expr) => {
        static $cell: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new($hundredths);
        fn $read() -> Option<cpu_limit::Parts> {
            let v = $cell.load(std::sync::atomic::Ordering::SeqCst);
            Some(cpu_limit::Parts {
                pool: v as f64 / 100.0,
                ..cpu_limit::Parts::default()
            })
        }
    };
}

/// A daemon of `tokens` slots on a host of `cpus` CPUs whose busy figure
/// is `busy`, without CPU pressure, whose queue looks again every 20 ms.
fn start_shares(
    tokens: usize,
    cpus: u32,
    busy: fn() -> Option<cpu_limit::Parts>,
    change: impl FnOnce(&mut Config),
) -> Fixture {
    start_cfg(tokens, |c| {
        c.cpu_busy = Some(busy);
        c.host_cpus = Some(cpus);
        c.cpu_pressure = Some(no_pressure);
        c.resource_poll = Duration::from_millis(20);
        change(c);
    })
}

/// Switches admission to shares with `floor` and `max`.
fn by_share(f: &Fixture, floor: usize, max: Option<usize>) -> crate::shares::View {
    f.server
        .set_admission(&crate::shares::Request {
            admission: Some(crate::shares::Mode::Shares),
            floor: Some(floor),
            max,
            pressure_max: None,
            min_budget: None,
        })
        .unwrap()
}

/// Whether job `id` claimed its start (or ended) within `within`.
fn claimed_within(f: &Fixture, id: i64, within: Duration) -> Option<Duration> {
    let t0 = Instant::now();
    loop {
        let claimed = {
            let g = f.server.inner.lock().unwrap();
            g.active.get(&id).is_none_or(|l| l.claimed)
        };
        if claimed {
            return Some(t0.elapsed());
        }
        if t0.elapsed() >= within {
            return None;
        }
        // The poll interval of a bounded wait.
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits until the daemon has looked at admission by share `n` more times
/// (once a sampler tick).
fn after_looks(f: &Fixture, n: usize) {
    let last = || f.server.inner.lock().unwrap().share_last_ms;
    let mut seen = last();
    for _ in 0..n {
        wait_for_jobs(f, "the next look of admission by share", || {
            let now = last();
            now.is_some() && now != seen
        });
        seen = last();
    }
}

/// The ids of the jobs that claimed their start, oldest first: a job
/// counts from its claim, before its process runs.
fn claimed(f: &Fixture) -> Vec<i64> {
    let g = f.server.inner.lock().unwrap();
    let mut ids: Vec<i64> = g
        .active
        .iter()
        .filter(|(_, l)| l.claimed)
        .map(|(id, _)| *id)
        .collect();
    ids.sort_unstable();
    ids
}

/// Samples the sampler has taken of running job `id`.
fn samples_of(f: &Fixture, id: i64) -> usize {
    let g = f.server.inner.lock().unwrap();
    g.active.get(&id).map_or(0, |l| l.recent.len())
}

/// Gives the command `sleep 60` of `repo` three earlier runs that used
/// `cores` CPUs each, so its share is predicted, and returns its request.
fn give_share_history(f: &Fixture, repo: &str, cores: f64) -> Request {
    assert_eq!(run(f, sh(&f.root, "true")).code, 0);
    let mut j = f.server.state().done[0].clone();
    let mut req = sh(&f.root, "sleep 60");
    req.repo = repo.into();
    let now = now_ms();
    j.repo = repo.into();
    j.argv.clone_from(&req.argv);
    j.state = JobState::Ok;
    for i in 0..3 {
        j.ended_ms = now - 1000 * (3 - i);
        j.started_ms = j.ended_ms - 10_000;
        j.cpu_s = cores * 10.0;
        f.server.inner.lock().unwrap().memory.record(&j);
    }
    req
}

/// Sets `cell` to `per_job` hundredths of a CPU for each job that claimed
/// its start, every 10 ms, until the returned flag is set: the census of a
/// host whose jobs each use `per_job`.
fn busy_follows_jobs(
    f: &Fixture,
    cell: &'static std::sync::atomic::AtomicI64,
    per_job: i64,
) -> Arc<std::sync::atomic::AtomicBool> {
    use std::sync::atomic::Ordering::SeqCst;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (server, done) = (Arc::clone(&f.server), Arc::clone(&stop));
    std::thread::spawn(move || {
        while !done.load(SeqCst) {
            let n = {
                let g = server.inner.lock().unwrap();
                g.active.values().filter(|l| l.claimed).count()
            };
            cell.store(per_job * n as i64, SeqCst);
            // The census of this test follows its jobs within 10 ms.
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    stop
}

/// By slots (the default) nothing changes: with 2 slots two jobs that
/// sleep hold both, a third waits however many times the sampler looks,
/// and no job and no state carries a share.
#[test]
fn by_slots_two_sleeping_jobs_hold_both_slots_and_a_third_waits() {
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(2, 3, busy, |_| {});
    let (a, ja) = quiet(&f);
    let (b, jb) = quiet(&f);
    wait_for_jobs(&f, "two jobs to run", || running(&f) == vec![ja, jb]);
    let (c, jc) = queued_job(&f, sh(&f.root, "echo third"));
    wait_for_jobs(&f, "the sampler to look 3 times at a sleeping job", || {
        samples_of(&f, ja) >= 3
    });
    assert_eq!(state_of(&f, jc), JobState::Queued);
    assert_eq!(running(&f), vec![ja, jb]);
    let st = f.server.state();
    assert_eq!(st.shares, None);
    assert!(st.active.iter().all(|j| j.share.is_none()), "{st:?}");
    assert_eq!(
        f.server.admission_view().admission,
        crate::shares::Mode::Slots
    );
    assert_eq!(f.server.pool().limit(), 2);
    end_all(&f, vec![a, b, c]);
}

/// By share, with a budget of 2.25 CPUs (a host of 3, pool of 2 slots)
/// and a floor of 1: two jobs that sleep start and a third waits; once
/// the sampler measures the two near 0 CPUs they count with what they use
/// and the third starts within 5 s while both still run. `slot status`
/// shows the lent share; the jobs' records the CPU-seconds they lent.
/// When the lenders' CPU comes back no further job starts until the busy
/// CPUs leave room for its share; a job that ends gives that room.
#[test]
fn by_share_jobs_that_sleep_lend_their_share_and_take_it_back_when_they_use_cpu() {
    use std::sync::atomic::Ordering::SeqCst;
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(2, 3, busy, |_| {});
    let view = by_share(&f, 1, None);
    assert_eq!((view.budget, view.cap, view.free), (2.25, 3.0, 2.25));
    assert_eq!(f.server.pool().limit(), 2, "the FIFO follows the budget");
    let (first, ja) = quiet(&f);
    let (second, jb) = quiet(&f);
    wait_for_jobs(&f, "two jobs to run", || running(&f) == vec![ja, jb]);
    let t0 = Instant::now();
    let (third, jc) = submit(&f, sh(&f.root, "sleep 60"));
    let took = claimed_within(&f, jc, Duration::from_secs(5));
    assert!(took.is_some(), "the third job did not start within 5 s");
    eprintln!("third job started {:?} after it was sent", t0.elapsed());
    wait_for_jobs(&f, "the third job to run beside the first two", || {
        running(&f) == vec![ja, jb, jc]
    });
    wait_for_jobs(&f, "the two sleeping jobs to lend their shares", || {
        let v = f.server.admission_view();
        v.idle >= 2 && v.lent >= 1.9
    });
    let shown = crate::status::render(&f.server.state());
    assert!(shown.contains(" idle lend "), "{shown}");
    let job = active_job(&f, ja).unwrap();
    assert_eq!(job.share, Some(1.0));
    assert!(job.share_used.is_some_and(|u| u < 0.1), "{job:?}");

    // The lenders use their CPU again: the census reads 2.5 busy.
    BUSY.store(250, SeqCst);
    after_looks(&f, 1);
    let (fourth, jd) = queued_job(&f, sh(&f.root, "echo fourth"));
    wait_for_jobs(&f, "the fourth job to wait for its share", || {
        reason_of(&f, jd).starts_with("waits for a share of 1.0 CPUs: 0.0 of a budget of")
    });
    after_looks(&f, 2);
    assert_eq!(state_of(&f, jd), JobState::Queued, "{}", reason_of(&f, jd));

    // The first job ends: its CPU leaves the census and the fourth starts.
    drop(first);
    wait_for_jobs(&f, "the first job to end", || active_job(&f, ja).is_none());
    BUSY.store(100, SeqCst);
    let res = collect(fourth);
    assert_eq!((res.code, res.out.as_str()), (0, "fourth\n"), "{res:?}");
    let lent = done_job(&f, ja).lent_s;
    assert!(lent > 0.5, "the first job lent {lent} CPU-seconds");
    end_all(&f, vec![second, third]);
}

/// The most jobs by share bounds the loans: with a most of 2 a third job
/// waits while two sleep, whatever share is free.
#[test]
fn by_share_the_most_jobs_bound_what_idle_jobs_lend() {
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(20, 20, busy, |_| {});
    by_share(&f, 1, Some(2));
    let (a, ja) = quiet(&f);
    let (b, jb) = quiet(&f);
    wait_for_jobs(&f, "two jobs to run", || running(&f) == vec![ja, jb]);
    let (c, jc) = queued_job(&f, sh(&f.root, "echo third"));
    wait_for_jobs(&f, "the third job to wait for the most", || {
        reason_of(&f, jc) == "2 jobs run, the most by share"
    });
    after_looks(&f, 2);
    assert_eq!(state_of(&f, jc), JobState::Queued);
    assert!(f.server.admission_view().free > 10.0);
    end_all(&f, vec![a, b, c]);
}

/// Jobs that each use 0.25 CPUs: on a host of 2 CPUs more of them run at
/// once than it has CPUs, up to the most (5).
#[test]
fn by_share_jobs_of_a_quarter_cpu_run_more_than_the_host_has_cpus() {
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(2, 2, busy, |_| {});
    let req = give_share_history(&f, "quarter", 0.2);
    let stop = busy_follows_jobs(&f, &BUSY, 25);
    by_share(&f, 1, Some(5));
    let mut clients = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..7 {
        let (c, id) = submit(&f, req.clone());
        clients.push(c);
        ids.push(id);
    }
    wait_for_jobs(&f, "five jobs to run", || running(&f).len() == 5);
    wait_for_jobs(&f, "the rest to wait for the most", || {
        reason_of(&f, ids[6]) == "5 jobs run, the most by share"
    });
    assert_eq!(active_job(&f, ids[0]).unwrap().share, Some(0.23));
    after_looks(&f, 2);
    assert_eq!(claimed(&f).len(), 5, "more than the 2 CPUs, at most 5");
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    end_all(&f, clients);
}

/// Jobs that each use 4 CPUs: on a host of 20 CPUs (budget 15) three run,
/// fewer than a quarter of the CPUs, and the fourth waits for its share.
#[test]
fn by_share_jobs_of_four_cpus_run_fewer_than_a_quarter_of_the_cpus() {
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(20, 20, busy, |_| {});
    let req = give_share_history(&f, "four", 3.5);
    let stop = busy_follows_jobs(&f, &BUSY, 400);
    by_share(&f, 1, None);
    let mut clients = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..5 {
        let (c, id) = submit(&f, req.clone());
        clients.push(c);
        ids.push(id);
    }
    wait_for_jobs(&f, "the fourth job to wait for its share", || {
        reason_of(&f, ids[3]) == "waits for a share of 4.0 CPUs: 3.0 of a budget of 15.0 free"
    });
    assert_eq!(claimed(&f), ids[..3].to_vec(), "three let through");
    after_looks(&f, 2);
    assert_eq!(claimed(&f).len(), 3, "fewer than 5, a quarter of 20 CPUs");
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    end_all(&f, clients);
}

/// A queued job does not start while the free share is under its share
/// (0.9 of 1.0) over several looks, and starts within one tick once the
/// free share reaches it (1.1).
#[test]
fn by_share_a_job_starts_within_one_tick_of_its_share_coming_free_and_not_before() {
    use std::sync::atomic::Ordering::SeqCst;
    pool_busy!(BUSY, busy, 1410);
    let f = start_shares(20, 20, busy, |_| {});
    by_share(&f, 1, None);
    let (a, ja) = quiet(&f);
    wait_for_jobs(&f, "the floor job to run", || running(&f) == vec![ja]);
    let (b, jb) = queued_job(&f, sh(&f.root, "echo second"));
    wait_for_jobs(&f, "the second job to wait at 0.9 free", || {
        reason_of(&f, jb) == "waits for a share of 1.0 CPUs: 0.9 of a budget of 15.0 free"
    });
    after_looks(&f, 3);
    assert_eq!(state_of(&f, jb), JobState::Queued);
    let t0 = Instant::now();
    BUSY.store(1390, SeqCst);
    let took = claimed_within(&f, jb, Duration::from_millis(2500));
    eprintln!("started {:?} after its share came free", t0.elapsed());
    assert!(took.is_some(), "not started within one tick");
    let res = collect(b);
    assert_eq!((res.code, res.out.as_str()), (0, "second\n"), "{res:?}");
    end_all(&f, vec![a]);
}

/// A client of 0.21.0 or older asks for slots: `--tokens 3` is read as a
/// share of 3 CPUs, and the job runs. It holds no slot of the FIFO: the
/// FIFO's tokens are left for the build processes inside the jobs.
#[test]
fn by_share_a_request_for_slots_is_read_as_a_share() {
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(20, 20, busy, |_| {});
    by_share(&f, 1, None);
    let (a, ja) = quiet(&f);
    let mut r = sh(&f.root, "sleep 60");
    r.tokens = Some(3);
    let (b, jb) = submit(&f, r);
    wait_for_jobs(&f, "both jobs to run", || running(&f) == vec![ja, jb]);
    assert_eq!(active_job(&f, jb).unwrap().share, Some(3.0));
    let held = f.server.inner.lock().unwrap().active[&jb].held;
    assert_eq!(held, 0, "a job by share holds no slot of the FIFO");
    assert_eq!(f.server.pool().free(), 15);
    end_all(&f, vec![a, b]);
}

/// The budget never passes the pool's CPUs less the headroom: with 50 %
/// on 20 CPUs it is 10, and the FIFO carries 10 tokens.
#[test]
fn by_share_the_budget_stays_inside_the_pools_cpus() {
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(20, 20, busy, |_| {});
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(Arc::clone(&f.server).set_headroom(50));
    let v = by_share(&f, 1, None);
    assert_eq!((v.cap, v.budget), (10.0, 10.0));
    assert_eq!(f.server.pool().limit(), 10);
    let shown = f
        .server
        .state()
        .shares
        .expect("the state carries the shares");
    assert_eq!(shown.budget, 10.0);
}

/// By share the FIFO carries as many tokens as the budget has CPUs (15 of
/// 20), and back by slots the pool's 20; no token is lost on the way.
#[test]
fn the_fifo_follows_the_budget_and_goes_back_to_the_pool_by_slots() {
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(20, 20, busy, |_| {});
    let (a, _) = quiet(&f);
    by_share(&f, 1, None);
    assert_eq!(f.server.pool().limit(), 15);
    after_looks(&f, 1);
    assert!(
        f.server
            .metrics_text()
            .contains("slot_tokens_unaccounted 0\n"),
        "{}",
        f.server.metrics_text()
    );
    let v = f
        .server
        .set_admission(&crate::shares::Request {
            admission: Some(crate::shares::Mode::Slots),
            ..crate::shares::Request::default()
        })
        .unwrap();
    assert_eq!(v.admission, crate::shares::Mode::Slots);
    assert_eq!(f.server.pool().limit(), 20);
    assert_eq!(f.server.state().shares, None);
    assert!(
        f.server
            .metrics_text()
            .contains("slot_tokens_unaccounted 0\n"),
        "{}",
        f.server.metrics_text()
    );
    end_all(&f, vec![a]);
}

/// Over the CPU pressure mark only a job predicted short starts; a longer
/// one waits and is told why.
#[test]
fn by_share_over_the_pressure_mark_only_short_jobs_start() {
    fn pressing() -> f64 {
        80.0
    }
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(20, 20, busy, |c| c.cpu_pressure = Some(pressing));
    give_history(&f, "brake");
    f.server
        .set_admission(&crate::shares::Request {
            admission: Some(crate::shares::Mode::Shares),
            floor: Some(1),
            max: None,
            pressure_max: Some(60.0),
            min_budget: None,
        })
        .unwrap();
    let (a, ja) = quiet(&f);
    wait_for_jobs(&f, "the floor job to run", || running(&f) == vec![ja]);
    let (long, jl) = queued_job(&f, named(&f, LONG_CMD, "brake-long", "long", None));
    wait_for_jobs(&f, "the longer job to wait for the pressure", || {
        reason_of(&f, jl) == "CPU pressure 80 % over 60 %: only short jobs start"
    });
    let (short, js) = submit(&f, named(&f, SHORT_CMD, "brake", "short", None));
    assert!(
        claimed_within(&f, js, Duration::from_secs(3)).is_some(),
        "the short job did not start"
    );
    assert_eq!(state_of(&f, jl), JobState::Queued);
    assert!(f.server.admission_view().braking);
    go(&f, "short");
    assert_eq!(collect(short).code, 0);
    end_all(&f, vec![a, long]);
}

/// Status and text of the answer to `PUT /api/admission`.
fn put_admission(f: &Fixture, content_type: &str, body: &str) -> (u16, String) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        content_type.parse().unwrap(),
    );
    let state = axum::extract::State(Arc::clone(&f.server));
    let body = axum::body::Bytes::from(body.to_owned());
    answer(http::admission_put(state, headers, body))
}

/// Status and text of the answer to `GET /api/admission`.
fn get_admission(f: &Fixture) -> (u16, String) {
    use axum::response::IntoResponse;
    let state = axum::extract::State(Arc::clone(&f.server));
    answer(async move { http::admission_get(state).await.into_response() })
}

/// Admission set through `PUT /api/admission`, the call `slot admission`
/// makes, is read back by `GET`, saved beside the headroom, and in force
/// in the daemon that starts next; back by slots it leaves the file.
/// Bounds that do not fit are refused.
#[test]
fn the_admission_is_set_saved_and_read_back() {
    use crate::shares::Mode;
    let f = start_cfg(2, |c| c.host_cpus = Some(20));
    let json = "application/json";
    let view = |(status, text): (u16, String)| {
        assert_eq!(status, 200, "{text}");
        let v: crate::shares::View = serde_json::from_str(&text).unwrap();
        (v.admission, v.floor, v.max, v.pressure_max)
    };
    let saved = || std::fs::read_to_string(f.root.join("state").join(limits::FILE)).unwrap();
    let restarted = |socket: &str| {
        let next = Server::new(Config {
            socket: f.root.join(socket),
            host_cpus: Some(20),
            ..config(&f.root, 2)
        })
        .unwrap();
        let v = next.admission_view();
        (v.admission, v.floor, v.max)
    };
    assert_eq!(view(get_admission(&f)), (Mode::Slots, 4, 48, 60.0));
    assert_eq!(
        view(put_admission(
            &f,
            json,
            r#"{"admission":"shares","floor":2,"max":30}"#
        )),
        (Mode::Shares, 2, 30, 60.0)
    );
    assert_eq!(view(get_admission(&f)), (Mode::Shares, 2, 30, 60.0));
    assert!(
        saved().contains(r#""admission":"shares","share_floor":2,"share_max":30"#),
        "{}",
        saved()
    );
    assert_eq!(restarted("s2"), (Mode::Shares, 2, 30));
    for bad in [
        "{}",
        r#"{"floor":0}"#,
        r#"{"max":0}"#,
        r#"{"floor":40}"#,
        r#"{"pressure_max":0}"#,
        r#"{"pressure_max":120}"#,
        r#"{"admission":"tokens"}"#,
        "not json",
    ] {
        let (status, text) = put_admission(&f, json, bad);
        assert_eq!(status, 400, "{bad}: {text}");
        assert!(text.starts_with("slot: "), "{bad}: {text}");
        assert_eq!(view(get_admission(&f)), (Mode::Shares, 2, 30, 60.0));
    }
    let (status, _) = put_admission(&f, "text/plain", r#"{"admission":"slots"}"#);
    assert_eq!(status, 415);
    assert_eq!(
        view(put_admission(&f, json, r#"{"admission":"slots"}"#)),
        (Mode::Slots, 2, 30, 60.0)
    );
    assert!(!saved().contains("\"admission\""), "{}", saved());
    assert_eq!(restarted("s3").0, Mode::Slots);
}

// adversary u6

/// A request for 16 slots on a host of 20 CPUs: by slots it starts beside
/// four sleeping jobs (16 of 20 slots free). By share it is read as a
/// share of 16 CPUs while the budget is 15, so it never fits the free
/// share, although the host is idle; it starts only once fewer than the
/// floor (4) run. The client is even told its request was capped at the
/// 15 slots in circulation.
#[test]
fn adversary_u6_a_request_for_more_slots_than_the_budget_starts_on_an_idle_host() {
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(20, 20, busy, |_| {});
    by_share(&f, crate::shares::DEFAULT_FLOOR, None);
    let mut clients = Vec::new();
    for _ in 0..4 {
        clients.push(quiet(&f).0);
    }
    wait_for_jobs(&f, "four sleeping jobs to run", || running(&f).len() == 4);
    let mut wide = sh(&f.root, "sleep 60");
    wide.tokens = Some(16);
    let (c, id) = submit(&f, wide);
    clients.push(c);
    let started = claimed_within(&f, id, Duration::from_millis(3000));
    assert!(
        started.is_some(),
        "a request for 16 slots waits on an idle host: share {:?}, {}; {:?}",
        active_job(&f, id).and_then(|j| j.share),
        reason_of(&f, id),
        f.server.admission_view()
    );
    end_all(&f, clients);
}

/// After a reload the jobs that run are in the table with no samples and
/// their old start (`adopt_running`), and the first look of the sampler
/// has no busy figure (`primed` is false, so `host_busy` is empty and the
/// figure is unknown). Three jobs of 4 CPUs run on a budget of 15; a
/// fourth waits. In that state the running jobs count with nothing and
/// the unknown figure with 0 CPUs: the whole budget is free and the
/// fourth job is let through beside the 12 CPUs the three asked for.
#[test]
fn adversary_u6_an_unknown_busy_figure_does_not_free_the_budget_of_the_running_jobs() {
    fn unknown() -> Option<cpu_limit::Parts> {
        None
    }
    let f = start_shares(20, 20, unknown, |_| {});
    let req = give_share_history(&f, "four", 3.5);
    // The most of 3 holds the fourth job in the queue until the look
    // below, so the sleeping jobs lending their share cannot let it in
    // first.
    by_share(&f, 1, Some(3));
    let mut clients = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let (c, id) = submit(&f, req.clone());
        clients.push(c);
        ids.push(id);
    }
    wait_for_jobs(&f, "three jobs of 4 CPUs to run", || running(&f) == ids);
    let (c, fourth) = queued_job(&f, req.clone());
    clients.push(c);
    let granted = {
        let mut g = f.server.inner.lock().unwrap();
        f.server.shares.lock().unwrap().max = None;
        for id in &ids {
            let l = g.active.get_mut(id).unwrap();
            // As `adopt_running` puts a job that has run for a minute.
            l.job.started_ms -= 60_000;
            l.recent = cpu_limit::Recent::default();
        }
        f.server.share_step(&mut g, now_ms());
        g.active[&fourth]
            .waiter
            .as_ref()
            .is_some_and(WaiterHandle::granted)
    };
    let view = f.server.admission_view();
    assert!(
        !granted,
        "let through beside three running jobs of 4 CPUs with the busy figure unknown: {view:?}"
    );
    end_all(&f, clients);
}

/// A daemon whose `limits.json` says shares, started (or re-executed by a
/// reload) on a host whose pool is 20 of 20 CPUs busy, with a floor of
/// one job. `run` starts the admission of the queued jobs a handover passed on
/// and then the sampler, whose first look is the first that hands the
/// pool a room for shares. Until then the pool admits by its tokens: the
/// FIFO is full (jobs by share hold none), so every queued job starts,
/// not the one the floor lets through.
#[test]
fn adversary_u6_a_daemon_that_starts_by_share_admits_no_job_by_slots_before_its_first_look() {
    pool_busy!(BUSY, busy, 2000);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut cfg = config(&root, 20);
    cfg.cpu_busy = Some(busy);
    cfg.host_cpus = Some(20);
    cfg.cpu_pressure = Some(no_pressure);
    cfg.resource_poll = Duration::from_millis(20);
    std::fs::create_dir_all(&cfg.state_dir).unwrap();
    std::fs::write(
        cfg.state_dir.join(limits::FILE),
        r#"{"headroom_percent":0,"admission":"shares","share_floor":1}"#,
    )
    .unwrap();
    let server = Server::new(cfg).unwrap();
    assert_eq!(
        server.admission_view().admission,
        crate::shares::Mode::Shares
    );
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut gone = Vec::new();
    // What `run` does after a handover: each queued job's admission is
    // spawned, then the sampler.
    for _ in 0..3 {
        let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
        let (g, gone_rx) = watch::channel(false);
        gone.push(g);
        let mut req = sh(&root, "sleep 30");
        req.env = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        let s = Arc::clone(&server);
        rt.spawn(async move {
            let _ = s.run_job(req, tx, gone_rx, None, None, None).await;
        });
        rt.spawn(async move { while rx.recv().await.is_some() {} });
    }
    rt.spawn(Arc::clone(&server).sampler());
    let started = || {
        let g = server.inner.lock().unwrap();
        g.active.values().filter(|l| l.claimed).count()
    };
    let t0 = Instant::now();
    while started() < 3 && t0.elapsed() < Duration::from_millis(2000) {
        // The poll interval of a bounded wait.
        std::thread::sleep(Duration::from_millis(10));
    }
    let n = started();
    let held: Vec<usize> = server
        .inner
        .lock()
        .unwrap()
        .active
        .values()
        .map(|l| l.held)
        .collect();
    for g in &gone {
        let _ = g.send(true);
    }
    let t1 = Instant::now();
    while !server.inner.lock().unwrap().active.is_empty() && t1.elapsed() < Duration::from_secs(10)
    {
        // The poll interval of a bounded wait.
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        n <= 1,
        "{n} jobs started on a host 20 of 20 busy with the floor at 1; slots held {held:?}"
    );
}

/// Three sleeping jobs started by share (floor 3) on a pool of 2 slots,
/// each holding no slot. Back by slots the pool does not count them: its
/// 2 tokens are free, and a fourth job starts at once, so 4 jobs run on a
/// pool of 2 until the three end.
#[test]
fn adversary_u6_back_by_slots_the_jobs_that_started_by_share_still_count() {
    pool_busy!(BUSY, busy, 0);
    let f = start_shares(2, 3, busy, |_| {});
    by_share(&f, 3, None);
    let mut clients = Vec::new();
    for _ in 0..3 {
        clients.push(quiet(&f).0);
    }
    wait_for_jobs(&f, "three jobs to run by share", || running(&f).len() == 3);
    f.server
        .set_admission(&crate::shares::Request {
            admission: Some(crate::shares::Mode::Slots),
            ..crate::shares::Request::default()
        })
        .unwrap();
    let (c, id) = submit(&f, sh(&f.root, "sleep 60"));
    clients.push(c);
    let started = claimed_within(&f, id, Duration::from_millis(1500));
    let free = f.server.pool().free();
    assert!(
        started.is_none(),
        "by slots a fourth job started on a pool of 2 beside 3 running; {free} slot(s) free"
    );
    end_all(&f, clients);
}

/// `slot admission shares` against a daemon without `/api/admission` is
/// meant to say "the daemon is older than this slot" (`status::admission`
/// looks for a 404, as AGENTS.md promises for an unknown `/api/` path).
/// The call is a `PUT`; this daemon answers a `PUT` on an `/api/` path it
/// lacks, as an older one does on `/api/admission`, with what?
#[test]
fn adversary_u6_a_put_on_an_api_path_the_daemon_lacks_answers_404() {
    use std::io::Read as _;
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let _f = start_cfg(2, |c| c.http = Some(addr));
    wait_for("the web address to listen", || {
        std::net::TcpStream::connect(addr).is_ok()
    });
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    let body = r#"{"admission":"shares"}"#;
    write!(
        s,
        "PUT /api/no-such-setting HTTP/1.0\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let first = raw.lines().next().unwrap_or_default().to_string();
    assert!(first.contains(" 404"), "{first}");
}

/// The bound on a CPU hold counts only time held for the host's CPUs: a job
/// held for CPU, then for disk past the bound, is held for CPU again when
/// the disk frees, and does not start as "held as long as a job is".
#[test]
fn time_held_for_disk_does_not_count_towards_the_cpu_holds_bound() {
    use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
    const GB: u64 = crate::admission::GB;
    static FREE: AtomicU64 = AtomicU64::new(60 * GB);
    #[allow(clippy::unnecessary_wraps, reason = "the type of Config::disk_free")]
    fn free(_: &Path) -> Option<u64> {
        Some(FREE.load(SeqCst))
    }
    host_busy!(BUSY, busy, 184);
    let f = start_busy(4, busy, |c| {
        c.cpu_hold_max = Duration::from_millis(300);
        c.disk_free = Some(free);
        c.admission.min_free_disk = 50 * GB;
    });
    set_cpu_limit(&f, Some(17.0), Some(1));
    let (first, a) = quiet(&f);
    wait_for_jobs(&f, "the first job to run", || running(&f) == [a]);
    let (second, b) = submit(&f, sh(&f.root, "echo second"));
    wait_for_jobs(&f, "the second job to be held for CPU", || {
        reason_of(&f, b).contains("host busy")
    });
    FREE.store(45 * GB, SeqCst);
    wait_for_jobs(&f, "the second job to be held for disk", || {
        reason_of(&f, b).contains("builds need 50.0 GB")
    });
    std::thread::sleep(Duration::from_millis(500));
    FREE.store(60 * GB, SeqCst);
    wait_for_jobs(&f, "the second job to be held for CPU again", || {
        reason_of(&f, b).contains("host busy")
    });
    assert!(
        census_events(&f, "cpu_hold_bound").is_empty(),
        "started on the bound although it was held for disk"
    );
    assert_eq!(running(&f), [a]);
    end_all(&f, vec![first, second]);
}

/// With a pressure mark the CPU limit holds a queued job while the host's
/// CPU pressure is at or over it, though the CPU figure is far under its own
/// mark, and lets it start when the pressure falls under the mark.
#[test]
fn a_start_is_held_while_cpu_pressure_is_at_or_over_its_mark_and_let_go_when_it_falls() {
    use std::sync::atomic::{AtomicI64, Ordering::SeqCst};
    static PRESSURE: AtomicI64 = AtomicI64::new(85);
    fn pressure() -> f64 {
        PRESSURE.load(SeqCst) as f64
    }
    host_busy!(BUSY, busy, 50);
    let f = start_busy(4, busy, |c| c.cpu_pressure = Some(pressure));
    let view = f
        .server
        .set_cpu_limit(&cpu_limit::Request {
            cpu_busy_max: Some(Some(17.0)),
            cpu_busy_floor: Some(1),
            cpu_pressure_max: Some(Some(80.0)),
        })
        .unwrap();
    assert_eq!(
        (view.cpu_busy_max, view.cpu_pressure_max, view.pressure),
        (Some(17.0), Some(80.0), Some(85.0))
    );
    assert_eq!(f.server.state().cpu_pressure_max, Some(80.0));
    let (first, a) = quiet(&f);
    wait_for_jobs(&f, "the first job to run: it is the floor", || {
        running(&f) == [a]
    });
    let (second, b) = submit(&f, sh(&f.root, "echo second"));
    let held = "host busy: CPU pressure 85 % at or over the mark 80 %";
    wait_for_jobs(&f, "the second job to be held for CPU pressure", || {
        reason_of(&f, b) == held
    });
    assert_eq!(state_of(&f, b), JobState::Queued);
    assert_eq!(f.server.cpu_limit_view().held, 1);
    assert_eq!(f.server.pool().free(), 3, "a held job holds no slot");

    PRESSURE.store(79, SeqCst);
    let res = collect(second);
    assert_eq!((res.code, res.out.as_str()), (0, "second\n"), "{res:?}");
    assert_eq!(f.server.cpu_limit_view().held, 0);
    end_all(&f, vec![first]);
}

/// Held jobs that reach the CPU hold's bound together start one by one,
/// `cpu_bound_gap` apart, not all at once.
#[test]
fn held_jobs_that_reach_the_bound_together_start_one_by_one() {
    host_busy!(BUSY, busy, 184);
    let f = start_busy(4, busy, |c| {
        c.cpu_hold_max = Duration::from_millis(300);
        c.cpu_bound_gap = Duration::from_millis(1500);
    });
    set_cpu_limit(&f, Some(17.0), Some(1));
    let (first, a) = quiet(&f);
    wait_for_jobs(&f, "the first job to run", || running(&f) == [a]);
    let (second, b) = quiet(&f);
    let (third, c) = quiet(&f);
    wait_for_jobs(&f, "one held job to start on the bound", || {
        census_events(&f, "cpu_hold_bound").len() == 1
    });
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(
        census_events(&f, "cpu_hold_bound").len(),
        1,
        "the second held job started within the gap"
    );
    wait_for_jobs(&f, "the other held job to start", || {
        census_events(&f, "cpu_hold_bound").len() == 2
    });
    let starts: Vec<i64> = census_events(&f, "cpu_hold_bound")
        .iter()
        .map(|e| e["t"].as_i64().unwrap())
        .collect();
    assert!(starts[1] - starts[0] >= 1500, "{starts:?}");
    wait_for_jobs(&f, "all three to run", || running(&f) == [a, b, c]);
    end_all(&f, vec![first, second, third]);
}

/// A job predicted to run at most `cpu_limit::PASS_MS` is not held for the
/// host's CPUs: on a busy host it starts while a job of a command without
/// history stays held, and the census records why it passed.
#[test]
fn a_job_predicted_short_is_not_held_for_the_hosts_cpus() {
    host_busy!(BUSY, busy, 184);
    let f = start_busy(4, busy, |_| {});
    give_history(&f, "app");
    set_cpu_limit(&f, Some(17.0), Some(1));
    let (first, a) = quiet(&f);
    wait_for_jobs(&f, "the first job to run", || running(&f) == [a]);
    let (long, l) = submit(&f, named(&f, LONG_CMD, "app", "long", None));
    wait_for_jobs(&f, "the job without history to be held for CPU", || {
        reason_of(&f, l).contains("host busy")
    });
    go(&f, "short");
    let (short, _) = submit(&f, named(&f, SHORT_CMD, "app", "short", None));
    wait_for_jobs(&f, "the short job to pass the CPU hold", || {
        !census_events(&f, "cpu_hold_short").is_empty()
    });
    assert_eq!(collect(short).code, 0);
    let passed = census_events(&f, "cpu_hold_short");
    assert_eq!(passed.len(), 1, "{passed:?}");
    assert!(passed[0]["predicted_ms"].as_u64().is_some(), "{passed:?}");
    assert!(
        reason_of(&f, l).contains("host busy"),
        "the long job started"
    );
    end_all(&f, vec![first, long]);
}

/// `GET /api/jobs` sends every job of `GET /api/state` as a bare array, the
/// ended ones newest first, and `GET /api/jobs/{id}` the one job or 404
/// (views `slot.pool.Jobs` and `slot.pool.JobDetail` of `spec/`).
#[test]
fn the_job_routes_answer_as_the_spec_binds_them() {
    use axum::response::IntoResponse;
    let f = start(2);
    assert_eq!(run(&f, sh(&f.root, "true")).code, 0);
    assert_eq!(run(&f, sh(&f.root, "exit 3")).code, 3);
    let state = || axum::extract::State(Arc::clone(&f.server));
    let (status, text) = answer(async { http::jobs(state()).await.into_response() });
    assert_eq!(status, 200, "{text}");
    let rows: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    let st = f.server.state();
    assert_eq!(rows.len(), st.active.len() + st.done.len(), "{text}");
    let newest = st.done.last().unwrap();
    assert_eq!(rows[0]["id"], newest.id, "newest first: {text}");
    assert_eq!(rows[0]["state"], "failed", "{text}");
    assert_eq!(rows[1]["state"], "ok", "{text}");
    let (status, text) = answer(http::job(state(), axum::extract::Path(newest.id)));
    assert_eq!(status, 200, "{text}");
    let one: crate::types::Job = serde_json::from_str(&text).unwrap();
    assert_eq!(&one, newest);
    let (status, text) = answer(http::job(state(), axum::extract::Path(newest.id + 100)));
    assert_eq!((status, text.as_str()), (404, ""));
}

/// The daemon's sandbox, or `None` with the reason printed where `bwrap` is
/// missing or cannot create namespaces here.
fn sandbox_or_skip(test: &str) -> Option<crate::sandbox::Sandbox> {
    match crate::sandbox::Sandbox::find(Vec::new(), false) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("{test}: skipped: {e:#}");
            None
        }
    }
}

/// A home directory with a toolchain in `.cargo/bin` and `.rustup`, the way
/// a build server has one.
fn fake_home(root: &Path) -> PathBuf {
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".cargo/bin")).unwrap();
    std::fs::create_dir_all(home.join(".rustup/toolchains")).unwrap();
    std::fs::write(home.join(".cargo/bin/cargo"), "the toolchain\n").unwrap();
    home
}

/// A job of a synced workspace under `--sandbox`: it writes its tree, its
/// target directory and /tmp; it cannot add a program to `~/.cargo/bin`,
/// replace one there, touch `~/.rustup`, or see another workspace, another
/// job's target directory or the daemon's socket. Its jobserver works.
#[test]
fn a_sandboxed_workspace_job_writes_only_what_it_builds() {
    let Some(sandbox) = sandbox_or_skip("a_sandboxed_workspace_job_writes_only_what_it_builds")
    else {
        return;
    };
    let f = start_cfg(2, |c| c.sandbox = Some(sandbox));
    let home = fake_home(&f.root);
    std::fs::create_dir_all(f.root.join("ws/mine")).unwrap();
    std::fs::create_dir_all(f.root.join("ws/other")).unwrap();
    std::fs::write(f.root.join("ws/other/keep"), "theirs\n").unwrap();
    std::fs::create_dir_all(f.root.join("cache/target/other")).unwrap();
    std::fs::write(f.root.join("cache/target/other/keep"), "theirs\n").unwrap();
    let script = format!(
        "w() {{ if ( eval \"$2\" ) 2>/dev/null; then echo \"$1=yes\"; else echo \"$1=no\"; fi; }}
         w bin 'echo x > {home}/.cargo/bin/x'
         w replace 'echo y > {home}/.cargo/bin/cargo'
         w rustup 'mkdir {home}/.rustup/toolchains/evil'
         w other_ws 'test -e {root}/ws/other/keep'
         w other_target 'test -e {root}/cache/target/other/keep'
         w socket 'test -S {sock}'
         w tree 'echo t > built.txt'
         w target 'echo t > \"$CARGO_TARGET_DIR/built.txt\"'
         w tmp 'echo t > /tmp/x && echo t > \"$TMPDIR/y\"'
         w registry 'echo t > {home}/.cargo/registry/x'
         w fifo 'f=\"${{CARGO_MAKEFLAGS##*fifo:}}\"; test -p \"$f\" && test -w \"$f\"'
         w pid1 'test \"$(cat /proc/1/comm)\" = bwrap'
         echo sandbox=$SLOT_SANDBOX",
        home = home.display(),
        root = f.root.display(),
        sock = f.socket.display(),
    );
    let req = Request {
        argv: vec!["sh".into(), "-c".into(), script],
        env: vec![
            format!("HOME={}", home.display()),
            format!("CARGO_HOME={}", home.join(".cargo").display()),
        ],
        workspace: Some(Workspace {
            id: "mine".into(),
            rel_cwd: ".".into(),
            client_root: "/client".into(),
            same_path: false,
            ..Workspace::default()
        }),
        ..Request::default()
    };
    let r = run(&f, req);
    assert_eq!(r.code, 0, "{r:?}");
    for want in [
        "bin=no",
        "replace=no",
        "rustup=no",
        "other_ws=no",
        "other_target=no",
        "socket=no",
        "tree=yes",
        "target=yes",
        "tmp=yes",
        "registry=yes",
        "fifo=yes",
        "pid1=yes",
        "sandbox=bwrap",
    ] {
        assert!(r.out.lines().any(|l| l == want), "{want} in {r:?}");
    }
    assert!(!home.join(".cargo/bin/x").exists());
    assert_eq!(
        std::fs::read_to_string(home.join(".cargo/bin/cargo")).unwrap(),
        "the toolchain\n"
    );
    assert!(f.root.join("ws/other/keep").exists());
    assert!(f.root.join("ws/mine/built.txt").exists());
    assert!(f.root.join("cache/target/mine/built.txt").exists());
}

/// A local job under `--sandbox` writes the checkout it runs in, and cannot
/// delete a file in a checkout beside it, which it sees read-only.
#[test]
fn a_sandboxed_local_job_cannot_delete_a_file_in_another_checkout() {
    let Some(sandbox) =
        sandbox_or_skip("a_sandboxed_local_job_cannot_delete_a_file_in_another_checkout")
    else {
        return;
    };
    let f = start_cfg(1, |c| c.sandbox = Some(sandbox));
    let home = fake_home(&f.root);
    for repo in ["mine", "theirs"] {
        std::fs::create_dir_all(f.root.join(repo).join(".git")).unwrap();
        std::fs::create_dir_all(f.root.join(repo).join("src")).unwrap();
    }
    std::fs::write(f.root.join("theirs/src/main.rs"), "fn main() {}\n").unwrap();
    let mut req = sh(
        &f.root.join("mine/src"),
        &format!(
            "rm {0}/theirs/src/main.rs; echo rm=$?; echo t > ../out.txt; echo own=$?",
            f.root.display()
        ),
    );
    req.env = std::env::vars()
        .map(|(k, v)| format!("{k}={v}"))
        .chain([format!("HOME={}", home.display())])
        .collect();
    let r = run(&f, req);
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("rm=1\n"), "{r:?}");
    assert!(r.out.contains("own=0\n"), "{r:?}");
    assert!(f.root.join("theirs/src/main.rs").exists());
    assert!(f.root.join("mine/out.txt").exists());
}

/// A crate of one file at `dir`, whose binary prints where it was built
/// from and where it runs.
fn tiny_crate(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.rs"),
        "fn main() {\n    println!(\"tiny ran\");\n    \
         println!(\"manifest={}\", env!(\"CARGO_MANIFEST_DIR\"));\n    \
         println!(\"cwd={}\", std::env::current_dir().unwrap().display());\n}\n",
    )
    .unwrap();
}

/// The environment of a cargo job with `HOME` and `CARGO_HOME` under
/// `root` and its target directory at `target`; the toolchain stays where
/// the caller has it, and rustup finds it there.
fn cargo_env(root: &Path, target: &Path) -> Vec<String> {
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".cargo")).unwrap();
    let rustup = std::env::var("RUSTUP_HOME").ok().or_else(|| {
        std::env::var("HOME")
            .ok()
            .map(|h| format!("{h}/.rustup"))
            .filter(|p| Path::new(p).is_dir())
    });
    let mut env: Vec<String> = std::env::vars()
        .filter(|(k, _)| !k.starts_with("CARGO") && k != "RUSTC_WRAPPER")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    env.extend([
        format!("HOME={}", home.display()),
        format!("CARGO_HOME={}", home.join(".cargo").display()),
        format!("CARGO_TARGET_DIR={}", target.display()),
        // A fixture of its own: no compile cache server for this child.
        "RUSTC_WRAPPER=".into(),
        "CARGO_BUILD_RUSTC_WRAPPER=".into(),
        "CARGO_INCREMENTAL=0".into(),
        "CARGO_NET_OFFLINE=true".into(),
    ]);
    env.extend(rustup.map(|r| format!("RUSTUP_HOME={r}")));
    env
}

const BUILD_AND_RUN: &str = "cargo build --quiet --offline && \"$CARGO_TARGET_DIR/debug/tiny\" \
                             && echo target=$CARGO_TARGET_DIR";

/// `cargo build` of a crate of one file succeeds under `--sandbox`, with
/// `HOME` and `CARGO_HOME` in temporary directories and the toolchain
/// read-only, and its binary runs.
#[test]
fn cargo_builds_a_tiny_crate_in_the_sandbox() {
    let Some(sandbox) = sandbox_or_skip("cargo_builds_a_tiny_crate_in_the_sandbox") else {
        return;
    };
    let f = start_cfg(1, |c| c.sandbox = Some(sandbox));
    let krate = f.root.join("tiny");
    tiny_crate(&krate);
    let target = f.root.join("target");
    let mut req = sh(&krate, BUILD_AND_RUN);
    req.env = cargo_env(&f.root, &target);
    let started = Instant::now();
    let r = run(&f, req);
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("tiny ran"), "{r:?}");
    assert!(target.join("debug/tiny").exists());
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
}

/// With `--sandbox-fixed-paths` two worktrees of one repository build at
/// the same paths: the job of each starts in /work/src, cargo builds into
/// /work/target and `CARGO_MANIFEST_DIR` is the same; the binaries land in
/// each worktree's own target directory on the host.
#[test]
fn with_fixed_paths_two_worktrees_build_at_the_same_paths() {
    let Some(sandbox) = sandbox_or_skip("with_fixed_paths_two_worktrees_build_at_the_same_paths")
    else {
        return;
    };
    let fixed = crate::sandbox::Sandbox::find(Vec::new(), true).unwrap();
    assert!(fixed.fixed() && !sandbox.fixed());
    let f = start_cfg(1, |c| c.sandbox = Some(fixed));
    let started = Instant::now();
    let mut seen = Vec::new();
    for wt in ["wt-a", "wt-b"] {
        let tree = f.root.join(wt);
        std::fs::create_dir_all(tree.join(".git")).unwrap();
        tiny_crate(&tree.join("tiny"));
        let target = f.root.join(format!("target-{wt}"));
        let mut req = sh(&tree.join("tiny"), BUILD_AND_RUN);
        req.env = cargo_env(&f.root, &target);
        let r = run(&f, req);
        assert_eq!(r.code, 0, "{wt}: {r:?}");
        assert!(target.join("debug/tiny").exists(), "{wt}: {r:?}");
        seen.push(r.out);
    }
    assert_eq!(seen[0], seen[1]);
    for want in [
        "manifest=/work/src/tiny",
        "cwd=/work/src/tiny",
        "target=/work/target",
    ] {
        assert!(seen[0].lines().any(|l| l == want), "{want} in {}", seen[0]);
    }
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
}

/// A synced cargo test job of workspace `ws`, repository `r`, whose client
/// measured its sync; the command, `cargo-test` as a link to the shell,
/// compiles for 300 ms into its target directory and then runs its tests
/// for 200 ms.
fn synced_test(f: &Fixture, ws: &str) -> Request {
    std::fs::create_dir_all(f.root.join("ws").join(ws)).unwrap();
    let script = "mkdir -p \"$CARGO_TARGET_DIR/debug\"; echo '   Compiling x v0.1.0' >&2; \
                  sleep 0.3; echo '     Running unittests src/lib.rs (target/debug/deps/x-1)' >&2; \
                  sleep 0.2; echo 'test adds ... ok'";
    Request {
        cwd: String::new(),
        env: Vec::new(),
        repo: "r".into(),
        sync_ms: Some(1234),
        sync_bytes: Some(5678),
        workspace: Some(Workspace {
            id: ws.into(),
            rel_cwd: ".".into(),
            client_root: "/srv/acct/r".into(),
            ..Workspace::default()
        }),
        ..fake_build(&f.root, &f.root.join("unused"), script)
    }
}

/// Runs `req` and returns its result and the `seeded_from` of its start
/// frame.
fn run_seeded(f: &Fixture, req: Request) -> (Res, Option<String>) {
    let mut r = Res {
        code: -999,
        ..Res::default()
    };
    let mut seeded = None;
    for line in BufReader::new(send(&f.socket, req)).lines() {
        let Ok(line) = line else { break };
        match serde_json::from_str::<Frame>(&line).unwrap() {
            Frame::Out { b } => r.out += &b,
            Frame::Err { b } => r.err += &b,
            Frame::Info { msg } | Frame::Fallback { msg } => r.info += &msg,
            Frame::Start { seeded_from, .. } => {
                r.started = true;
                seeded = seeded_from;
            }
            Frame::Exit { code } => {
                r.code = code;
                break;
            }
        }
    }
    (r, seeded)
}

/// A synced cargo test job's record carries the sync its client measured,
/// the target directory it built into on this server and what that held
/// when the job queued (also in its start frame), and its compile and test
/// time split at the first test binary. The client's back time comes after
/// the job, once, and is in the job log for the next start. A local job
/// takes none of the sync fields, and a job that runs no tests has no split.
#[test]
fn a_jobs_record_names_its_sync_phases_and_seed_on_the_node() {
    let f = start(1);
    let target = f.root.join("cache/target/w1");
    let (r, seeded) = run_seeded(&f, synced_test(&f, "w1"));
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(seeded.as_deref(), Some("empty"));
    let first = last_logged(&f);
    assert_eq!((first.sync_ms, first.sync_bytes), (Some(1234), Some(5678)));
    assert_eq!(first.back_ms, None);
    assert_eq!(
        first.node_target_dir.as_deref(),
        Some(target.to_str().unwrap())
    );
    assert_eq!(first.seeded_from.as_deref(), Some("empty"));
    let (compile, test) = (first.compile_ms.unwrap(), first.test_ms.unwrap());
    assert!(compile >= 250, "compile {compile} ms, test {test} ms");
    assert!(test >= 150, "compile {compile} ms, test {test} ms");
    assert_eq!(
        compile + test,
        u64::try_from(first.ended_ms - first.started_ms).unwrap()
    );

    let (r, seeded) = run_seeded(&f, synced_test(&f, "w1"));
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(seeded.as_deref(), Some("kept"));
    assert_eq!(last_logged(&f).seeded_from.as_deref(), Some("kept"));

    // A sibling of the same repository is seeded from w1 where the
    // filesystem makes reflink copies, else it starts empty.
    let (r, seeded) = run_seeded(&f, synced_test(&f, "w2"));
    assert_eq!(r.code, 0, "{r:?}");
    let want = if r.info.contains("seeded from workspace w1") {
        "w1"
    } else {
        "empty"
    };
    assert_eq!(seeded.as_deref(), Some(want), "{}", r.info);
    assert_eq!(last_logged(&f).seeded_from.as_deref(), Some(want));

    let amend = |id, back_ms| Request {
        amend: Some(crate::proto::Amend { id, back_ms }),
        ..Request::default()
    };
    assert_eq!(run(&f, amend(first.id, Some(77))).code, 0);
    assert_eq!(done_job(&f, first.id).back_ms, Some(77));
    let r = run(&f, amend(first.id, Some(99)));
    assert_eq!(r.code, 1, "once: {r:?}");
    assert_eq!(done_job(&f, first.id).back_ms, Some(77));
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    let line = format!(r#"{{"amend":{},"back_ms":77}}"#, first.id);
    assert!(log.lines().any(|l| l == line), "{log}");

    let mut local = fake_build(&f.root, &f.root.join("target"), "echo ran");
    local.sync_ms = Some(1);
    local.sync_bytes = Some(2);
    assert_eq!(run(&f, local).code, 0);
    let j = last_logged(&f);
    assert_eq!((j.sync_ms, j.sync_bytes), (None, None));
    assert_eq!((j.node_target_dir, j.seeded_from), (None, None));
    assert_eq!(j.compile_ms.map(|_| ()), Some(()), "a cargo test job");
    assert_eq!(j.test_ms, None, "no test binary ran");
    let r = run(&f, amend(j.id, Some(5)));
    assert_eq!(r.code, 1, "a local job: {r:?}");
    assert_eq!(run(&f, sh(&f.root, "true")).code, 0);
    let j = last_logged(&f);
    assert_eq!((j.compile_ms, j.test_ms), (None, None));

    let again = Server::new(config(&f.root, 1)).unwrap();
    let read = again
        .state()
        .done
        .into_iter()
        .find(|j| j.id == first.id)
        .unwrap();
    assert_eq!(read.back_ms, Some(77), "the amend line, read at start");
}

/// What `slot warm` sends after a merge to `main` of repository `repo`:
/// `script` stands for the build of the tree's tests.
fn merge_of(f: &Fixture, repo: &str, script: &str) -> Request {
    let mut r = sh(&f.root, script);
    r.repo = repo.into();
    r.warm = true;
    r
}

fn warm_jobs(f: &Fixture) -> Vec<Job> {
    f.server
        .state()
        .active
        .into_iter()
        .filter(|j| j.warm)
        .collect()
}

/// Told of a merge of a repository that built here in the last 24 hours,
/// the daemon answers at once and starts one warm-up job at `low`, the
/// lowest level, over a rule that puts the repository on `high`; a second
/// word of the same merge starts none. A job a session submits next waits
/// for the slot and preempts it: the warm-up ends cancelled with the job
/// that preempted it as its reason, and the session's job runs.
#[test]
fn a_merge_starts_one_low_warm_up_and_a_sessions_next_job_preempts_it() {
    let f = start(1);
    let mut built = sh(&f.root, "true");
    built.repo = "r".into();
    assert_eq!(run(&f, built).code, 0);
    f.server
        .set_priority(&priority::Request {
            pattern: Some("r".into()),
            level: Some(Priority::High),
            promote_after_s: None,
        })
        .unwrap();

    // `sleep 30` stands for the build; the session's job preempts it.
    let told = Instant::now();
    let r = run(&f, merge_of(&f, "r", "sleep 30"));
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.info.contains("warm-up"), "{r:?}");
    assert!(
        told.elapsed() < Duration::from_secs(5),
        "answered without waiting for the build: {:?}",
        told.elapsed()
    );
    wait_for("the warm-up to run", || {
        warm_jobs(&f).iter().any(|j| j.state == JobState::Running)
    });
    let again = run(&f, merge_of(&f, "r", "sleep 30"));
    assert_eq!(again.code, 0, "{again:?}");
    assert!(again.info.contains("not warmed"), "{again:?}");
    let warm = warm_jobs(&f);
    assert_eq!(warm.len(), 1, "one warm-up: {warm:?}");
    assert_eq!((warm[0].priority, warm[0].promoted), (Priority::Low, false));

    let mut next = sh(&f.root, "echo session");
    next.repo = "r".into();
    let r = run(&f, next);
    assert_eq!((r.code, r.out.trim()), (0, "session"), "{r:?}");
    let st = f.server.state();
    let session = st
        .done
        .iter()
        .find(|j| j.argv.iter().any(|a| a == "echo session"))
        .expect("the session's job ended");
    let ended = done_job(&f, warm[0].id);
    assert_eq!(ended.state, JobState::Cancelled, "{ended:?}");
    assert_eq!(
        ended.cancel_reason,
        format!("preempted by job {}", session.id)
    );
    let log = std::fs::read_to_string(f.root.join("state/jobs.jsonl")).unwrap();
    let logged: Job = log
        .lines()
        .filter_map(|l| serde_json::from_str::<Job>(l).ok())
        .find(|j| j.id == ended.id)
        .expect("the warm-up is in the job log");
    assert!(logged.warm, "{logged:?}");
    assert_eq!(logged.cancel_reason, ended.cancel_reason);
}

/// A repository that built here in no job of the last 24 hours is not
/// warmed, and nothing is queued: a job of 25 hours ago in the job log does
/// not count, one of 23 hours ago does.
#[test]
fn a_repository_that_built_in_no_job_of_the_last_day_is_not_warmed() {
    let hour = 3_600_000;
    let f = start_cfg(1, |c| {
        std::fs::create_dir_all(&c.state_dir).unwrap();
        let now = now_ms();
        let lines = [
            log_line(1, now - 25 * hour, "ok", 0, "local"),
            log_line(2, now - 23 * hour, "ok", 0, "local")
                .replace(r#""repo":"r""#, r#""repo":"q""#),
        ];
        std::fs::write(c.state_dir.join("jobs.jsonl"), lines.join("\n") + "\n").unwrap();
    });
    for repo in ["r", "never"] {
        let r = run(&f, merge_of(&f, repo, "true"));
        assert_eq!(r.code, 0, "{r:?}");
        assert!(r.info.contains("not warmed"), "{repo}: {r:?}");
    }
    assert_eq!(f.server.state().active.len(), 0, "nothing queued");
    assert_eq!(f.server.inner.lock().unwrap().next_id, 2, "nothing queued");
    let r = run(&f, merge_of(&f, "q", "true"));
    assert!(r.info.contains("warm-up"), "{r:?}");
    assert!(!r.info.contains("not warmed"), "{r:?}");
    wait_for("the warm-up of q to end", || {
        f.server
            .state()
            .done
            .iter()
            .any(|j| j.warm && j.repo == "q")
    });
}

/// A warm-up on a node builds into the target directory of its own
/// workspace and leaves it as the one a new workspace of the repository is
/// seeded from, even when another workspace's job started while it built.
/// That job had slots free and did not preempt it.
#[test]
fn a_warm_up_leaves_the_target_directory_new_workspaces_are_seeded_from() {
    let f = start(2);
    assert_eq!(run(&f, synced_test(&f, "a1")).code, 0);
    let mut warm = synced_test(&f, "b0");
    warm.argv[2] = "mkdir -p \"$CARGO_TARGET_DIR/debug\"; sleep 1".into();
    warm.warm = true;
    let r = run(&f, warm);
    assert_eq!(r.code, 0, "{r:?}");
    wait_for("the warm-up to run", || {
        warm_jobs(&f).iter().any(|j| j.state == JobState::Running)
    });
    // Queued after the warm-up started, ends before it.
    let r = run(&f, synced_test(&f, "a1"));
    assert_eq!(r.code, 0, "{r:?}");
    wait_for("the warm-up to end", || {
        f.server.state().done.iter().any(|j| j.warm)
    });
    let st = f.server.state();
    let ended = st.done.iter().find(|j| j.warm).unwrap();
    assert_eq!(ended.state, JobState::Ok, "{ended:?}");
    assert_eq!(
        ended.node_target_dir.as_deref(),
        Some(f.root.join("cache/target/b0").to_str().unwrap())
    );
    // The record is touched off the request path, after the job ended.
    let cache = cache::Cache::new(&f.root.join("cache"), &f.root.join("ws"));
    let seeded_from = || cache.seed_source("c2", "r", &HashSet::new()).map(|e| e.id);
    wait_for("the warm-up's target directory to be the seed", || {
        seeded_from().as_deref() == Some("b0")
    });
}
