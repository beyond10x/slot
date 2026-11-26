//! Warm-up after a merge: `slot warm` tells the daemon that `main` of a
//! repository moved, and the daemon builds that tree's tests at the lowest
//! level, so the next worktree of the repository starts from a built
//! target directory instead of a cold one.
//!
//! - The daemon starts one only for a repository a job built here in the
//!   last [`WITHIN_MS`], and one at a time per repository.
//! - It answers the client at once and runs the job without it, queued at
//!   `low` whatever a priority rule says, never promoted.
//! - A job of a session that still waits for its slots [`PREEMPT_AFTER`]
//!   after it queued cancels every running warm-up; the warm-up's record
//!   ends `cancelled` with `cancel_reason` `preempted by job N`.
//! - On a node a warm-up builds into its own workspace's target directory
//!   and touches it when it ends ok, so [`super::cache::Cache::seed`] picks
//!   it for the next new workspace of the repository.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use super::{Server, Stop, now_ms};
use crate::proto::Request;
use crate::types::{Job, JobState, Priority};

/// A merge warms a repository that a job built here within this long.
pub const WITHIN_MS: i64 = 24 * 3_600_000;

/// How long a job of a session waits for its slots before it cancels the
/// running warm-ups: a job the pool serves at once never does.
pub const PREEMPT_AFTER: Duration = Duration::from_millis(50);

/// Records that a job of `job.repo` started at `job.started_ms`; warm-ups
/// and jobs that never started are left out.
pub fn record_built(built: &mut HashMap<String, i64>, job: &Job) {
    if job.warm || job.repo.is_empty() || job.started_ms <= 0 {
        return;
    }
    let at = built.entry(job.repo.clone()).or_insert(0);
    *at = (*at).max(job.started_ms);
}

impl Server {
    /// `slot warm`: starts the warm-up `req` describes without its client,
    /// or says why it starts none. `(exit code, message)`; the code is 0
    /// either way, so a hook that runs it never fails a merge.
    pub(super) fn warm(self: &Arc<Self>, req: &Request, uid: Option<u32>) -> (i32, String) {
        let repo = req.repo.clone();
        if let Some(why) = self.warm_refusal(&repo) {
            return (0, format!("slot: {repo} not warmed: {why}"));
        }
        let mut req = req.clone();
        req.priority = Some(Priority::Low);
        let shown = req.argv.join(" ");
        let (gone_tx, gone_rx) = watch::channel(false);
        // Nobody reads the job's frames: its output is in the web view.
        let (tx, _rx) = mpsc::unbounded_channel();
        let stop = Stop {
            uid,
            gone: Arc::new(gone_tx),
            frames: tx.clone(),
        };
        let msg = format!(
            "slot: warm-up of {repo} queued at low: `{shown}`; a job of a session that waits \
             for slots cancels it"
        );
        let s = Arc::clone(self);
        tokio::spawn(async move {
            let _ = s.run_job(req, tx, gone_rx, Some(stop), None, None).await;
            s.warming.lock().expect("warming lock").remove(&repo);
        });
        (0, msg)
    }

    /// Why a merge of `repo` starts no warm-up; `None` when it starts one,
    /// and then the repository counts as warming.
    fn warm_refusal(&self, repo: &str) -> Option<String> {
        if repo.is_empty() {
            return Some("the request names no repository".into());
        }
        let now = now_ms();
        {
            let g = self.inner.lock().expect("server lock");
            let lately = g.built.get(repo).is_some_and(|&t| now - t <= WITHIN_MS)
                || g.active
                    .values()
                    .any(|l| !l.job.warm && l.job.repo == repo && l.job.started_ms > 0);
            if !lately {
                return Some("no job of it built here in the last 24 hours".into());
            }
            if g.active.values().any(|l| l.job.warm && l.job.repo == repo) {
                return Some("a warm-up of it is queued or running".into());
            }
        }
        if !self
            .warming
            .lock()
            .expect("warming lock")
            .insert(repo.to_string())
        {
            return Some("a warm-up of it is queued or running".into());
        }
        None
    }

    /// Cancels every running warm-up for job `by`, which waits for its
    /// slots; each ends `cancelled` with `preempted by job <by>`.
    pub(super) fn preempt_warm(&self, by: i64) {
        let stops: Vec<(i64, Stop)> = {
            let mut g = self.inner.lock().expect("server lock");
            g.active
                .values_mut()
                .filter(|l| {
                    l.job.warm && l.job.state == JobState::Running && l.job.cancel_reason.is_empty()
                })
                .filter_map(|l| {
                    let s = l.stop.as_ref()?;
                    l.job.cancel_reason = format!("preempted by job {by}");
                    Some((
                        l.job.id,
                        Stop {
                            uid: s.uid,
                            gone: Arc::clone(&s.gone),
                            frames: s.frames.clone(),
                        },
                    ))
                })
                .collect()
        };
        for (id, s) in stops {
            eprintln!("slot: warm-up job {id} preempted by job {by}");
            s.gone.send_replace(true);
            self.publish(id);
        }
    }

    /// Whether job `id` ended with exit 0.
    pub(super) fn ended_ok(&self, id: i64) -> bool {
        let g = self.inner.lock().expect("server lock");
        g.done
            .iter()
            .rev()
            .find(|d| d.job.id == id)
            .is_some_and(|d| d.job.state == JobState::Ok)
    }
}
