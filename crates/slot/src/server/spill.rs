//! Spill-over: the daemon hands a queued local job to a remote node of
//! `cluster.toml` that is Ready, before its deadline and has free slots
//! by its last answer. The job's process is then this binary as a
//! remote-mode client (`slot run -- <argv>`, with `SLOT_HOST`, `SLOT_SSH`
//! and `SLOT_REMOTE` of the node, in same-path mode): it syncs, runs the command on
//! the node and streams its output back, holding no local slot. A client
//! that ends with [`INFRA_EXIT`] puts the job back in the local queue,
//! once.

use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use super::nodes::Remotes;
use super::ring::Ring;
use super::{Server, installed_exe, now_ms};
use crate::hostconfig::RemoteNode;
use crate::proto::{Frame, Request};
use crate::run_limit;
use crate::types::{JobState, LOCAL_NODE, Node, NodeState};

/// The exit code of a remote-mode client whose sync or ssh failed, or
/// whose connection to the node was lost.
pub const INFRA_EXIT: i32 = 3;

/// Set in a spilled job's client: a remote start that fails exits
/// [`INFRA_EXIT`] instead of running on this machine's daemon, and a job
/// with it is never spilled.
pub const SPILLED_ENV: &str = "SLOT_SPILLED";

/// How often a queued job looks for a node with room.
pub const TICK: Duration = Duration::from_secs(1);

/// A job handed to a node counts against the node's free slots this long:
/// the node's answer shows it only after the sync, once it queued there.
pub const HANDED_MS: i64 = 15_000;

/// Remote mode with the same paths on the node as here: a handed job runs
/// in the directory it was submitted from, git tree or not.
pub const SAME_PATH_ENV: &str = "SLOT_REMOTE_SAME_PATH";

/// A node a job goes to, as its client reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Node name.
    pub name: String,
    /// The last word of the node's ssh line: `SLOT_HOST`.
    pub host: String,
    /// The words before it: `SLOT_SSH`.
    pub ssh: Vec<String>,
    /// `slot` on the node: `SLOT_REMOTE`.
    pub slot_path: String,
}

impl Target {
    /// The target of `conf`; `None` when its ssh line has no word before
    /// the host.
    pub fn of(conf: &RemoteNode) -> Option<Self> {
        let (host, ssh) = conf.ssh.split_last()?;
        if ssh.is_empty() || ssh.iter().any(|w| w.contains(char::is_whitespace)) {
            return None;
        }
        Some(Self {
            name: conf.name.clone(),
            host: host.clone(),
            ssh: ssh.to_vec(),
            slot_path: conf
                .table
                .slot_path
                .clone()
                .unwrap_or_else(|| "slot".into()),
        })
    }
}

fn get<'a>(env: &'a [String], key: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find_map(|e| e.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

fn set(env: &[String], key: &str) -> bool {
    get(env, key).is_some_and(|v| !v.is_empty() && v != "0")
}

/// Set by a wrapper that keeps its job on this machine: the text is the
/// job's `local_reason`, and the job is never handed to a node.
pub const LOCAL_REASON_ENV: &str = "SLOT_LOCAL_REASON";

/// The reason of a job whose caller asked for this machine
/// (`SLOT_NO_REMOTE`, or a command `.slot.toml` lists as local).
pub const LOCAL_REQUESTED: &str = "local requested";

/// The reason of a job submitted with `slot --here`: it needs this host
/// (its credentials) while it runs.
pub const HERE: &str = "here: needs this host";

/// The reason of a job a node sent back with [`INFRA_EXIT`].
pub const SENT_BACK: &str = "sent back by a node";

/// The reason of a job that may go to a node but got slots here before
/// one took it.
pub const HERE_FIRST: &str = "slots free here first";

/// The longest reason kept from a request.
const REASON_MAX: usize = 200;

/// The first non-empty line of `text`, trimmed and cut at [`REASON_MAX`]
/// characters.
pub fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .chars()
        .take(REASON_MAX)
        .collect()
}

/// The reason the client named: the request's own, else `SLOT_LOCAL_REASON`.
fn named(req: &Request) -> String {
    let own = first_line(&req.local_reason);
    if own.is_empty() {
        first_line(get(&req.env, LOCAL_REASON_ENV).unwrap_or_default())
    } else {
        own
    }
}

/// Why the job of `req` stays here, or `None` when it may go to a node:
/// a nested job, one whose caller asked for this machine or named a
/// reason, and the ones that are no command of a caller here (a remote
/// client's, a gate's hold, one a node sent back).
pub fn refusal(req: &Request) -> Option<String> {
    if req.workspace.is_some() {
        return Some("remote job".into());
    }
    if req.hold_for.is_some() {
        return Some("gated job".into());
    }
    if set(&req.env, super::JOB_ENV) {
        return Some("nested job".into());
    }
    if set(&req.env, SPILLED_ENV) {
        return Some(SENT_BACK.into());
    }
    let named = named(req);
    if !named.is_empty() {
        return Some(named);
    }
    if set(&req.env, "SLOT_NO_REMOTE") {
        return Some(LOCAL_REQUESTED.into());
    }
    if under_home_cache(req) {
        return Some("cache rule".into());
    }
    None
}

/// The `local_reason` a new job of `req` carries: with a node that takes
/// jobs, why it stays here; without one only a reason its client named
/// (a remote start that failed, `SLOT_LOCAL_REASON`).
pub fn local_reason(req: &Request, spills: bool) -> String {
    if spills {
        refusal(req).unwrap_or_default()
    } else {
        named(req)
    }
}

/// Whether the job's working directory or an argument lies under the
/// caller's `$HOME/.cache` (session scratch): such jobs never leave this host.
fn under_home_cache(req: &Request) -> bool {
    let own = std::env::var("HOME").unwrap_or_default();
    let home = req
        .env
        .iter()
        .find_map(|e| e.strip_prefix("HOME="))
        .unwrap_or(own.as_str());
    if home.is_empty() {
        return false;
    }
    let cache = format!("{}/.cache/", home.trim_end_matches('/'));
    format!("{}/", req.cwd).starts_with(&cache) || req.argv.iter().any(|a| a.contains(&cache))
}

/// The spilled job's environment: the job's, with the node's `SLOT_HOST`,
/// `SLOT_SSH` and `SLOT_REMOTE`, same-path mode and [`SPILLED_ENV`].
pub fn child_env(env: &[String], t: &Target) -> Vec<String> {
    let drop = [
        "SLOT_HOST",
        "SLOT_SSH",
        "SLOT_REMOTE",
        SAME_PATH_ENV,
        super::JOB_ENV,
    ];
    env.iter()
        .filter(|e| {
            let k = e.split_once('=').map_or(e.as_str(), |(k, _)| k);
            !drop.contains(&k) && k != SPILLED_ENV
        })
        .cloned()
        .chain([
            format!("SLOT_HOST={}", t.host),
            format!("SLOT_SSH={}", t.ssh.join(" ")),
            format!("SLOT_REMOTE={}", t.slot_path),
            format!("{SAME_PATH_ENV}=1"),
            format!("{SPILLED_ENV}=1"),
        ])
        .collect()
}

/// The spilled job's command: `exe run [--timeout <s>] -- <argv>`.
pub fn child_argv(exe: &Path, req: &Request) -> Vec<String> {
    let mut v = vec![exe.to_string_lossy().into_owned(), "run".into()];
    if let Some(t) = req.timeout_s {
        v.push("--timeout".into());
        v.push(t.to_string());
    }
    v.push("--".into());
    v.extend(req.argv.iter().cloned());
    v
}

/// Jobs node `row` may still take at `now` (Unix ms): its free slots by
/// its last answer less the jobs `handed` to it within [`HANDED_MS`]; none
/// unless it is Ready, has no queue and is before its deadline.
pub fn room(row: &Node, handed: &[i64], now: i64) -> usize {
    if row.state != NodeState::Ready || row.queued_jobs > 0 {
        return 0;
    }
    if let Some(d) = &row.deadline {
        let ends = time::OffsetDateTime::parse(d, &time::format_description::well_known::Rfc3339)
            .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64);
        if ends.is_ok_and(|e| e <= now) {
            return 0;
        }
    }
    let recent = handed.iter().filter(|&&t| now - t < HANDED_MS).count();
    (row.slots.saturating_sub(row.slots_in_use) as usize).saturating_sub(recent)
}

/// How a supervised job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    /// It finished and its client was answered.
    Finished,
    /// Its node failed it with [`INFRA_EXIT`]: it waits here again.
    Requeue,
}

impl Server {
    /// Whether job `id` of `req` may go to a remote node; marks it so.
    pub(super) fn spill_eligible(&self, id: i64, req: &Request) -> bool {
        let ok = self.remotes.spills() && refusal(req).is_none();
        if ok && let Some(l) = self.inner.lock().expect("server lock").active.get_mut(&id) {
            l.spillable = true;
        }
        ok
    }

    /// Whether job `id` has waited long enough and no older job that may
    /// go waits for slots: the oldest goes first.
    pub(super) fn spill_due(&self, id: i64) -> bool {
        let g = self.inner.lock().expect("server lock");
        let Some(l) = g.active.get(&id) else {
            return false;
        };
        let after = i64::try_from(self.remotes.spill.after_s.saturating_mul(1000)).unwrap_or(0);
        l.spillable
            && l.job.state == JobState::Queued
            && now_ms() - l.job.enqueued_ms >= after
            && !g.active.values().any(|o| {
                o.job.id < id
                    && o.spillable
                    && o.waiter.is_some()
                    && o.job.state == JobState::Queued
            })
    }

    /// A node for job `id`, when it is due and one has room.
    pub(super) fn spill_pick(&self, id: i64) -> Option<Target> {
        if !self.spill_due(id) {
            return None;
        }
        self.remotes.take(now_ms())
    }

    /// Runs queued job `id` on node `t` as a remote-mode client of this
    /// binary. `None` when a handover took the job.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_spilled(
        self: &Arc<Self>,
        id: i64,
        req: &Request,
        cwd: &Path,
        t: Target,
        tx: &mpsc::UnboundedSender<Frame>,
        gone: watch::Receiver<bool>,
        out: &Arc<Ring>,
    ) -> Option<Ran> {
        let argv = child_argv(&installed_exe(), req);
        let env = child_env(&req.env, &t);
        let spawned = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(cwd)
            .env_clear()
            .envs(env.iter().filter_map(|e| e.split_once('=')))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                eprintln!("slot: job {id}: node {}: {e}", t.name);
                if let Some(l) = self.inner.lock().expect("server lock").active.get_mut(&id) {
                    l.spillable = false;
                    l.job.local_reason = first_line(&format!("node {} not started: {e}", t.name));
                }
                return Some(Ran::Requeue);
            }
        };
        let pid = i32::try_from(child.id()).ok()?;
        let pipes = [
            child.stdout.take().map(OwnedFd::from),
            child.stderr.take().map(OwnedFd::from),
        ];
        // The process is reaped by pid, like a local job's.
        drop(child);
        let pumps = self.start_pumps(pipes, tx, out, &(String::new(), String::new()));
        let limit_s = {
            let limits = self.run_limit.lock().expect("run limit lock");
            run_limit::job_limit(req.timeout_s, limits.run_limit_s, limits.max_s())
        };
        {
            let mut g = self.inner.lock().expect("server lock");
            if let Some(l) = g.active.get_mut(&id) {
                l.pid = Some(pid);
                l.held = 0;
                l.claimed = true;
                // It uses no CPU here: admission by share and the CPU
                // limit count it with nothing.
                l.share = 0.0;
                l.expect_cores = 0.0;
                l.waiter = None;
                l.req = None;
                l.pumps = pumps;
                l.job.started_ms = now_ms();
                l.job.limit_s = limit_s;
                l.job.state = JobState::Running;
                l.job.node.clone_from(&t.name);
                l.job.wait_reason.clear();
            }
            let _ = tx.send(Frame::Info {
                msg: format!("slot: job {id} runs on node {}", t.name),
            });
            let _ = tx.send(Frame::Start {
                id,
                target_dir: String::new(),
            });
        }
        self.publish(id);
        let ran = self.supervise(id, pid, tx, gone, true).await?;
        if ran == Ran::Requeue {
            {
                let mut g = self.inner.lock().expect("server lock");
                if let Some(l) = g.active.get_mut(&id) {
                    l.pid = None;
                    l.exited = None;
                    l.claimed = false;
                    l.spillable = false;
                    l.req = Some(req.clone());
                    l.pumps.clear();
                    l.job.started_ms = 0;
                    l.job.limit_s = None;
                    l.job.state = JobState::Queued;
                    l.job.node = LOCAL_NODE.to_string();
                    l.job.local_reason = SENT_BACK.to_string();
                }
            }
            let _ = tx.send(Frame::Info {
                msg: format!(
                    "slot: node {} did not run job {id} (exit {INFRA_EXIT}); queued here again",
                    t.name
                ),
            });
            self.publish(id);
        }
        Some(ran)
    }
}

impl Remotes {
    /// Whether a node that takes spilled jobs has room at `now`.
    pub fn has_room(&self, now: i64) -> bool {
        let g = self.nodes.lock().expect("nodes lock");
        g.iter()
            .filter(|e| e.conf.table.spill != Some(false))
            .any(|e| room(&e.row, &e.handed, now) > 0)
    }

    /// Whether any node takes spilled jobs.
    pub fn spills(&self) -> bool {
        let g = self.nodes.lock().expect("nodes lock");
        g.iter().any(|e| e.conf.table.spill != Some(false))
    }

    /// Takes a place for one job on the node with the most room, and
    /// counts it handed.
    pub fn take(&self, now: i64) -> Option<Target> {
        let mut g = self.nodes.lock().expect("nodes lock");
        let best = g
            .iter_mut()
            .filter(|e| e.conf.table.spill != Some(false))
            .filter_map(|e| {
                e.handed.retain(|&t| now - t < HANDED_MS);
                let n = room(&e.row, &e.handed, now);
                (n > 0).then_some((n, e))
            })
            .max_by_key(|(n, _)| *n)?
            .1;
        let t = Target::of(&best.conf)?;
        best.handed.push(now);
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hostconfig::parse;

    fn req(argv: &[&str]) -> Request {
        Request {
            argv: argv.iter().map(|s| (*s).to_string()).collect(),
            cwd: "/w/repo".into(),
            env: vec!["PATH=/usr/bin".into(), "HOME=/w".into()],
            repo: "repo".into(),
            ..Request::default()
        }
    }

    fn why(r: &Request) -> Option<String> {
        refusal(r)
    }

    #[test]
    fn every_job_a_caller_here_submitted_may_go() {
        for argv in [
            &["cargo", "build", "--release"][..],
            &["cargo", "test", "--no-run", "--message-format=json"],
            &["npx", "vitest", "run", "/etc/hosts"],
            &["task", "check"],
        ] {
            assert_eq!(why(&req(argv)), None, "{argv:?}");
        }
    }

    #[test]
    fn nested_local_and_foreign_jobs_stay_here() {
        let base = req(&["cargo", "build"]);
        let with_env = |kv: &str| {
            let mut r = base.clone();
            r.env.push(kv.into());
            r
        };
        assert_eq!(why(&with_env("SLOT_JOB=4")), Some("nested job".into()));
        assert_eq!(why(&with_env("SLOT_SPILLED=1")), Some(SENT_BACK.into()));
        assert_eq!(
            why(&with_env("SLOT_NO_REMOTE=1")),
            Some(LOCAL_REQUESTED.into())
        );
        assert_eq!(why(&with_env("SLOT_NO_REMOTE=0")), None);
        let mut r = base.clone();
        r.cwd = "/w/.cache/scratch/s1/copy".into();
        assert_eq!(why(&r), Some("cache rule".into()));
        let mut r = base.clone();
        r.argv = vec!["sh".into(), "/w/.cache/scratch/s1/run.sh".into()];
        assert_eq!(why(&r), Some("cache rule".into()));
        let mut r = base.clone();
        r.cwd = "/w/work/repo".into();
        assert_eq!(why(&r), None);
        let mut r = base.clone();
        r.hold_for = Some(vec!["cargo".into()]);
        assert_eq!(why(&r), Some("gated job".into()));
        let mut r = base.clone();
        r.workspace = Some(crate::proto::Workspace::default());
        assert_eq!(why(&r), Some("remote job".into()));
    }

    #[test]
    fn a_named_reason_keeps_the_job_here_and_is_its_local_reason() {
        let base = req(&["cargo", "build"]);
        let mut wrapped = base.clone();
        wrapped.env.push("SLOT_LOCAL_REASON=cache rule".into());
        wrapped.env.push("SLOT_NO_REMOTE=1".into());
        assert_eq!(why(&wrapped), Some("cache rule".into()));
        let mut generic = base.clone();
        generic
            .env
            .push("SLOT_LOCAL_REASON=  needs the GPU here\nsecond line".into());
        assert_eq!(why(&generic), Some("needs the GPU here".into()));
        let mut empty = base.clone();
        empty.env.push("SLOT_LOCAL_REASON=".into());
        assert_eq!(why(&empty), None);
        let mut failed = base.clone();
        failed.local_reason = "sync failed: rsync exited 12\ndetail".into();
        assert_eq!(why(&failed), Some("sync failed: rsync exited 12".into()));
        // The request's own reason wins over the environment's; a nested
        // job is named as one whatever it inherited.
        failed.env.push("SLOT_LOCAL_REASON=other".into());
        assert_eq!(why(&failed), Some("sync failed: rsync exited 12".into()));
        failed.env.push("SLOT_JOB=3".into());
        assert_eq!(why(&failed), Some("nested job".into()));
        let mut long = base.clone();
        long.local_reason = "x".repeat(500);
        assert_eq!(why(&long).map(|s| s.len()), Some(REASON_MAX));

        // Without a node that takes jobs only a named reason is kept.
        let mut no_remote = base.clone();
        no_remote.env.push("SLOT_NO_REMOTE=1".into());
        assert_eq!(local_reason(&no_remote, true), LOCAL_REQUESTED);
        assert_eq!(local_reason(&no_remote, false), "");
        assert_eq!(local_reason(&generic, false), "needs the GPU here");
        assert_eq!(local_reason(&base, true), "");
    }

    const FILE: &str = "spill_after_s = 5\n\
        [remote.node-1]\nssh = \"ssh -o BatchMode=yes node-1\"\nslot_path = \"/opt/slot\"\n\
        [remote.node-2]\nssh = \"ssh node-2\"\nspill = false\n";

    fn ready(r: &Remotes, at: usize, slots: u32, in_use: u32) {
        let mut g = r.nodes.lock().unwrap();
        let row = &mut g[at].row;
        row.state = NodeState::Ready;
        row.slots = slots;
        row.slots_in_use = in_use;
    }

    #[test]
    fn a_node_takes_no_more_jobs_than_its_free_slots() {
        let (nodes, spill) = crate::hostconfig::parse_all(FILE).unwrap();
        assert_eq!(spill.after_s, 5);
        let r = Remotes::new(nodes, "endpoint");
        assert!(r.spills());
        // Joining: no answer yet, no room.
        assert_eq!(r.take(1_000), None);
        ready(&r, 0, 4, 1);
        ready(&r, 1, 64, 0);
        let mut got = Vec::new();
        while let Some(t) = r.take(1_000) {
            got.push(t);
        }
        // node-2 has `spill = false`; node-1 has 3 free slots.
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0],
            Target {
                name: "node-1".into(),
                host: "node-1".into(),
                ssh: vec!["ssh".into(), "-o".into(), "BatchMode=yes".into()],
                slot_path: "/opt/slot".into(),
            }
        );
        // The handed jobs stop counting once the node's answers show them.
        assert!(r.take(1_000 + HANDED_MS).is_some());
    }

    #[test]
    fn no_room_past_the_deadline_or_with_a_queue() {
        let mut row = Node {
            state: NodeState::Ready,
            slots: 8,
            ..Node::default()
        };
        assert_eq!(room(&row, &[], 0), 8);
        assert_eq!(room(&row, &[0, 1], 2), 6);
        row.deadline = Some("1970-01-01T00:00:10Z".into());
        assert_eq!(room(&row, &[], 9_999), 8);
        assert_eq!(room(&row, &[], 10_000), 0);
        row.deadline = None;
        row.queued_jobs = 1;
        assert_eq!(room(&row, &[], 0), 0);
        row.queued_jobs = 0;
        row.state = NodeState::Gone;
        assert_eq!(room(&row, &[], 0), 0);
        let nodes = parse("[remote.n]\nssh = \"node-only\"\n").unwrap();
        assert_eq!(Target::of(&nodes[0]), None);
    }

    #[test]
    fn the_client_gets_the_node_and_the_command() {
        let t = Target {
            name: "node-1".into(),
            host: "node-1".into(),
            ssh: vec!["ssh".into(), "-p".into(), "2222".into()],
            slot_path: "slot".into(),
        };
        let env = child_env(
            &[
                "PATH=/usr/bin".into(),
                "SLOT_HOST=old".into(),
                "SLOT_REMOTE_SAME_PATH=0".into(),
                "SLOT_JOB=9".into(),
            ],
            &t,
        );
        assert_eq!(
            env,
            [
                "PATH=/usr/bin",
                "SLOT_HOST=node-1",
                "SLOT_SSH=ssh -p 2222",
                "SLOT_REMOTE=slot",
                "SLOT_REMOTE_SAME_PATH=1",
                "SLOT_SPILLED=1"
            ]
        );
        let mut r = req(&["cargo", "build"]);
        r.timeout_s = Some(600);
        assert_eq!(
            child_argv(Path::new("/usr/bin/slot"), &r),
            [
                "/usr/bin/slot",
                "run",
                "--timeout",
                "600",
                "--",
                "cargo",
                "build"
            ]
        );
    }
}
