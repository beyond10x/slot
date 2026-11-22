//! `systemctl --user reload slot` with jobs running: the daemon re-executes
//! at once and the new process keeps every running and queued job, its
//! output, its slots and its client (`docs/design.md` § 15.2).

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SLOT: &str = env!("CARGO_BIN_EXE_slot");
const O_NONBLOCK: i32 = 0o4000;
const LINES: usize = 300;

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
}

impl Daemon {
    fn start(tokens: usize, env: &[(&str, &str)]) -> Self {
        Self::start_with_http(tokens, env, "off")
    }

    fn start_with_http(tokens: usize, env: &[(&str, &str)], addr: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let log = std::fs::File::create(dir.path().join("daemon.log")).unwrap();
        let mut cmd = Command::new(SLOT);
        cmd.args(["serve", "--http", addr, "--cpu-quota", "false"])
            .args(["--tokens", &tokens.to_string()])
            .args(["--cpu-weight", "0", "--evict-below", "0"])
            .args(["--call-headroom", "0", "--sccache-watch", "false"])
            .args(["--min-free-disk", "0", "--min-free-memory", "0"])
            .arg("--socket")
            .arg(dir.path().join("sock"))
            .arg("--state-dir")
            .arg(dir.path().join("state"))
            .env_remove("SLOT_JOB")
            .stdout(Stdio::null())
            .stderr(log);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().unwrap();
        let d = Self { child, dir };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !d.socket().exists() {
            assert!(
                Instant::now() < deadline,
                "daemon did not start: {}",
                d.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        d
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("sock")
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("daemon.log")).unwrap_or_default()
    }

    fn reload(&self) {
        nix_kill(self.child.id(), "HUP");
    }

    fn client(&self, script: &str) -> Child {
        Command::new(SLOT)
            .args(["sh", "-c", script])
            .env("SLOT_SOCKET", self.socket())
            .env_remove("SLOT_JOB")
            .env_remove("SLOT_HOST")
            .current_dir(self.dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// Tokens in the FIFO: taken non-blocking and written back.
    fn free_tokens(&self) -> usize {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(self.state().join("jobserver.fifo"))
            .unwrap();
        let mut buf = [0u8; 4096];
        let n = f.read(&mut buf).unwrap_or(0);
        f.write_all(&buf[..n]).unwrap();
        n
    }

    /// Child processes of the daemon: the jobs it started.
    fn children(&self) -> usize {
        let pid = self.child.id();
        std::fs::read_dir(format!("/proc/{pid}/task")).map_or(0, |d| {
            d.flatten()
                .map(|t| {
                    std::fs::read_to_string(t.path().join("children"))
                        .unwrap_or_default()
                        .split_whitespace()
                        .count()
                })
                .sum()
        })
    }

    /// Open sockets of the daemon: the listener and one per client.
    fn sockets(&self) -> usize {
        let pid = self.child.id();
        std::fs::read_dir(format!("/proc/{pid}/fd")).map_or(0, |d| {
            d.flatten()
                .filter(|f| {
                    std::fs::read_link(f.path())
                        .is_ok_and(|l| l.to_string_lossy().starts_with("socket:"))
                })
                .count()
        })
    }

    /// Waits until `ok` holds; the condition, not a fixed sleep, so a
    /// loaded machine only makes the test slower.
    fn wait_until(&self, what: &str, within: Duration, ok: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + within;
        while !ok(self) {
            assert!(
                Instant::now() < deadline,
                "{what} within {within:?}:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn logged(&self, needle: &str) -> Vec<(i64, i32)> {
        std::fs::read_to_string(self.state().join("jobs.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains(needle))
            .map(|l| (field(l, "\"id\":"), field(l, "\"exit\":") as i32))
            .collect()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn nix_kill(pid: u32, sig: &str) {
    let ok = Command::new("kill")
        .args([format!("-{sig}"), pid.to_string()])
        .status()
        .unwrap()
        .success();
    assert!(ok, "kill -{sig} {pid}");
}

fn field(line: &str, key: &str) -> i64 {
    let rest = &line[line.find(key).unwrap() + key.len()..];
    let end = rest.find([',', '}']).unwrap();
    rest[..end].trim().parse().unwrap()
}

fn counter(tag: &str, code: i32) -> String {
    format!(
        "i=0; while [ $i -lt {LINES} ]; do echo {tag}$i; i=$((i+1)); sleep 0.01; done; exit {code}"
    )
}

fn expected(tag: &str) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for i in 0..LINES {
        let _ = writeln!(s, "{tag}{i}");
    }
    s
}

fn wait_all(clients: Vec<(String, Child)>, within: Duration) -> Vec<(String, i32, String, String)> {
    let deadline = Instant::now() + within;
    let mut out = Vec::new();
    for (tag, mut c) in clients {
        loop {
            if c.try_wait().unwrap().is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "client {tag} did not end");
            std::thread::sleep(Duration::from_millis(50));
        }
        let o = c.wait_with_output().unwrap();
        out.push((
            tag,
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ));
    }
    out
}

fn http_get(addr: SocketAddr, path: &str) -> Option<BufReader<TcpStream>> {
    let timeout = Duration::from_millis(500);
    let mut stream = TcpStream::connect_timeout(&addr, timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    if !line.starts_with("HTTP/1.1 200 ") {
        return None;
    }
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" {
            return Some(reader);
        }
    }
}

fn dashboard_ticks(addr: SocketAddr) -> Option<Vec<serde_json::Value>> {
    serde_json::from_reader(http_get(addr, "/api/history")?).ok()
}

fn dashboard_output_contains(addr: SocketAddr, id: i64, marker: &str) -> bool {
    http_get(addr, &format!("/api/jobs/{id}/output")).is_some_and(|reader| {
        reader
            .lines()
            .map_while(Result::ok)
            .any(|l| l.contains(marker))
    })
}

#[test]
fn chart_history_reads_persisted_cpu_and_queue_windows() {
    let bind = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = bind.local_addr().unwrap();
    drop(bind);
    let d = Daemon::start_with_http(2, &[], &addr.to_string());
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let first = serde_json::json!({"t":now-60_000,"host_cores":12.25,"waiting":4.2,"n":10});
    let second = serde_json::json!({"t":now-20_000,"host_cores":9.75,"waiting":1.5,"n":10});
    {
        let mut log = std::fs::File::create(d.state().join("census.jsonl")).unwrap();
        let too_old =
            serde_json::json!({"t":now-6*3_600_000-60_000,"host_cores":4.0,"waiting":1.0,"n":10});
        writeln!(log, "{too_old}\n{second}").unwrap();
        // An invalid line wider than the initial tail forces another read;
        // the complete in-range row before it must still be found.
        log.write_all(&vec![b'x'; 1 << 20]).unwrap();
        writeln!(log, "\n{{\"t\":{now},\"event\":\"tokens_restored\"}}").unwrap();
        writeln!(log, "{first}").unwrap();
        let unfinished = serde_json::json!({"t":now,"host_cores":8.0,"waiting":2.0,"n":10});
        write!(log, "{unfinished}").unwrap();
    }
    d.wait_until("chart history API", Duration::from_secs(2), |_| {
        http_get(addr, "/api/chart-history").is_some()
    });
    let samples: serde_json::Value =
        serde_json::from_reader(http_get(addr, "/api/chart-history").unwrap()).unwrap();
    assert_eq!(samples, serde_json::json!([first, second]));
}

#[test]
fn reload_keeps_dashboard_history_and_output() {
    let bind = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = bind.local_addr().unwrap();
    drop(bind);
    let d = Daemon::start_with_http(2, &[], &addr.to_string());
    let completed = d.client("echo finished-before-reload");
    let done = wait_all(vec![("finished".into(), completed)], Duration::from_secs(5));
    assert_eq!(done[0].1, 0);
    let finished_id = d.logged("echo finished-before-reload")[0].0;
    let running = d.client(
        "echo running-before-reload; echo stderr-before-reload >&2; \
         i=0; while [ ! -e release ] && [ $i -lt 250 ]; do sleep 0.1; i=$((i+1)); done; \
         test -e release && echo running-after-reload",
    );
    let running_id = finished_id + 1;
    d.wait_until("original dashboard output", Duration::from_secs(6), |_| {
        dashboard_ticks(addr).is_some_and(|ticks| !ticks.is_empty())
            && dashboard_output_contains(addr, running_id, "running-before-reload")
            && dashboard_output_contains(addr, running_id, "stderr-before-reload")
    });
    let before = dashboard_ticks(addr).unwrap();
    assert_eq!(
        before.first().and_then(|t| t["type"].as_str()),
        Some("tick")
    );

    d.reload();
    d.wait_until("running job handed over", Duration::from_secs(6), |d| {
        d.log().contains("took over 1 running")
            && dashboard_ticks(addr).is_some_and(|after| after.starts_with(&before))
    });
    assert!(dashboard_output_contains(
        addr,
        finished_id,
        "finished-before-reload"
    ));
    assert!(dashboard_output_contains(
        addr,
        running_id,
        "running-before-reload"
    ));
    assert!(dashboard_output_contains(
        addr,
        running_id,
        "stderr-before-reload"
    ));

    std::fs::write(d.dir.path().join("release"), []).unwrap();
    let done = wait_all(vec![("running".into(), running)], Duration::from_secs(6));
    assert_eq!(done[0].1, 0);
    assert!(dashboard_output_contains(
        addr,
        running_id,
        "running-before-reload"
    ));
    assert!(dashboard_output_contains(
        addr,
        running_id,
        "running-after-reload"
    ));
}

#[test]
fn reloads_keep_running_and_queued_jobs_their_output_and_slots() {
    let d = Daemon::start(20, &[]);
    let pid = d.child.id();
    assert_eq!(d.free_tokens(), 20);
    let mut clients = Vec::new();
    for i in 0..40 {
        let tag = format!("j{i}-");
        clients.push((tag.clone(), d.client(&counter(&tag, 7))));
        std::thread::sleep(Duration::from_millis(5));
    }
    d.wait_until(
        "20 jobs running and 40 clients connected",
        Duration::from_secs(60),
        |d| d.children() == 20 && d.sockets() > 40,
    );
    d.reload();
    d.wait_until("the first handover", Duration::from_secs(60), |d| {
        d.log().contains("took over")
    });
    d.reload();
    let done = wait_all(clients, Duration::from_secs(120));
    let log = d.log();
    assert!(
        log.matches("handing ").count() >= 2,
        "two handovers expected:\n{log}"
    );
    // A client counted in `sockets()` may not be queued yet when the first
    // reload comes, so the count queued varies; what is handed over must be
    // what the next binary takes over.
    let first = |prefix: &str| {
        let rest = &log[log
            .find(prefix)
            .unwrap_or_else(|| panic!("{prefix}:\n{log}"))
            + prefix.len()..];
        let n: Vec<usize> = rest
            .split_whitespace()
            .take(4)
            .filter_map(|w| w.parse().ok())
            .collect();
        (n[0], n[1])
    };
    let (running, queued) = first("handing ");
    assert_eq!((running, queued), first("took over "), "{log}");
    assert_eq!(running, 20, "{log}");
    assert!(queued > 0, "nothing queued at the first reload:\n{log}");
    assert!(!log.contains("reconciled"), "tokens went missing:\n{log}");
    for (tag, code, out, err) in &done {
        assert_eq!(*code, 7, "client {tag}: {err}");
        assert!(
            *out == expected(tag),
            "client {tag} output differs:\n{out}\nstderr:\n{err}"
        );
    }
    assert_eq!(d.child.id(), pid);
    let jobs = d.logged("echo j");
    let ids: BTreeSet<i64> = jobs.iter().map(|(id, _)| *id).collect();
    assert_eq!(jobs.len(), 40, "jobs logged: {jobs:?}");
    assert_eq!(ids.len(), 40, "a job logged twice: {jobs:?}");
    assert!(jobs.iter().all(|(_, exit)| *exit == 7), "{jobs:?}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(d.free_tokens(), 20, "every slot is back:\n{log}");
}

#[test]
fn an_unreadable_handover_file_starts_a_plain_daemon() {
    let d = Daemon::start(2, &[("SLOT_HANDOVER", "/nonexistent/handover.json")]);
    assert!(d.log().contains("handover state unreadable"), "{}", d.log());
    let done = wait_all(
        vec![("a-".into(), d.client(&counter("a-", 0)))],
        Duration::from_secs(30),
    );
    assert_eq!(done[0].1, 0, "{}", done[0].3);
    assert_eq!(done[0].2, expected("a-"));
    assert_eq!(d.free_tokens(), 2);
}

#[test]
fn a_finished_job_logged_before_the_reload_is_not_logged_again() {
    let d = Daemon::start(2, &[]);
    let quick = d.client("echo quick");
    let done = wait_all(vec![("q".into(), quick)], Duration::from_secs(10));
    assert_eq!(done[0].1, 0);
    d.reload();
    std::thread::sleep(Duration::from_millis(500));
    let again = wait_all(
        vec![("r".into(), d.client("echo after"))],
        Duration::from_secs(10),
    );
    assert_eq!(
        (again[0].1, again[0].2.as_str()),
        (0, "after\n"),
        "{}",
        again[0].3
    );
    assert_eq!(d.logged("echo quick").len(), 1);
    assert_eq!(d.free_tokens(), 2, "{}", d.log());
    assert!(!d.log().contains("reconciled"), "{}", d.log());
}

#[test]
fn a_burst_of_reloads_never_ends_the_daemon() {
    let mut d = Daemon::start(4, &[]);
    let pid = d.child.id();
    let mut clients = Vec::new();
    for i in 0..8 {
        let tag = format!("b{i}-");
        clients.push((tag.clone(), d.client(&counter(&tag, 3))));
    }
    d.wait_until("4 jobs running", Duration::from_secs(60), |d| {
        d.children() == 4
    });
    for _ in 0..6 {
        d.reload();
        std::thread::sleep(Duration::from_millis(30));
    }
    let done = wait_all(clients, Duration::from_secs(120));
    let log = d.log();
    for (tag, code, out, err) in &done {
        assert_eq!(*code, 3, "client {tag}: {err}\n{log}");
        assert!(*out == expected(tag), "client {tag} output differs:\n{log}");
    }
    assert!(log.matches("handing ").count() >= 2, "{log}");
    assert!(
        d.child.try_wait().unwrap().is_none(),
        "the daemon ended:\n{log}"
    );
    assert_eq!(d.child.id(), pid);
    assert_eq!(d.logged("echo b").len(), 8, "{log}");
    d.wait_until("every slot back", Duration::from_secs(10), |d| {
        d.free_tokens() == 4
    });
}

/// A job handed over keeps its run-time limit and its start. It names 5 s
/// with `SLOT_TIMEOUT`, the daemon reloads 3 s into its run, and the image
/// that took it over ends it 5 s after the job started, not 5 s after the
/// reload.
#[test]
fn a_handed_over_job_keeps_its_run_time_limit_and_its_start() {
    let d = Daemon::start(2, &[]);
    let client = Command::new(SLOT)
        .args(["sh", "-c", "echo limited; sleep 30"])
        .env("SLOT_SOCKET", d.socket())
        .env("SLOT_TIMEOUT", "5")
        .env_remove("SLOT_JOB")
        .env_remove("SLOT_HOST")
        .current_dir(d.dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    d.wait_until("the job to run", Duration::from_secs(30), |d| {
        d.children() == 1
    });
    std::thread::sleep(Duration::from_secs(3));
    let reload_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    d.reload();
    let done = wait_all(vec![("limited".into(), client)], Duration::from_secs(40));
    let log = d.log();
    assert!(log.contains("took over 1 running"), "{log}");
    let (_, code, out, err) = &done[0];
    assert_eq!(*code, 124, "{err}\n{log}");
    assert_eq!(out, "limited\n");
    assert!(err.contains("over its run-time limit of 5s"), "{err}");
    let jobs = std::fs::read_to_string(d.state().join("jobs.jsonl")).unwrap();
    let line = jobs.lines().last().unwrap();
    assert!(line.contains(r#""timed_out":true"#), "{line}");
    assert_eq!(field(line, "\"limit_s\":"), 5, "{line}");
    assert_eq!(field(line, "\"exit\":"), 124, "{line}");
    let (started, ended) = (field(line, "\"started_ms\":"), field(line, "\"ended_ms\":"));
    assert!(ended - started >= 5000, "ended early: {line}");
    assert!(
        ended < reload_ms + 5000,
        "the limit started again at the reload ({reload_ms}): {line}"
    );
    d.wait_until("every slot back", Duration::from_secs(10), |d| {
        d.free_tokens() == 2
    });
}

/// `slot job-parallel 3`, set through the command while a job runs, holds
/// across a reload. The image that took over names the bound in `slot
/// job-parallel` and `slot status` and starts a job with `CARGO_BUILD_JOBS`
/// and `MAKEFLAGS=-j` at 3 and without the pool's jobserver. The job that
/// started before the setting keeps the pool's jobserver through the reload
/// and ends as usual. `off` takes the value out of `limits.json` again.
#[test]
fn the_bound_per_job_holds_across_a_reload() {
    let bind = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = bind.local_addr().unwrap();
    drop(bind);
    let d = Daemon::start_with_http(2, &[], &addr.to_string());
    let slot = |args: &[&str]| {
        let o = Command::new(SLOT)
            .args(args)
            .args(["--http", &addr.to_string()])
            .env_remove("SLOT_JOB")
            .env_remove("SLOT_HOST")
            .output()
            .unwrap();
        (
            o.status.code(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    let limits = || std::fs::read_to_string(d.state().join("limits.json")).unwrap_or_default();
    let on = "at most 3 build processes per job; they take no slots of the pool\n";
    let off = "no limit per job: every build process takes a slot of the pool\n";
    let env = "echo jobs=${CARGO_BUILD_JOBS-unset} make=${MAKEFLAGS-unset} \
               cargo=${CARGO_MAKEFLAGS-unset}";
    d.wait_until("the web address", Duration::from_secs(10), |_| {
        http_get(addr, "/api/state").is_some()
    });
    let (code, out, err) = slot(&["job-parallel"]);
    assert_eq!((code, out.as_str()), (Some(0), off), "{err}");

    let before = d.client(&format!(
        "{env}; i=0; while [ ! -e release ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done; \
         test -e release"
    ));
    d.wait_until("the first job's output", Duration::from_secs(30), |_| {
        dashboard_output_contains(addr, 1, "jobs=unset")
    });
    let (code, out, err) = slot(&["job-parallel", "3"]);
    assert_eq!((code, out.as_str()), (Some(0), on), "{err}");
    assert!(limits().contains(r#""job_parallel":3"#), "{}", limits());
    let (code, _, err) = slot(&["job-parallel", "0"]);
    assert_eq!(code, Some(2), "0 is refused by the command: {err}");
    assert!(err.contains("at least 1"), "{err}");

    d.reload();
    d.wait_until("the job handed over", Duration::from_secs(10), |d| {
        d.log().contains("took over 1 running") && http_get(addr, "/api/state").is_some()
    });
    let (code, out, err) = slot(&["job-parallel"]);
    assert_eq!(
        (code, out.as_str()),
        (Some(0), on),
        "after the reload: {err}"
    );
    let (_, status, err) = slot(&["status"]);
    assert!(
        status.contains("at most 3 build processes per job, outside the pool's slots\n"),
        "{status}{err}"
    );
    let after = d.client(env);
    let done = wait_all(vec![("after".into(), after)], Duration::from_secs(20));
    assert_eq!(done[0].1, 0, "{}", done[0].3);
    assert_eq!(done[0].2, "jobs=3 make=-j3 cargo=unset\n");

    std::fs::write(d.dir.path().join("release"), "").unwrap();
    let done = wait_all(vec![("before".into(), before)], Duration::from_secs(20));
    let pool = format!(
        "-j --jobserver-auth=fifo:{}",
        d.state().join("jobserver.fifo").display()
    );
    assert_eq!(done[0].1, 0, "{}", done[0].3);
    assert_eq!(
        done[0].2,
        format!("jobs=unset make={pool} cargo={pool}\n"),
        "the job from before the setting"
    );

    let (code, out, err) = slot(&["job-parallel", "off"]);
    assert_eq!((code, out.as_str()), (Some(0), off), "{err}");
    assert!(!limits().contains("job_parallel"), "{}", limits());
    d.wait_until("every slot back", Duration::from_secs(10), |d| {
        d.free_tokens() == 2
    });
}

/// `slot cpu-limit` prints, sets and switches off the CPU limit of a live
/// daemon, refuses what is no mark and no floor, and the values hold across
/// a reload: they are in `limits.json`, which the new image reads. The mark
/// used is above any host's CPU count, so it holds no start here and the
/// test does not depend on what else the machine runs.
#[test]
fn the_cpu_limit_holds_across_a_reload() {
    let bind = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = bind.local_addr().unwrap();
    drop(bind);
    let d = Daemon::start_with_http(2, &[], &addr.to_string());
    let slot = |args: &[&str]| {
        let o = Command::new(SLOT)
            .args(args)
            .args(["--http", &addr.to_string()])
            .env_remove("SLOT_JOB")
            .env_remove("SLOT_HOST")
            .output()
            .unwrap();
        (
            o.status.code(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    let limits = || std::fs::read_to_string(d.state().join("limits.json")).unwrap_or_default();
    let off = "no CPU limit: a queued job starts whatever the host's CPUs do\n";
    let on = "the CPU limit of 4096.0 CPUs never holds a start: the host has ";
    d.wait_until("the web address", Duration::from_secs(10), |_| {
        http_get(addr, "/api/state").is_some()
    });
    let (code, out, err) = slot(&["cpu-limit"]);
    assert_eq!((code, out.as_str()), (Some(0), off), "{err}");

    for (args, exit, says) in [
        (&["cpu-limit", "0.5"][..], 2, "from 1"),
        (&["cpu-limit", "many"][..], 2, "is not a number of CPUs"),
        (&["cpu-limit", "4096", "--floor", "0"][..], 2, "at least 1"),
        (
            &["cpu-limit", "off", "--floor", "3"][..],
            1,
            "takes no --floor",
        ),
        (&["cpu-limit", "--floor", "3"][..], 1, "needs cpu_busy_max"),
    ] {
        let (code, out, err) = slot(args);
        assert_eq!(code, Some(exit), "{args:?}: {out}{err}");
        assert!(err.contains(says), "{args:?}: {err}");
        assert!(!limits().contains("cpu_busy"), "{args:?}: {}", limits());
    }

    let before = d.client(
        "echo started; i=0; while [ ! -e release ] && [ $i -lt 600 ]; do sleep 0.05; \
         i=$((i+1)); done; test -e release",
    );
    d.wait_until("the first job's output", Duration::from_secs(30), |_| {
        dashboard_output_contains(addr, 1, "started")
    });
    let (code, out, err) = slot(&["cpu-limit", "4096", "--floor", "2"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.starts_with(on) && out.ends_with('\n'), "{out}");
    assert!(
        limits().contains(r#""cpu_busy_max":4096.0,"cpu_busy_floor":2"#),
        "{}",
        limits()
    );

    d.reload();
    d.wait_until("the job handed over", Duration::from_secs(10), |d| {
        d.log().contains("took over 1 running") && http_get(addr, "/api/state").is_some()
    });
    let (code, again, err) = slot(&["cpu-limit"]);
    assert_eq!(
        (code, again.as_str()),
        (Some(0), out.as_str()),
        "after the reload: {err}"
    );
    let (_, status, err) = slot(&["status"]);
    assert!(status.contains(on), "{status}{err}");
    let state = http_get(addr, "/api/state").map(|mut r| {
        let mut body = String::new();
        let _ = r.read_to_string(&mut body);
        body
    });
    let state = state.unwrap_or_default();
    assert!(
        state.contains(r#""cpu_busy_max":4096.0,"cpu_busy_floor":2"#),
        "{state}"
    );
    let after = d.client("echo after");
    let done = wait_all(vec![("after".into(), after)], Duration::from_secs(20));
    assert_eq!(
        (done[0].1, done[0].2.as_str()),
        (0, "after\n"),
        "{}",
        done[0].3
    );

    let (code, out, err) = slot(&["cpu-limit", "off"]);
    assert_eq!((code, out.as_str()), (Some(0), off), "{err}");
    assert!(!limits().contains("cpu_busy"), "{}", limits());
    let (_, status, _) = slot(&["status"]);
    assert!(!status.contains("CPU limit"), "{status}");

    std::fs::write(d.dir.path().join("release"), "").unwrap();
    let done = wait_all(vec![("before".into(), before)], Duration::from_secs(20));
    assert_eq!(done[0].1, 0, "{}", done[0].3);
}

// adversary u4

/// The soft (`which = 3`) or hard (`which = 4`) open-file limit of `pid`,
/// read from `/proc/<pid>/limits` (`Max open files  <soft>  <hard>  files`).
fn nofile_of(pid: &str, which: usize) -> u64 {
    let t = std::fs::read_to_string(format!("/proc/{pid}/limits")).unwrap();
    let line = t.lines().find(|l| l.starts_with("Max open files")).unwrap();
    line.split_whitespace().nth(which).unwrap().parse().unwrap()
}

/// A daemon as [`Daemon::start`] starts it, with the soft open-file limit
/// `soft`, as systemd gives `slot.service` 1,024.
fn start_with_soft_nofile(soft: u64) -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let log = std::fs::File::create(dir.path().join("daemon.log")).unwrap();
    let child = Command::new("sh")
        .arg("-c")
        .arg(format!("ulimit -Sn {soft} && exec \"$0\" \"$@\""))
        .arg(SLOT)
        .args(["serve", "--http", "off", "--cpu-quota", "false"])
        .args(["--tokens", "4"])
        .args(["--cpu-weight", "0", "--evict-below", "0"])
        .args(["--call-headroom", "0", "--sccache-watch", "false"])
        .args(["--min-free-disk", "0", "--min-free-memory", "0"])
        .arg("--socket")
        .arg(dir.path().join("sock"))
        .arg("--state-dir")
        .arg(dir.path().join("state"))
        .env_remove("SLOT_JOB")
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .unwrap();
    let d = Daemon { child, dir };
    d.wait_until("the daemon to start", Duration::from_secs(10), |d| {
        d.socket().exists()
    });
    d
}

/// What a job prints for a client older than the open-file limit: the
/// request carries no `nofile`.
fn job_of_an_older_client(d: &Daemon, script: &str) -> String {
    let env: Vec<String> = std::env::vars()
        .filter(|(k, _)| k != "SLOT_JOB" && k != "SLOT_HOST")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    let req = serde_json::json!({
        "argv": ["sh", "-c", script],
        "cwd": d.dir.path().to_str().unwrap(),
        "env": env,
    });
    let mut c = std::os::unix::net::UnixStream::connect(d.socket()).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    writeln!(c, "{req}").unwrap();
    let mut out = String::new();
    for line in BufReader::new(c.try_clone().unwrap()).lines() {
        let v: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        match v["t"].as_str() {
            Some("out") => out.push_str(v["b"].as_str().unwrap_or_default()),
            Some("exit") => break,
            _ => {}
        }
    }
    out
}

/// `slot sh -c <script>` from a caller whose soft open-file limit is `soft`.
fn caller_with_soft_nofile(d: &Daemon, soft: u64, script: &str) -> Child {
    Command::new("sh")
        .arg("-c")
        .arg(format!("ulimit -Sn {soft} && exec \"$0\" sh -c \"$1\""))
        .arg(SLOT)
        .arg(script)
        .env("SLOT_SOCKET", d.socket())
        .env_remove("SLOT_JOB")
        .env_remove("SLOT_HOST")
        .current_dir(d.dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// The soft open-file limit of `pid`, `None` once it is gone.
fn soft_nofile_if_alive(pid: &str) -> Option<u64> {
    let t = std::fs::read_to_string(format!("/proc/{pid}/limits")).ok()?;
    let line = t.lines().find(|l| l.starts_with("Max open files"))?;
    line.split_whitespace().nth(3)?.parse().ok()
}

/// The pids of the daemon's children: the jobs it started.
fn job_pids(d: &Daemon) -> Vec<String> {
    let pid = d.child.id();
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .map(|tasks| {
            tasks
                .flatten()
                .flat_map(|t| {
                    std::fs::read_to_string(t.path().join("children"))
                        .unwrap_or_default()
                        .split_whitespace()
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A daemon started with the soft limit 512, with a job running of a caller
/// whose soft limit is `high` (up to 8,192). Before the daemon started jobs
/// through `slot __exec`, it raised its own soft limit to `high` for that
/// job; now the job's process has `high` and the daemon keeps 512. Returns
/// the daemon, the caller and `high`.
fn daemon_raised_by_a_running_job() -> (Daemon, Child, u64) {
    let high = nofile_of("self", 4).min(8192);
    assert!(high > 1024, "the test needs a hard limit above 1024");
    let d = start_with_soft_nofile(512);
    let pid = d.child.id().to_string();
    assert_eq!(nofile_of(&pid, 3), 512);
    let caller = caller_with_soft_nofile(&d, high, "sleep 3; ulimit -Sn");
    d.wait_until(
        "the caller's job to run with the caller's soft limit",
        Duration::from_secs(20),
        |d| {
            job_pids(d)
                .iter()
                .any(|p| soft_nofile_if_alive(p) == Some(high))
        },
    );
    assert_eq!(
        nofile_of(&pid, 3),
        512,
        "the daemon keeps its own soft limit"
    );
    (d, caller, high)
}

/// Story acceptance 1: "`slot sh -c 'ulimit -Sn'` prints the caller's soft
/// limit". A caller whose limit is lower than the daemon's soft limit at
/// that moment (raised for another caller's job before) gets it by `prlimit`
/// after the spawn, and the job's first instructions run before that: the
/// job reads the daemon's raised limit. Five tries, each must print the
/// caller's.
#[test]
fn a_caller_with_a_lower_limit_than_the_daemons_reads_its_own_from_the_first_instruction() {
    const MID: u64 = 1024;
    let (d, caller, high) = daemon_raised_by_a_running_job();
    let mut seen = Vec::new();
    for _ in 0..5 {
        let c = caller_with_soft_nofile(&d, MID, "ulimit -Sn");
        let done = wait_all(vec![("mid".into(), c)], Duration::from_secs(20));
        assert_eq!(done[0].1, 0, "{}", done[0].3);
        seen.push(done[0].2.trim().to_string());
    }
    let done = wait_all(vec![("caller".into(), caller)], Duration::from_secs(20));
    assert_eq!(done[0].2, format!("{high}\n"), "{}", done[0].3);
    assert_eq!(
        seen,
        vec![MID.to_string(); 5],
        "a caller with the soft limit {MID} read the daemon's raised {high} in its job"
    );
}

/// `website/docs/reference/environment.md`: "A request without a limit (a
/// client older than this feature) runs with the limit the daemon had when
/// it started." Story acceptance 2: "a newer daemon runs a request without
/// it as today". A job of a client with a higher limit raises the daemon's
/// own soft limit, and a reload re-executes the daemon, which inherits the
/// raised limit: the new image reads that as its start limit. The job reads
/// its limit after a pause, after the daemon's `prlimit`.
#[test]
fn after_a_reload_a_request_without_a_limit_still_gets_the_daemons_start_limit() {
    const LOW: u64 = 512;
    let (d, caller, high) = daemon_raised_by_a_running_job();
    let before = job_of_an_older_client(&d, "sleep 0.5; ulimit -Sn");
    assert_eq!(before, format!("{LOW}\n"), "before the reload: {}", d.log());

    d.reload();
    d.wait_until("the job handed over", Duration::from_secs(20), |d| {
        d.log().contains("took over 1 running")
    });
    let after = job_of_an_older_client(&d, "sleep 0.5; ulimit -Sn");

    let done = wait_all(vec![("caller".into(), caller)], Duration::from_secs(20));
    assert_eq!(
        (done[0].1, done[0].2.clone()),
        (0, format!("{high}\n")),
        "the caller's job has the caller's limit: {}",
        done[0].3
    );
    assert_eq!(
        after,
        format!("{LOW}\n"),
        "after the reload a request without a limit gets the limit the daemon raised for \
         another job, not the one it started with"
    );
}

/// A caller whose soft limit is lower than the one the daemon started with
/// reads its own in the job's first instruction (the limit is lowered in
/// the job's process by `slot __exec` before the job's command runs).
#[test]
fn a_caller_with_a_lower_limit_than_the_daemons_start_limit_reads_its_own() {
    let high = nofile_of("self", 4).min(8192);
    assert!(high > 1024, "the test needs a hard limit above 1024");
    let d = start_with_soft_nofile(high);
    let c = caller_with_soft_nofile(&d, 1000, "ulimit -Sn");
    let done = wait_all(vec![("low".into(), c)], Duration::from_secs(20));
    assert_eq!(
        (done[0].1, done[0].2.as_str()),
        (0, "1000\n"),
        "{}",
        done[0].3
    );
}

/// A job started through `slot __exec` is the job's own process: the
/// daemon's child runs the job's argv in its own process group, its record
/// keeps that argv, and when its client goes the whole tree ends.
#[test]
fn a_job_started_with_its_callers_limit_is_its_own_process_and_ends_with_its_tree() {
    let script = "sleep 30 & echo $!; wait";
    let d = start_with_soft_nofile(2048);
    let mut c = caller_with_soft_nofile(&d, 1000, script);
    let mut out = BufReader::new(c.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let grandchild = line.trim().to_string();
    assert!(!grandchild.is_empty(), "{}", d.log());

    // The daemon's children: the job, and what the daemon itself runs
    // (its samplers); none of them is `slot __exec` any more.
    let children: Vec<(String, Vec<u8>)> = job_pids(&d)
        .into_iter()
        .filter_map(|p| {
            let line = std::fs::read(format!("/proc/{p}/cmdline")).ok()?;
            Some((p, line))
        })
        .collect();
    let shown = |l: &[u8]| String::from_utf8_lossy(l).replace('\0', " ");
    assert!(
        children
            .iter()
            .all(|(_, l)| !l.windows(6).any(|w| w == b"__exec")),
        "{:?}",
        children.iter().map(|(_, l)| shown(l)).collect::<Vec<_>>()
    );
    let own = format!("sh\0-c\0{script}\0").into_bytes();
    let root = &children
        .iter()
        .find(|(_, l)| *l == own)
        .unwrap_or_else(|| {
            panic!(
                "no child of the daemon runs the job's own argv: {:?}",
                children.iter().map(|(_, l)| shown(l)).collect::<Vec<_>>()
            )
        })
        .0;
    let stat = std::fs::read_to_string(format!("/proc/{root}/stat")).unwrap();
    let after_comm = &stat[stat.rfind(')').unwrap() + 2..];
    let pgrp = after_comm.split_whitespace().nth(2).unwrap();
    assert_eq!(pgrp, root, "the job leads its own process group");
    assert_eq!(soft_nofile_if_alive(&grandchild), Some(1000));
    assert_eq!(nofile_of(&d.child.id().to_string(), 3), 2048);

    c.kill().unwrap();
    let _ = c.wait();
    let gone = |pid: &str| {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map_or(true, |s| s[s.rfind(')').unwrap() + 2..].starts_with('Z'))
    };
    d.wait_until("the job's tree to end", Duration::from_secs(20), |_| {
        gone(root) && gone(&grandchild)
    });
    d.wait_until("the job's record", Duration::from_secs(20), |d| {
        !d.logged("sleep 30").is_empty()
    });
    let log = std::fs::read_to_string(d.state().join("jobs.jsonl")).unwrap();
    let rec: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(
        rec["argv"],
        serde_json::json!(["sh", "-c", script]),
        "{log}"
    );
}
