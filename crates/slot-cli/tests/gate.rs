//! `slot gate`: a client that cannot reach the daemon's socket queues
//! through FIFOs, holds its slots through `slot hold`, and runs its command
//! itself (`docs/design.md` § 19).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const SLOT: &str = env!("CARGO_BIN_EXE_slot");

struct Setup {
    daemon: Child,
    gate: Child,
    dir: tempfile::TempDir,
    http: SocketAddr,
}

impl Setup {
    fn start(tokens: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bind = TcpListener::bind("127.0.0.1:0").unwrap();
        let http = bind.local_addr().unwrap();
        drop(bind);
        let log = std::fs::File::create(dir.path().join("daemon.log")).unwrap();
        let daemon = Command::new(SLOT)
            .args(["serve", "--http", &http.to_string(), "--cpu-quota", "false"])
            .args(["--tokens", &tokens.to_string()])
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
        wait_for(&dir.path().join("sock"));
        let log = std::fs::File::create(dir.path().join("gate.log")).unwrap();
        let gate = Command::new(SLOT)
            .arg("gate")
            .arg("--dir")
            .arg(dir.path().join("gate"))
            .arg("--socket")
            .arg(dir.path().join("sock"))
            .arg("--state-dir")
            .arg(dir.path().join("state"))
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let s = Self {
            daemon,
            gate,
            dir,
            http,
        };
        // The FIFO exists before the gate opens it: wait for the open.
        let fifo = s.gate_dir().join("requests.fifo");
        until(&format!("the gate to open {}", fifo.display()), || {
            opened_by(s.gate.id(), &fifo)
        });
        s
    }

    /// Jobs of the daemon in `state`, read from its web API.
    fn jobs_in(&self, state: &str) -> usize {
        let Ok(mut c) = TcpStream::connect_timeout(&self.http, Duration::from_secs(1)) else {
            return 0;
        };
        let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
        let req = format!(
            "GET /api/state HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.http
        );
        if c.write_all(req.as_bytes()).is_err() {
            return 0;
        }
        let mut body = String::new();
        let _ = c.read_to_string(&mut body);
        body.matches(&format!(r#""state":"{state}""#)).count()
    }

    fn gate_dir(&self) -> PathBuf {
        self.dir.path().join("gate")
    }

    /// A client as a sandbox runs it: `SLOT_GATE` set, the socket out of
    /// reach.
    fn client(&self, script: &str) -> Child {
        Command::new(SLOT)
            .args(["sh", "-c", script])
            .env("SLOT_GATE", self.gate_dir())
            .env("SLOT_SOCKET", self.dir.path().join("no-such-socket"))
            .env_remove("SLOT_JOB")
            .env_remove("SLOT_HOST")
            .current_dir(self.dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn jobs(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("state").join("jobs.jsonl"))
            .unwrap_or_default()
    }

    fn logs(&self) -> String {
        let r = |n: &str| std::fs::read_to_string(self.dir.path().join(n)).unwrap_or_default();
        format!("daemon:\n{}\ngate:\n{}", r("daemon.log"), r("gate.log"))
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = self.gate.kill();
        let _ = self.daemon.kill();
        let _ = self.gate.wait();
        let _ = self.daemon.wait();
    }
}

/// Polls `ok` until it holds, at most 10 s.
fn until(what: &str, ok: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < deadline, "{what} within 10 s");
        // The poll interval of a bounded wait.
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for(p: &Path) {
    until(&format!("{} to appear", p.display()), || p.exists());
}

/// Whether process `pid` has `path` open.
fn opened_by(pid: u32, path: &Path) -> bool {
    std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|d| {
        d.flatten()
            .any(|f| std::fs::read_link(f.path()).is_ok_and(|l| l == path))
    })
}

fn finish(c: Child) -> Output {
    c.wait_with_output().unwrap()
}

#[test]
fn a_gated_job_runs_in_the_client_with_the_pool_jobserver() {
    let s = Setup::start(2);
    let c = s.client("echo auth=$CARGO_MAKEFLAGS; echo job=$SLOT_JOB; echo parent=$PPID; exit 3");
    let client_pid = c.id();
    let out = finish(c);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{stdout}\n{stderr}\n{}",
        s.logs()
    );
    let fifo = s.gate_dir().join("jobserver.fifo");
    assert!(
        stdout.contains(&format!("auth=-j --jobserver-auth=fifo:{}", fifo.display())),
        "{stdout}"
    );
    assert!(stdout.contains("job=1"), "{stdout}");
    // The command is the client's child, not the daemon's.
    assert!(stdout.contains(&format!("parent={client_pid}")), "{stdout}");
    // The job is booked under the client's command, with its exit code.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !s.jobs().contains("\"exit\":3") {
        assert!(Instant::now() < deadline, "{}\n{}", s.jobs(), s.logs());
        // The poll interval of a bounded wait.
        std::thread::sleep(Duration::from_millis(50));
    }
    let jobs = s.jobs();
    assert!(jobs.contains(r#""argv":["sh","-c","#), "{jobs}");
    // Nothing of the request is left behind.
    let left: Vec<String> = std::fs::read_dir(s.gate_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "jobserver.fifo" && n != "requests.fifo")
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_gated_job_holds_its_slot_until_the_client_ends() {
    let s = Setup::start(1);
    let started = s.dir.path().join("first-started");
    let go = s.dir.path().join("go");
    let marker = s.dir.path().join("first-done");
    // The first job polls for the file `go` the test writes.
    let first = s.client(&format!(
        ": > {}; while [ ! -e {} ]; do sleep 0.02; done; : > {}",
        started.display(),
        go.display(),
        marker.display()
    ));
    wait_for(&started);
    // The pool has one slot: the second job starts only after the first.
    let second = s.client(&format!(
        "test -e {} && echo after || echo overlap",
        marker.display()
    ));
    until("the second job to queue behind the first", || {
        s.jobs_in("queued") == 1
    });
    std::fs::write(&go, "").unwrap();
    let a = finish(first);
    let b = finish(second);
    assert_eq!(a.status.code(), Some(0), "{}", s.logs());
    let out = String::from_utf8_lossy(&b.stdout);
    assert!(out.contains("after"), "{out}\n{}", s.logs());
}

#[test]
fn a_killed_client_frees_its_slot() {
    let s = Setup::start(1);
    let started = s.dir.path().join("first-started");
    // `sleep 5` is the job's run; the test kills its client.
    let mut first = s.client(&format!(": > {}; sleep 5", started.display()));
    wait_for(&started);
    first.kill().unwrap();
    let _ = first.wait();
    let t = Instant::now();
    let out = finish(s.client("echo ran"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("ran"));
    assert!(t.elapsed() < Duration::from_secs(10), "{}", s.logs());
}

#[test]
fn without_a_gate_the_client_runs_outside_the_pool_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(SLOT)
        .args(["sh", "-c", "echo here"])
        .env("SLOT_GATE", dir.path())
        .env_remove("SLOT_JOB")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("here"));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("ran outside the pool"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
