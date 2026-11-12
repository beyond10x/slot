//! A client that gets a signal cancels its job and never sends it again,
//! whether the job was running or still queued.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SLOT: &str = env!("CARGO_BIN_EXE_slot");

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let child = Command::new(SLOT)
            .args([
                "serve",
                "--http",
                "off",
                "--tokens",
                "1",
                "--cpu-quota",
                "false",
            ])
            .args(["--call-headroom", "0", "--sccache-watch", "false"])
            .args(["--min-free-disk", "0", "--min-free-memory", "0"])
            .arg("--socket")
            .arg(dir.path().join("sock"))
            .arg("--state-dir")
            .arg(dir.path().join("state"))
            .env_remove("SLOT_JOB")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let d = Self { child, dir };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !d.socket().exists() {
            assert!(Instant::now() < deadline, "daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        d
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("sock")
    }

    fn client(&self, args: &[&str]) -> Command {
        let mut c = Command::new(args[0]);
        c.args(&args[1..])
            .env("SLOT_SOCKET", self.socket())
            .env_remove("SLOT_JOB")
            .env_remove("SLOT_HOST")
            .current_dir(self.dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        c
    }

    /// Jobs in the log whose command is `argv`, once none is active.
    fn logged(&self, argv: &[&str]) -> usize {
        let log = self.dir.path().join("state/jobs.jsonl");
        let want = serde_like(argv);
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains(&want))
            .count()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `"argv":["a","b"]` as the job log writes it.
fn serde_like(argv: &[&str]) -> String {
    let quoted: Vec<String> = argv.iter().map(|a| format!("\"{a}\"")).collect();
    format!("\"argv\":[{}]", quoted.join(","))
}

fn has_timeout() -> bool {
    Command::new("timeout")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn wait_for(pred: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn a_running_job_is_cancelled_once_on_sigterm() {
    if !has_timeout() {
        return;
    }
    let d = Daemon::start();
    let out = d
        .client(&["timeout", "2", SLOT, "sleep", "30"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(124));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("resubmitting"), "{err}");
    assert!(
        wait_for(|| d.logged(&["sleep", "30"]) == 1),
        "job not logged once"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(d.logged(&["sleep", "30"]), 1);
}

/// The client used to take the closed connection for a daemon upgrade and
/// send the job again; 0.5.1 did so for 18 of 60 queued jobs. Ten tries
/// with each signal make a pass on that code unlikely.
#[test]
fn a_queued_job_is_not_sent_again_on_a_signal() {
    if !has_timeout() {
        return;
    }
    let d = Daemon::start();
    let mut blocker = d.client(&[SLOT, "sleep", "60"]).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let mut tries = Vec::new();
    for i in 0..10 {
        for sig in ["TERM", "INT"] {
            let secs = format!("{}", 100 + tries.len());
            let out = d
                .client(&["timeout", "-s", sig, "0.3", SLOT, "sleep", &secs])
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(124), "try {i} SIG{sig}");
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(!err.contains("resubmitting"), "try {i} SIG{sig}: {err}");
            tries.push(secs);
        }
    }
    let _ = blocker.kill();
    let _ = blocker.wait();
    assert!(
        wait_for(|| d.logged(&["sleep", "60"]) == 1),
        "blocker not logged"
    );
    std::thread::sleep(Duration::from_millis(500));
    for secs in &tries {
        assert_eq!(d.logged(&["sleep", secs]), 1, "sleep {secs}");
    }
}
