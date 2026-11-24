//! Remote nodes of `cluster.toml`: the endpoint polls each every 5 s with
//! the read-only `state` step of `slot shell` and shows the node's row and
//! its jobs beside its own (`GET /api/nodes`, `GET /api/state`, job events).
//!
//! A node is Joining until its first answer, Ready while it answers, and
//! Gone with reason Lost after three missed polls in a row; an answer
//! after that makes it Ready again. A poll that fails or takes longer than
//! 4 s is a miss; the node keeps the last row it sent.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use super::{Server, now_ms};
use crate::hostconfig::RemoteNode;
use crate::types::{
    Event, GoneReason, Job, JobState, LOCAL_NODE, Node, NodeKind, NodeState, State,
};

/// Interval between two polls of one node.
pub const POLL_EVERY: Duration = Duration::from_secs(5);

/// The longest one poll may take.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(4);

/// Missed polls in a row after which a node is Gone.
const MISSES_GONE: u32 = 3;

/// Finished jobs the `state` step reports, newest.
const DONE_SENT: usize = 30;

/// Remote ids of one node lie in their own range below 0, so they never
/// meet a local id or another node's.
const ID_RANGE: i64 = 1_000_000_000_000;

/// What the `state` step prints: the node daemon's own row and its jobs,
/// as its web API answers them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Answer {
    /// The slot version of the step.
    pub slot_version: String,
    /// The node daemon's clock, Unix milliseconds.
    pub now_ms: i64,
    /// The node daemon's local row of `GET /api/nodes`.
    pub node: Option<serde_json::Value>,
    /// Queued and running jobs.
    pub active: Vec<serde_json::Value>,
    /// Recently finished jobs, oldest first.
    pub done: Vec<serde_json::Value>,
}

/// The `state` step on the node: reads the node daemon's web address
/// `http` and prints one JSON object.
pub fn state_step(http: &str) -> Result<String> {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&crate::status::get(http, "/api/nodes")?).context("/api/nodes")?;
    let node = rows
        .into_iter()
        .find(|r| r.get("node_id").and_then(|v| v.as_str()) == Some(LOCAL_NODE));
    let mut st: serde_json::Value =
        serde_json::from_str(&crate::status::get(http, "/api/state")?).context("/api/state")?;
    let take =
        |st: &mut serde_json::Value, k: &str| match st.get_mut(k).map(serde_json::Value::take) {
            Some(serde_json::Value::Array(a)) => a,
            _ => Vec::new(),
        };
    let active = take(&mut st, "active");
    let mut done = take(&mut st, "done");
    done.drain(..done.len().saturating_sub(DONE_SENT));
    let a = Answer {
        slot_version: env!("CARGO_PKG_VERSION").to_string(),
        now_ms: st
            .get("now_ms")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        node,
        active,
        done,
    };
    Ok(serde_json::to_string(&a)?)
}

/// One remote node as the endpoint knows it.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    pub(super) conf: RemoteNode,
    /// Its place in the file, for the id range of its jobs.
    index: usize,
    pub(super) row: Node,
    misses: u32,
    active: Vec<Job>,
    done: Vec<Job>,
    /// Whether an answer came since the start.
    seen: bool,
    /// When the daemon handed the node a local job, Unix ms; see
    /// [`super::spill`].
    pub(super) handed: Vec<i64>,
}

/// The remote nodes of the host configuration.
#[derive(Debug, Default)]
pub struct Remotes {
    pub(super) nodes: Mutex<Vec<Entry>>,
    /// The spill settings of the file.
    pub spill: crate::hostconfig::SpillConf,
}

impl Remotes {
    /// The nodes of `cluster.toml` at `path`, each Joining; none for no
    /// path or no file. A file that does not parse is reported and gives
    /// none.
    pub fn load(path: Option<&Path>, cluster: &str) -> Self {
        let (confs, spill) = match path.map(crate::hostconfig::load_all) {
            None => Default::default(),
            Some(Ok(c)) => c,
            Some(Err(e)) => {
                eprintln!("slot: cluster config: {e}");
                Default::default()
            }
        };
        let mut r = Self::new(confs, cluster);
        r.spill = spill;
        r
    }

    /// The nodes `confs`, each Joining.
    pub fn new(confs: Vec<RemoteNode>, cluster: &str) -> Self {
        let nodes = confs
            .into_iter()
            .enumerate()
            .map(|(index, conf)| {
                let mut row = Node {
                    state: NodeState::Joining,
                    ..Node::default()
                };
                configure(&mut row, &conf, cluster);
                Entry {
                    conf,
                    index,
                    row,
                    misses: 0,
                    active: Vec::new(),
                    done: Vec::new(),
                    seen: false,
                    handed: Vec::new(),
                }
            })
            .collect();
        Self {
            nodes: Mutex::new(nodes),
            spill: crate::hostconfig::SpillConf::default(),
        }
    }

    fn is_empty(&self) -> bool {
        self.nodes.lock().expect("nodes lock").is_empty()
    }

    /// One row per remote node.
    pub fn rows(&self) -> Vec<Node> {
        let g = self.nodes.lock().expect("nodes lock");
        g.iter().map(|e| e.row.clone()).collect()
    }

    /// Adds the remote nodes' jobs to a state snapshot: active jobs of a
    /// node that is not Gone, and finished ones, oldest first.
    pub fn merge_jobs(&self, st: &mut State) {
        let g = self.nodes.lock().expect("nodes lock");
        for e in g.iter() {
            if e.row.state != NodeState::Gone {
                st.active.extend(e.active.iter().cloned());
            }
            st.done.extend(e.done.iter().cloned());
        }
        st.done.sort_by_key(|j| (j.ended_ms, j.id));
    }

    /// Takes a poll's outcome for the node at `at`: the jobs that are new or
    /// changed since the last answer (none on the first answer but the
    /// active ones).
    fn record(
        &self,
        at: usize,
        outcome: Result<Answer, String>,
        now: i64,
        cluster: &str,
    ) -> Vec<Job> {
        let mut nodes = self.nodes.lock().expect("nodes lock");
        let Some(e) = nodes.get_mut(at) else {
            return Vec::new();
        };
        match outcome {
            Err(why) => {
                e.misses += 1;
                if e.misses == 1 || e.misses == MISSES_GONE {
                    eprintln!("slot: node {}: poll missed ({why})", e.conf.name);
                }
                if e.misses >= MISSES_GONE && e.row.state != NodeState::Gone {
                    e.row.state = NodeState::Gone;
                    e.row.gone_reason = Some(GoneReason::Lost);
                    e.row.left_at = Some(crate::types::rfc3339(now));
                    e.row.running_jobs = 0;
                    e.row.queued_jobs = 0;
                    e.row.slots_in_use = 0;
                }
                Vec::new()
            }
            Ok(a) => {
                let mut row: Node = a
                    .node
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                if row.slot_version.is_empty() {
                    row.slot_version = a.slot_version;
                }
                configure(&mut row, &e.conf, cluster);
                row.state = NodeState::Ready;
                if row.slots_in_use > row.slots {
                    eprintln!(
                        "slot: node {}: {} slots in use of {}; not taken",
                        e.conf.name, row.slots_in_use, row.slots
                    );
                    row.slots_in_use = e.row.slots_in_use.min(row.slots);
                }
                let (name, index) = (e.conf.name.clone(), e.index);
                let jobs = |v: Vec<serde_json::Value>| -> Vec<Job> {
                    v.into_iter()
                        .filter_map(|j| match serde_json::from_value::<Job>(j) {
                            Ok(j) => Some(j),
                            Err(err) => {
                                eprintln!("slot: node {name}: a job not read: {err}");
                                None
                            }
                        })
                        .map(|j| on_node(j, &name, index))
                        .collect()
                };
                let active = jobs(a.active);
                let done = jobs(a.done);
                let before: HashMap<i64, JobState> = e
                    .active
                    .iter()
                    .chain(&e.done)
                    .map(|j| (j.id, j.state))
                    .collect();
                let changed = active
                    .iter()
                    .chain(&done)
                    .filter(|j| match before.get(&j.id) {
                        Some(s) => *s != j.state,
                        None => e.seen || active.iter().any(|a| a.id == j.id),
                    })
                    .cloned()
                    .collect();
                e.row = row;
                e.misses = 0;
                e.active = active;
                e.done = done;
                e.seen = true;
                changed
            }
        }
    }
}

/// Sets what the endpoint, not the node, says of a remote node.
fn configure(row: &mut Node, conf: &RemoteNode, cluster: &str) {
    let t = &conf.table;
    row.node_id.clone_from(&conf.name);
    row.cluster_id = cluster.to_string();
    row.kind = NodeKind::Remote;
    row.gone_reason = None;
    row.left_at = None;
    if t.deadline.is_some() {
        row.deadline.clone_from(&t.deadline);
    }
    if t.price_per_hour_cents.is_some() {
        row.price_per_hour_cents = t.price_per_hour_cents;
    }
    if t.location.is_some() {
        row.location.clone_from(&t.location);
    }
    if t.machine_type.is_some() {
        row.machine_type.clone_from(&t.machine_type);
    }
}

/// A job of node `name` as the endpoint shows it: its id moved into the
/// node's range, its node named.
fn on_node(mut j: Job, name: &str, index: usize) -> Job {
    j.id = remote_job_id(index, j.id);
    j.node = name.to_string();
    j
}

/// The endpoint's id of job `id` of the node at `index` of the file.
pub fn remote_job_id(index: usize, id: i64) -> i64 {
    -((index as i64 + 1) * ID_RANGE + id.rem_euclid(ID_RANGE))
}

/// Runs `argv` and parses its output as an [`Answer`], within `timeout`.
async fn ask(argv: &[String], timeout: Duration) -> Result<Answer, String> {
    let (prog, args) = argv.split_first().ok_or("empty command")?;
    let child = tokio::process::Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("no answer within {} s", timeout.as_secs()))?
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "{}: {}",
            out.status,
            err.lines().last().unwrap_or("").trim()
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("answer: {e}"))
}

impl Server {
    /// The `GET /api/state` snapshot: the local pool's, with the remote
    /// nodes' jobs.
    pub fn state(&self) -> State {
        let mut st = self.local_state();
        self.remotes.merge_jobs(&mut st);
        st
    }

    /// The `GET /api/nodes` answer: the local node, then each remote node.
    pub fn nodes(&self) -> Vec<Node> {
        let mut v = vec![self.local_node()];
        v.extend(self.remotes.rows());
        v
    }

    /// Polls every remote node every [`POLL_EVERY`]; publishes each job of
    /// a node that is new or changed.
    pub(super) async fn node_poll(self: Arc<Self>) {
        if self.remotes.is_empty() {
            return;
        }
        let mut tick = tokio::time::interval(POLL_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let argvs: Vec<Vec<String>> = {
                let g = self.remotes.nodes.lock().expect("nodes lock");
                g.iter().map(|e| e.conf.state_command()).collect()
            };
            let polls = argvs.into_iter().enumerate().map(|(i, argv)| {
                let s = Arc::clone(&self);
                tokio::spawn(async move {
                    let outcome = ask(&argv, POLL_TIMEOUT).await;
                    for job in s.remotes.record(i, outcome, now_ms(), &s.host) {
                        s.send(&Event::Job { job });
                    }
                })
            });
            for p in polls.collect::<Vec<_>>() {
                let _ = p.await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hostconfig::parse;

    fn remotes(text: &str) -> Remotes {
        Remotes::new(parse(text).unwrap(), "endpoint")
    }

    fn job(id: i64, state: JobState) -> serde_json::Value {
        serde_json::json!({
            "id": id, "argv": ["cargo", "build"], "cwd": "/w", "repo": "repo-a",
            "toolchain": "cargo", "remote": false, "tokens": 4, "state": state,
            "enqueued_ms": 1, "started_ms": 2, "ended_ms": if state == JobState::Running { 0 } else { 3 },
            "exit": 0, "cpu_s": 0.0, "compile_server_cpu_s": 0.0, "peak_rss": 0, "target_dir": "", "node": "local",
        })
    }

    fn answer(active: Vec<serde_json::Value>, done: Vec<serde_json::Value>) -> Answer {
        Answer {
            slot_version: "9.9.9".into(),
            now_ms: 1,
            node: Some(serde_json::json!({
                "node_id": "local", "cluster_id": "node-host", "kind": "Local", "state": "Ready",
                "cpus": 64, "slots": 32, "slots_in_use": 4, "memory_gib": 128, "disk_gib": 900,
                "running_jobs": 1, "queued_jobs": 0, "slot_version": "9.9.9", "cpu_busy_milli": 3500,
            })),
            active,
            done,
        }
    }

    const FILE: &str = "[remote.build-1]\nssh = \"ssh build-1\"\nprice_per_hour_cents = 250\ndeadline = \"2030-01-01T00:00:00Z\"\n";

    #[test]
    fn a_node_joins_answers_and_is_gone_after_three_misses() {
        let r = remotes(FILE);
        let rows = r.rows();
        assert_eq!(rows[0].node_id, "build-1");
        assert_eq!(rows[0].kind, NodeKind::Remote);
        assert_eq!(rows[0].state, NodeState::Joining);
        assert_eq!(rows[0].price_per_hour_cents, Some(250));

        let changed = r.record(
            0,
            Ok(answer(
                vec![job(7, JobState::Running)],
                vec![job(5, JobState::Ok)],
            )),
            10,
            "endpoint",
        );
        let row = &r.rows()[0];
        assert_eq!(
            (
                row.state,
                row.cpus,
                row.slots,
                row.slots_in_use,
                row.cpu_busy_milli
            ),
            (NodeState::Ready, 64, 32, 4, Some(3500))
        );
        assert_eq!(
            (row.node_id.as_str(), row.kind, row.cluster_id.as_str()),
            ("build-1", NodeKind::Remote, "endpoint")
        );
        assert_eq!(row.deadline.as_deref(), Some("2030-01-01T00:00:00Z"));
        // The first answer publishes the running job, not the old ones.
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].id, remote_job_id(0, 7));
        assert_eq!(changed[0].node, "build-1");

        // The running job finished: published once.
        let changed = r.record(
            0,
            Ok(answer(
                vec![],
                vec![job(5, JobState::Ok), job(7, JobState::Ok)],
            )),
            20,
            "endpoint",
        );
        assert_eq!(
            changed.iter().map(|j| j.id).collect::<Vec<_>>(),
            [remote_job_id(0, 7)]
        );

        for n in 1..=3 {
            r.record(0, Err("timeout".into()), 30 + n, "endpoint");
        }
        let row = &r.rows()[0];
        assert_eq!(
            (row.state, row.gone_reason, row.cpus),
            (NodeState::Gone, Some(GoneReason::Lost), 64)
        );
        r.record(0, Ok(answer(vec![], vec![])), 40, "endpoint");
        assert_eq!(r.rows()[0].state, NodeState::Ready);
        assert_eq!(r.rows()[0].gone_reason, None);
    }

    #[test]
    fn remote_ids_meet_no_local_id_and_no_other_node() {
        assert!(remote_job_id(0, 1) < 0);
        assert_ne!(remote_job_id(0, 1), remote_job_id(1, 1));
        assert_ne!(remote_job_id(0, 2), remote_job_id(0, 1));
    }

    #[test]
    fn state_gets_the_remote_jobs() {
        let r = remotes(FILE);
        r.record(
            0,
            Ok(answer(
                vec![job(7, JobState::Running)],
                vec![job(5, JobState::Ok)],
            )),
            10,
            "endpoint",
        );
        let mut st: State = serde_json::from_value(serde_json::json!({
            "host": "h", "cores": 1, "tokens": 1, "free": 1, "active": [], "done": [],
            "now_ms": 1, "toolchains": [],
        }))
        .unwrap();
        r.merge_jobs(&mut st);
        assert_eq!(st.active.len(), 1);
        assert_eq!(st.active[0].node, "build-1");
        assert_eq!(st.done.len(), 1);
    }

    #[test]
    fn more_slots_in_use_than_slots_is_not_taken() {
        let r = remotes(FILE);
        let mut a = answer(vec![], vec![]);
        a.node.as_mut().unwrap()["slots_in_use"] = 99.into();
        r.record(0, Ok(a), 10, "endpoint");
        assert_eq!(r.rows()[0].slots_in_use, 0);
    }

    #[tokio::test]
    async fn a_poll_runs_the_command_and_times_out() {
        let ok = ask(
            &[
                "echo".into(),
                serde_json::to_string(&answer(vec![], vec![])).unwrap(),
            ],
            POLL_TIMEOUT,
        )
        .await;
        assert_eq!(ok.unwrap().slot_version, "9.9.9");
        let start = std::time::Instant::now();
        let slow = ask(&["sleep".into(), "5".into()], Duration::from_millis(300)).await;
        assert!(slow.unwrap_err().contains("no answer"));
        assert!(start.elapsed() < Duration::from_secs(2));
        let failed = ask(&["false".into()], POLL_TIMEOUT).await;
        assert!(failed.is_err());
    }
}
