//! What an endpoint shows of a remote node, through the node's forced
//! command: the node's own counts and machine facts in its row, and the
//! output of a node's job read on request. Two daemons of the test's own,
//! the node reached through a fake `ssh`. No network but the loopback.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::{SLOT, script};

/// A daemon with its web view on a free loopback port and the cluster
/// file `cluster`.
struct Web {
    child: Child,
    http: String,
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

fn serve(dir: &Path, cluster: &str) -> Web {
    let http = free_port();
    let child = Command::new(SLOT)
        .env("SLOT_CLUSTER_FILE", cluster)
        .args(["serve", "--http", &http, "--tokens", "1"])
        .args(["--cpu-quota", "false", "--call-headroom", "0"])
        .args(["--sccache-watch", "false"])
        .args(["--min-free-disk", "0", "--min-free-memory", "0"])
        .arg("--socket")
        .arg(dir.join("sock"))
        .arg("--state-dir")
        .arg(dir.join("state"))
        .env_remove("SLOT_JOB")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let w = Web { child, http };
    assert!(
        until(|| get(&w.http, "/api/state").is_some()),
        "daemon did not start"
    );
    w
}

/// The body of `GET path`, read to its end; none for an answer that is not
/// 200.
fn get(addr: &str, path: &str) -> Option<String> {
    let mut s = TcpStream::connect(addr).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(20))).ok()?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").ok()?;
    let mut raw = String::new();
    s.read_to_string(&mut raw).ok()?;
    let (head, body) = raw.split_once("\r\n\r\n")?;
    head.starts_with("HTTP/1.1 200")
        .then(|| body.to_string())
        .or_else(|| head.starts_with("HTTP/1.0 200").then(|| body.to_string()))
}

fn until(pred: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn an_endpoint_shows_a_nodes_counts_and_the_output_of_its_job() {
    let t = tempfile::tempdir().unwrap();
    let (nd, hd) = (t.path().join("node"), t.path().join("hub"));
    std::fs::create_dir_all(&nd).unwrap();
    std::fs::create_dir_all(&hd).unwrap();
    let node = serve(&nd, "off");
    let ran = Command::new(SLOT)
        .args(["sh", "-c", "echo hello-from-node; exit 127"])
        .env("SLOT_SOCKET", nd.join("sock"))
        .env_remove("SLOT_JOB")
        .env_remove("SLOT_HOST")
        .current_dir(&nd)
        .output()
        .unwrap();
    assert_eq!(ran.status.code(), Some(127));

    let ssh = t.path().join("ssh");
    script(
        &ssh,
        &format!(
            "shift\nSSH_ORIGINAL_COMMAND=\"$*\" exec {SLOT} shell --socket {sock} --state-dir {state} --http {http}",
            sock = nd.join("sock").display(),
            state = nd.join("state").display(),
            http = node.http,
        ),
    );
    let cluster = t.path().join("cluster.toml");
    std::fs::write(
        &cluster,
        format!(
            "[remote.node-a]\nssh = \"{} node-a\"\nslot_path = \"{SLOT}\"\ndeadline = \"2030-01-01T00:00:00Z\"\n",
            ssh.display()
        ),
    )
    .unwrap();
    let hub = serve(&hd, cluster.to_str().unwrap());

    let row = || -> Option<serde_json::Value> {
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(&get(&hub.http, "/api/nodes")?).ok()?;
        rows.into_iter()
            .find(|r| r["node_id"] == "node-a" && r["state"] == "Ready")
    };
    assert!(until(|| row().is_some()), "the node did not answer a poll");
    let r = row().unwrap();
    assert_eq!(r["ended_hour"]["infra"], 1, "the node's own count: {r}");
    assert!(r["booted_at"].is_string(), "the node's boot: {r}");
    assert!(
        r["host_cpu_milli"].is_number(),
        "the node's whole host: {r}"
    );

    let out = get(&hub.http, "/api/jobs/1/output?node=node-a").expect("output of the node's job");
    assert!(out.contains("hello-from-node"), "{out}");
    assert!(out.contains("event: end"), "{out}");
    assert!(get(&hub.http, "/api/jobs/1/output?node=node-b").is_none());
}
