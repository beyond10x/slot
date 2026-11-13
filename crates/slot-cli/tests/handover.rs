//! `systemctl --user reload slot` with jobs running: the daemon re-executes
//! at once and the new process keeps every running and queued job, its
//! output, its slots and its client (`docs/design.md` § 15.2).

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::{Read, Write};
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
        let dir = tempfile::tempdir().unwrap();
        let log = std::fs::File::create(dir.path().join("daemon.log")).unwrap();
        let mut cmd = Command::new(SLOT);
        cmd.args(["serve", "--http", "off", "--cpu-quota", "false"])
            .args(["--tokens", &tokens.to_string()])
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
    assert!(log.contains("took over 20 running and 20 queued"), "{log}");
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
