//! What a remote job's records say of where its time went: the node's job
//! log, and the file a daemon that handed the job to a node reads
//! (`SLOT_PHASES_FILE`), both from one remote `cargo test`.

use std::process::{Command, Stdio};

use serde_json::Value;

use super::{Daemon, SLOT, forced_ssh, logging_cargo, node_target, tiny_crate};

#[test]
fn a_remote_cargo_tests_records_name_its_sync_phases_and_seed() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = tiny_crate(t.path(), "");
    let ssh = forced_ssh(t.path(), &d, "");
    let cargo = logging_cargo(t.path());
    let report = t.path().join("phases.json");
    let out = Command::new(SLOT)
        .args([cargo.to_str().unwrap(), "test"])
        .current_dir(&repo)
        .env("SLOT_HOST", "node")
        .env("SLOT_SSH", &ssh)
        .env("SLOT_WSROOT", d.dir.path().join("state/ws"))
        .env("SLOT_REMOTE", SLOT)
        .env("SLOT_SOCKET", d.socket())
        .env("SLOT_SYNC_STALL", "20s")
        .env("SLOT_SYNC_LIMIT", "50s")
        .env("SLOT_PHASES_FILE", &report)
        .env_remove("SLOT_JOB")
        .env_remove("SLOT_REMOTE_SAME_PATH")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_NET_OFFLINE")
        .env("CARGO_TERM_COLOR", "never")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");

    let log = std::fs::read_to_string(d.dir.path().join("state/jobs.jsonl")).unwrap();
    let lines: Vec<Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let jobs: Vec<&Value> = lines.iter().filter(|l| l.get("argv").is_some()).collect();
    assert_eq!(jobs.len(), 1, "{log}");
    let job = jobs[0];
    assert!(job["sync_ms"].is_u64(), "{job}");
    assert!(job["sync_bytes"].as_u64().is_some_and(|b| b > 0), "{job}");
    let compile = job["compile_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("{job}"));
    let test = job["test_ms"].as_u64().unwrap_or_else(|| panic!("{job}"));
    let ran = job["ended_ms"].as_u64().unwrap() - job["started_ms"].as_u64().unwrap();
    assert_eq!(compile + test, ran, "{job}");
    let target = node_target(&d);
    assert_eq!(job["node_target_dir"], target.to_str().unwrap(), "{job}");
    assert_eq!(job["seeded_from"], "empty", "{job}");
    assert_eq!(
        job["back_ms"],
        Value::Null,
        "the job line is written as the job ends"
    );
    let amends: Vec<&Value> = lines.iter().filter(|l| l.get("amend").is_some()).collect();
    assert_eq!(amends.len(), 1, "{log}");
    assert_eq!(amends[0]["amend"], job["id"], "{log}");
    let back = amends[0]["back_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("{log}"));

    let r: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(r["sync_ms"], job["sync_ms"], "{r}");
    assert_eq!(r["sync_bytes"], job["sync_bytes"], "{r}");
    assert_eq!(r["back_ms"].as_u64(), Some(back), "{r}");
    assert_eq!(r["node_target_dir"], job["node_target_dir"], "{r}");
    assert_eq!(r["seeded_from"], "empty", "{r}");
    eprintln!(
        "sync {} ms, compile {compile} ms, test {test} ms, back {back} ms",
        job["sync_ms"]
    );
}
