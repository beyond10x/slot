//! Remote nodes of `cluster.toml`: the endpoint polls each every 5 s with
//! the read-only `state` step of `slot shell` and shows the node's row and
//! its jobs beside its own (`GET /api/nodes`, `GET /api/state`, job events).
//!
//! A node is Joining until its first answer, Ready while it answers, and
//! Gone with reason Lost after three missed polls in a row; an answer
//! after that makes it Ready again. A poll that fails or takes longer than
//! 4 s is a miss; the node keeps the last row it sent.
//!
//! The output of a node's job is read the same way, on request: the
//! read-only `output` step of `slot shell`, every 2 s from the next chunk
//! until the job ended ([`relay_output`]).
//!
//! A node's job that an answer shows ended for the first time is appended
//! to `node-jobs.jsonl` beside `jobs.jsonl` ([`Remotes::with_log`]), with
//! `node` set to the node's name and the id the node gave it, so `slot
//! usage` counts the jobs the nodes ran.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead as _, Write as _};
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
        slot_version: env!("SLOT_VERSION").to_string(),
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

impl Entry {
    /// Node `conf` before its first answer.
    fn joining(conf: RemoteNode, cluster: &str) -> Self {
        let mut row = Node {
            state: NodeState::Joining,
            ..Node::default()
        };
        configure(&mut row, &conf, cluster);
        Self {
            conf,
            row,
            misses: 0,
            active: Vec::new(),
            done: Vec::new(),
            seen: false,
            handed: Vec::new(),
        }
    }
}

/// The log of the jobs the nodes ran.
#[derive(Debug, Default)]
struct NodeLog {
    /// `node-jobs.jsonl`; `None` logs nothing.
    path: Option<std::path::PathBuf>,
    /// The file, open to append, once a job was written.
    file: Option<std::fs::File>,
    /// Node, id and start of each job written that its node may list again.
    logged: HashSet<(String, i64, i64)>,
}

/// The remote nodes of the host configuration.
#[derive(Debug, Default)]
pub struct Remotes {
    pub(super) nodes: Mutex<Vec<Entry>>,
    /// The spill settings of the file.
    pub spill: crate::hostconfig::SpillConf,
    /// The file the nodes came from and its modification time when last read.
    file: Mutex<Option<(std::path::PathBuf, Option<std::time::SystemTime>)>>,
    /// Where the jobs the nodes ran are logged.
    log: Mutex<NodeLog>,
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
        r.file = Mutex::new(path.map(|p| (p.to_path_buf(), modified(p))));
        r
    }

    /// Reads the file again when it changed since the last read: takes each
    /// known node's new terms (ssh line, deadline, price, place, machine
    /// type, spill) into its entry and its row, adds a node new in the file
    /// (Joining) and drops a node taken out of it with its jobs. A file that
    /// does not parse is reported and changes nothing.
    pub fn reread(&self, cluster: &str) {
        let mut file = self.file.lock().expect("file lock");
        let Some((path, seen)) = file.as_mut() else {
            return;
        };
        let now = modified(path);
        if now == *seen {
            return;
        }
        *seen = now;
        let confs = match crate::hostconfig::load_all(path) {
            Ok((confs, _)) => confs,
            Err(e) => {
                eprintln!("slot: cluster config: {e}");
                return;
            }
        };
        let mut nodes = self.nodes.lock().expect("nodes lock");
        let mut old: Vec<Entry> = std::mem::take(&mut *nodes);
        for c in confs {
            if let Some(i) = old.iter().position(|e| e.conf.name == c.name) {
                let mut e = old.swap_remove(i);
                e.conf = c;
                configure(&mut e.row, &e.conf, cluster);
                nodes.push(e);
            } else {
                eprintln!("slot: node {}: added", c.name);
                nodes.push(Entry::joining(c, cluster));
            }
        }
        for e in old {
            eprintln!("slot: node {}: removed", e.conf.name);
        }
    }

    /// Appends each node job that ended to `path` (`node-jobs.jsonl`) once:
    /// the jobs already in the file are not written again, also after a
    /// restart. A file that cannot be opened is reported, and nothing is
    /// logged.
    #[must_use]
    pub fn with_log(self, path: &Path) -> Self {
        let mut logged = HashSet::new();
        if let Ok(f) = std::fs::File::open(path) {
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(j) = serde_json::from_str::<Job>(&line) {
                    logged.insert((j.node, j.id, j.started_ms));
                }
            }
        }
        *self.log.lock().expect("log lock") = NodeLog {
            path: Some(path.to_path_buf()),
            file: None,
            logged,
        };
        self
    }

    /// Writes each job of `done` that ended and is not in the log yet, and
    /// keeps the keys of node `name` only for the jobs it still lists. The
    /// file is opened at the first job it gets.
    fn log_ended(&self, name: &str, done: &[Job]) {
        let mut g = self.log.lock().expect("log lock");
        let NodeLog { path, file, logged } = &mut *g;
        let Some(path) = path.as_deref() else {
            return;
        };
        for j in done.iter().filter(|j| j.ended_ms > 0) {
            if !logged.insert((name.to_string(), j.id, j.started_ms)) {
                continue;
            }
            if file.is_none() {
                *file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .inspect_err(|e| eprintln!("slot: {}: {e}", path.display()))
                    .ok();
            }
            if let Some(f) = file.as_mut()
                && let Ok(line) = serde_json::to_string(j)
                && let Err(e) = writeln!(f, "{line}")
            {
                eprintln!("slot: {}: {e}", path.display());
            }
        }
        let listed: HashSet<(i64, i64)> = done.iter().map(|j| (j.id, j.started_ms)).collect();
        logged.retain(|(n, id, start)| n != name || listed.contains(&(*id, *start)));
    }

    /// One poll of every node: rereads the file when it changed, asks each
    /// node at once, and returns the jobs that are new or changed.
    pub async fn poll(&self, cluster: &str, clock: fn() -> i64) -> Vec<Job> {
        self.reread(cluster);
        let asks: Vec<(String, Vec<String>)> = {
            let g = self.nodes.lock().expect("nodes lock");
            g.iter()
                .map(|e| (e.conf.name.clone(), e.conf.state_command()))
                .collect()
        };
        let answers =
            futures_util::future::join_all(asks.into_iter().map(|(name, argv)| async move {
                let outcome = ask(&argv, POLL_TIMEOUT).await;
                (name, outcome)
            }))
            .await;
        answers
            .into_iter()
            .flat_map(|(name, outcome)| self.record(&name, outcome, clock(), cluster))
            .collect()
    }

    /// The nodes `confs`, each Joining.
    pub fn new(confs: Vec<RemoteNode>, cluster: &str) -> Self {
        let nodes = confs
            .into_iter()
            .map(|conf| Entry::joining(conf, cluster))
            .collect();
        Self {
            nodes: Mutex::new(nodes),
            spill: crate::hostconfig::SpillConf::default(),
            file: Mutex::new(None),
            log: Mutex::new(NodeLog::default()),
        }
    }

    /// Whether no node can ever be polled: no node and no file to read
    /// one from.
    fn is_empty(&self) -> bool {
        self.file.lock().expect("file lock").is_none()
            && self.nodes.lock().expect("nodes lock").is_empty()
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

    /// Takes a poll's outcome for node `name`: the jobs that are new or
    /// changed since the last answer (none on the first answer but the
    /// active ones).
    fn record(
        &self,
        name: &str,
        outcome: Result<Answer, String>,
        now: i64,
        cluster: &str,
    ) -> Vec<Job> {
        let mut nodes = self.nodes.lock().expect("nodes lock");
        let Some(e) = nodes.iter_mut().find(|e| e.conf.name == name) else {
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
                let name = e.conf.name.clone();
                let jobs = |v: Vec<serde_json::Value>| -> Vec<Job> {
                    v.into_iter()
                        .filter_map(|j| match serde_json::from_value::<Job>(j) {
                            Ok(j) => Some(j),
                            Err(err) => {
                                eprintln!("slot: node {name}: a job not read: {err}");
                                None
                            }
                        })
                        .map(|j| on_node(j, &name))
                        .collect()
                };
                let active = jobs(a.active);
                let done = jobs(a.done);
                self.log_ended(&name, &done);
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

/// The modification time of `path`, if it can be read.
fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// A job of node `name` as the endpoint shows it: the id that node gave it,
/// the node named, and marked as a node's row, so `node` and `id` together
/// name it.
fn on_node(mut j: Job, name: &str) -> Job {
    j.node = name.to_string();
    j.from_node = true;
    j
}

/// Runs `argv` and parses its output as an [`Answer`], within `timeout`.
async fn ask(argv: &[String], timeout: Duration) -> Result<Answer, String> {
    ask_json(argv, timeout).await
}

/// The `output` step on the node: reads the node daemon's web address
/// `http` for the kept output of job `id` from chunk `from` on and prints
/// one JSON object.
pub fn output_step(http: &str, id: i64, from: u64) -> Result<String> {
    crate::status::get(http, &format!("/api/jobs/{id}/output-tail?from={from}"))
}

/// The longest one read of a node job's output may take.
pub const OUTPUT_TIMEOUT: Duration = Duration::from_secs(10);

/// Interval between two reads of a running node job's output.
pub const OUTPUT_EVERY: Duration = Duration::from_secs(2);

/// What the endpoint sends its page of a node job's output.
#[derive(Debug, Clone, PartialEq)]
pub enum Relay {
    /// A chunk of the output.
    Chunk(crate::types::Chunk),
    /// The end, with a message for the reader; empty when the job ended.
    End(String),
}

/// The output of job `id` of node `name`: reads it with `argv_for(from)`
/// (the node's `output` step from chunk `from` on) every `every` until the
/// job ended, and ends with the reason when a read fails.
pub fn relay_output(
    argv_for: impl Fn(u64) -> Vec<String> + Send + 'static,
    id: i64,
    name: String,
    every: Duration,
) -> impl futures_util::Stream<Item = Relay> + Send {
    use futures_util::StreamExt as _;
    struct At<F> {
        argv_for: F,
        from: u64,
        first: bool,
        done: bool,
    }
    let at = At {
        argv_for,
        from: 0,
        first: true,
        done: false,
    };
    futures_util::stream::unfold(at, move |mut at| {
        let name = name.clone();
        async move {
            if at.done {
                return None;
            }
            if !at.first {
                tokio::time::sleep(every).await;
            }
            at.first = false;
            let argv = (at.argv_for)(at.from);
            let out: Vec<Relay> =
                match ask_json::<crate::types::OutputTail>(&argv, OUTPUT_TIMEOUT).await {
                    Err(why) => {
                        at.done = true;
                        vec![Relay::End(format!(
                            "the output of job {id} on {name} was not read: {why}"
                        ))]
                    }
                    Ok(t) if !t.kept => {
                        at.done = true;
                        vec![Relay::End("output not kept for this job".into())]
                    }
                    Ok(t) => {
                        at.from = t.next.max(at.from);
                        let mut v: Vec<Relay> = t.chunks.into_iter().map(Relay::Chunk).collect();
                        if t.ended {
                            at.done = true;
                            v.push(Relay::End(String::new()));
                        }
                        v
                    }
                };
            Some((futures_util::stream::iter(out), at))
        }
    })
    .flatten()
}

impl Remotes {
    /// The table of node `name`; none for a node the file does not name.
    pub fn conf(&self, name: &str) -> Option<RemoteNode> {
        let g = self.nodes.lock().expect("nodes lock");
        g.iter()
            .find(|e| e.conf.name == name)
            .map(|e| e.conf.clone())
    }
}

/// Runs `argv` and parses its output as JSON, within `timeout`.
async fn ask_json<T: serde::de::DeserializeOwned>(
    argv: &[String],
    timeout: Duration,
) -> Result<T, String> {
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

    /// Polls every remote node every [`POLL_EVERY`], with the file read
    /// again when it changed; publishes each job of a node that is new or
    /// changed. Runs while there is a file, also one that names no node yet.
    pub(super) async fn node_poll(self: Arc<Self>) {
        if self.remotes.is_empty() {
            return;
        }
        let mut tick = tokio::time::interval(POLL_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            for job in self.remotes.poll(&self.host, now_ms).await {
                self.send(&Event::Job { job });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hostconfig::parse;
    use crate::types::OutputTail;
    use futures_util::StreamExt as _;

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
            "build-1",
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
        assert_eq!(changed[0].id, 7);
        assert_eq!(changed[0].node, "build-1");

        // The running job finished: published once.
        let changed = r.record(
            "build-1",
            Ok(answer(
                vec![],
                vec![job(5, JobState::Ok), job(7, JobState::Ok)],
            )),
            20,
            "endpoint",
        );
        assert_eq!(changed.iter().map(|j| j.id).collect::<Vec<_>>(), [7]);

        for n in 1..=3 {
            r.record("build-1", Err("timeout".into()), 30 + n, "endpoint");
        }
        let row = &r.rows()[0];
        assert_eq!(
            (row.state, row.gone_reason, row.cpus),
            (NodeState::Gone, Some(GoneReason::Lost), 64)
        );
        r.record("build-1", Ok(answer(vec![], vec![])), 40, "endpoint");
        assert_eq!(r.rows()[0].state, NodeState::Ready);
        assert_eq!(r.rows()[0].gone_reason, None);
    }

    #[test]
    fn a_changed_lease_in_the_file_reaches_the_node_row_without_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cluster.toml");
        std::fs::write(&path, FILE).unwrap();
        let r = Remotes::load(Some(&path), "endpoint");
        assert_eq!(
            r.rows()[0].deadline.as_deref(),
            Some("2030-01-01T00:00:00Z")
        );
        std::fs::write(&path, FILE.replace("2030-01-01", "2031-02-03")).unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        r.reread("endpoint");
        assert_eq!(
            r.rows()[0].deadline.as_deref(),
            Some("2031-02-03T00:00:00Z")
        );
        r.record("build-1", Ok(answer(vec![], vec![])), 10, "endpoint");
        assert_eq!(
            r.rows()[0].deadline.as_deref(),
            Some("2031-02-03T00:00:00Z")
        );
    }

    /// A stub node in `dir`: an executable named `name` that prints `answer`
    /// whatever its arguments; the ssh line `<stub> <name>` reaches it.
    fn stub(dir: &Path, name: &str, answer: &Answer) -> String {
        use std::os::unix::fs::PermissionsExt as _;
        let json = dir.join(format!("{name}.json"));
        std::fs::write(&json, serde_json::to_string(answer).unwrap()).unwrap();
        let exe = dir.join(name);
        std::fs::write(&exe, format!("#!/bin/sh\nexec cat {}\n", json.display())).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        format!("[remote.{name}]\nssh = \"{} {name}\"\n", exe.display())
    }

    /// Writes `text` to `path` with a modification time `secs` ahead, so
    /// a reread sees a change within the same second.
    fn rewrite(path: &Path, text: &str, secs: u64) {
        std::fs::write(path, text).unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(later)
            .unwrap();
    }

    fn empty_state() -> State {
        serde_json::from_value(serde_json::json!({
            "host": "h", "cores": 1, "tokens": 1, "free": 1, "active": [], "done": [],
            "now_ms": 1, "toolchains": [],
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn a_node_added_to_the_file_is_polled_and_one_taken_out_is_dropped_without_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cluster.toml");
        // The first node has no free slot: a job it could take goes nowhere.
        let mut full = answer(vec![], vec![]);
        full.node.as_mut().unwrap()["slots_in_use"] = 32.into();
        let first = stub(dir.path(), "build-1", &full);
        std::fs::write(&path, &first).unwrap();
        let r = Remotes::load(Some(&path), "endpoint");
        r.poll("endpoint", now_ms).await;
        assert_eq!(r.rows().len(), 1);
        assert_eq!(r.rows()[0].state, NodeState::Ready);

        let busy = answer(
            vec![job(7, JobState::Running), job(8, JobState::Running)],
            vec![],
        );
        let second = stub(dir.path(), "build-2", &busy) + "spill = false\n";
        rewrite(&path, &format!("{first}\n{second}"), 5);
        let published = r.poll("endpoint", now_ms).await;
        assert_eq!(
            r.rows()
                .iter()
                .map(|n| n.node_id.as_str())
                .collect::<Vec<_>>(),
            ["build-1", "build-2"]
        );
        let mut st = empty_state();
        r.merge_jobs(&mut st);
        assert_eq!(
            st.active
                .iter()
                .map(|j| (j.node.as_str(), j.id))
                .collect::<Vec<_>>(),
            [("build-2", 7), ("build-2", 8)]
        );
        assert_eq!(published.len(), 2, "{published:?}");
        // The second node has room, but takes no job of this host.
        assert!(!r.spills() || r.take(now_ms()).is_none());
        assert!(!r.has_room(now_ms()));

        rewrite(&path, &first, 10);
        r.poll("endpoint", now_ms).await;
        assert_eq!(
            r.rows()
                .iter()
                .map(|n| n.node_id.as_str())
                .collect::<Vec<_>>(),
            ["build-1"]
        );
        let mut st = empty_state();
        r.merge_jobs(&mut st);
        assert!(st.active.is_empty() && st.done.is_empty(), "{st:?}");
    }

    #[test]
    fn an_ended_node_job_is_logged_once_also_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("node-jobs.jsonl");
        let lines = || -> Vec<(String, i64, bool)> {
            std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .map(|l| {
                    let j: Job = serde_json::from_str(l).unwrap();
                    (j.node, j.id, j.from_node)
                })
                .collect()
        };
        let r = remotes(FILE).with_log(&log);
        r.record(
            "build-1",
            Ok(answer(
                vec![job(7, JobState::Running)],
                vec![job(5, JobState::Ok)],
            )),
            10,
            "endpoint",
        );
        assert_eq!(lines(), [("build-1".to_string(), 5, true)]);
        r.record(
            "build-1",
            Ok(answer(
                vec![],
                vec![job(5, JobState::Ok), job(7, JobState::Ok)],
            )),
            20,
            "endpoint",
        );
        r.record(
            "build-1",
            Ok(answer(
                vec![],
                vec![job(5, JobState::Ok), job(7, JobState::Ok)],
            )),
            30,
            "endpoint",
        );
        assert_eq!(
            lines(),
            [
                ("build-1".to_string(), 5, true),
                ("build-1".to_string(), 7, true)
            ]
        );
        // A restart reads the file: the jobs the node still lists are not
        // written again.
        let again = remotes(FILE).with_log(&log);
        again.record(
            "build-1",
            Ok(answer(
                vec![],
                vec![job(5, JobState::Ok), job(7, JobState::Ok)],
            )),
            40,
            "endpoint",
        );
        assert_eq!(lines().len(), 2);
    }

    #[test]
    fn a_node_job_keeps_the_id_its_node_gave_it() {
        let r = remotes(FILE);
        let changed = r.record(
            "build-1",
            Ok(answer(
                vec![job(7, JobState::Running)],
                vec![job(5, JobState::Ok)],
            )),
            10,
            "endpoint",
        );
        assert_eq!(
            changed
                .iter()
                .map(|j| (j.id, j.node.as_str(), j.from_node))
                .collect::<Vec<_>>(),
            [(7, "build-1", true)]
        );
        let mut st: State = serde_json::from_value(serde_json::json!({
            "host": "h", "cores": 1, "tokens": 1, "free": 1, "active": [], "done": [],
            "now_ms": 1, "toolchains": [],
        }))
        .unwrap();
        r.merge_jobs(&mut st);
        let ids: Vec<i64> = st.active.iter().chain(&st.done).map(|j| j.id).collect();
        assert_eq!(ids, [7, 5]);
        let text = serde_json::to_string(&st).unwrap();
        assert!(
            !text.contains("\"id\":-"),
            "a negative id in the state: {text}"
        );
    }

    #[test]
    fn state_gets_the_remote_jobs() {
        let r = remotes(FILE);
        r.record(
            "build-1",
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

    /// The node's own facts reach its row on the endpoint: its boot, the
    /// poweroff it has scheduled and its counts; the deadline stays the
    /// file's, so the page can compare the two.
    #[test]
    fn a_node_row_keeps_the_nodes_poweroff_boot_and_counts() {
        let r = remotes(FILE);
        let mut a = answer(vec![], vec![]);
        let row = a.node.as_mut().unwrap();
        row["booted_at"] = "2029-12-31T20:00:00Z".into();
        row["poweroff_at"] = "2029-12-31T23:00:00Z".into();
        row["deadline"] = "2000-01-01T00:00:00Z".into();
        row["ended_hour"] = serde_json::json!({"ok": 7566, "test": 3, "infra": 2});
        r.record("build-1", Ok(a), 10, "endpoint");
        let n = &r.rows()[0];
        assert_eq!(n.booted_at.as_deref(), Some("2029-12-31T20:00:00Z"));
        assert_eq!(n.poweroff_at.as_deref(), Some("2029-12-31T23:00:00Z"));
        assert_eq!(n.deadline.as_deref(), Some("2030-01-01T00:00:00Z"));
        let h = n.ended_hour.unwrap();
        assert_eq!((h.ok, h.test, h.timeout, h.infra), (7566, 3, 0, 2));
    }

    #[test]
    fn more_slots_in_use_than_slots_is_not_taken() {
        let r = remotes(FILE);
        let mut a = answer(vec![], vec![]);
        a.node.as_mut().unwrap()["slots_in_use"] = 99.into();
        r.record("build-1", Ok(a), 10, "endpoint");
        assert_eq!(r.rows()[0].slots_in_use, 0);
    }

    fn chunk(b: &str) -> crate::types::Chunk {
        crate::types::Chunk {
            t: "out".into(),
            b: b.into(),
        }
    }

    async fn relayed(argv_for: impl Fn(u64) -> Vec<String> + Send + 'static) -> Vec<Relay> {
        relay_output(argv_for, 7, "build-1".into(), Duration::from_millis(10))
            .collect()
            .await
    }

    #[tokio::test]
    async fn a_node_job_output_is_relayed_until_it_ends() {
        let first = OutputTail {
            from: 0,
            next: 2,
            chunks: vec![chunk("a"), chunk("b")],
            ended: false,
            kept: true,
        };
        let second = OutputTail {
            from: 2,
            next: 3,
            chunks: vec![chunk("c")],
            ended: true,
            kept: true,
        };
        let got = relayed(move |from| {
            let t = if from == 0 { &first } else { &second };
            vec!["echo".into(), serde_json::to_string(t).unwrap()]
        })
        .await;
        assert_eq!(
            got,
            [
                Relay::Chunk(chunk("a")),
                Relay::Chunk(chunk("b")),
                Relay::Chunk(chunk("c")),
                Relay::End(String::new()),
            ]
        );
    }

    #[tokio::test]
    async fn output_a_node_does_not_give_ends_with_the_reason() {
        let got = relayed(|_| vec!["false".into()]).await;
        let [Relay::End(why)] = got.as_slice() else {
            panic!("{got:?}");
        };
        assert!(why.contains("job 7 on build-1"), "{why}");
        let dropped = OutputTail {
            ended: true,
            ..OutputTail::default()
        };
        let got =
            relayed(move |_| vec!["echo".into(), serde_json::to_string(&dropped).unwrap()]).await;
        assert_eq!(got, [Relay::End("output not kept for this job".into())]);
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
