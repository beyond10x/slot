//! Wire format between `slot` and `slot serve`: newline-delimited JSON over a
//! unix socket, or over ssh stdio through `slot proxy`.

use serde::{Deserialize, Serialize};

/// The first and only message a client sends.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Request {
    /// Command and arguments.
    pub argv: Vec<String>,
    /// Absolute working directory (local mode).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    /// Environment as `KEY=VALUE`.
    #[serde(default)]
    pub env: Vec<String>,
    /// Set when the source was synced to the server's workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    /// Agent session id, for grouping in the UI.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session: String,
    /// Repository name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repo: String,
    /// Git checkout identified by the client at submission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<crate::types::Checkout>,
    /// `host:pid` of the client's parent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client: String,
    /// `slot cancel`: cancel this job instead of running `argv`. Only the
    /// user who submitted the job may. Daemons older than 0.5.1 answer
    /// "empty command".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<i64>,
    /// `slot --tokens N`: slots the job holds instead of the toolchain's
    /// count, capped at the slots in circulation. Daemons that predate it
    /// ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u32>,
    /// `slot --priority <level>`: the job's own queue level. A rule of the
    /// daemon's table for the repository wins over it. Daemons that predate
    /// it ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<crate::types::Priority>,
    /// `slot --timeout <duration>`: seconds the job may run before the
    /// daemon ends it, in place of the daemon's run-time limit; capped at
    /// the most a job may name (`slot run-limit`). Daemons that predate it
    /// ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    /// Set by `slot gate` for a client in a sandbox: the command the
    /// client runs there. `argv` is then `slot hold <fifo>`, which keeps
    /// the job's slots while the sandboxed command runs. The daemon shows,
    /// counts and sizes the job by this command and starts `argv`. A daemon
    /// that predates it shows and sizes the job as `slot hold`, but still
    /// runs nothing but the hold process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_for: Option<Vec<String>>,
    /// The client's open-file limit. The daemon runs a local job with it,
    /// capped at its own hard limit ([`FileLimit::for_job`]); a request
    /// without it runs with the limit the daemon started with, and a
    /// daemon that predates it ignores it. Not sent in remote mode: the
    /// build server's limit applies there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nofile: Option<FileLimit>,
    /// A remote cargo job: before it queues, the daemon fetches the crates
    /// of the mirror's `Cargo.lock` once and runs it offline
    /// (`cargo_fetch.rs`); when the fetch fails it answers with a
    /// [`Frame::Fallback`], which only a client that sets this reads. A
    /// daemon that predates it ignores it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fetch_once: bool,
    /// Why the client sends the job to this machine's daemon when a remote
    /// node is configured, such as `sync failed: …` after a remote start
    /// failed. The daemon shows it as the job's `local_reason` and hands
    /// such a job to no node. A daemon that predates it ignores it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub local_reason: String,
    /// A remote request: milliseconds the client took to send the tree to
    /// this server; the job's record carries it as `sync_ms`. A daemon
    /// that predates it ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_ms: Option<u64>,
    /// A remote request: bytes that sync sent; the record's `sync_bytes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_bytes: Option<u64>,
    /// Adds what the client measured after the job ended to the record of
    /// an ended remote job, in place of running `argv`. A daemon that
    /// predates it answers "empty command".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amend: Option<Amend>,
}

/// What a client adds to the record of a remote job after it ended
/// ([`Request::amend`]).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Amend {
    /// The job's id on the server, from its `start` frame.
    pub id: i64,
    /// Milliseconds the products and `--fetch` paths took to come back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back_ms: Option<u64>,
}

impl Request {
    /// The open-file limit the client asks for its job: none for a remote
    /// request, whose job runs with the build server's limit.
    pub fn job_nofile(&self) -> Option<FileLimit> {
        self.workspace.is_none().then_some(self.nofile).flatten()
    }
}

/// An open-file limit (`RLIMIT_NOFILE`, `ulimit -n`); `u64::MAX` is
/// unlimited.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileLimit {
    /// The soft limit, the one in force.
    #[serde(default)]
    pub soft: u64,
    /// The hard limit, the most the soft one may be raised to.
    #[serde(default)]
    pub hard: u64,
}

/// `unlimited` for `u64::MAX`, else the number.
fn limit_text(v: u64) -> String {
    if v == u64::MAX {
        "unlimited".into()
    } else {
        v.to_string()
    }
}

impl FileLimit {
    fn from_rlimit(r: rustix::process::Rlimit) -> Self {
        Self {
            soft: r.current.unwrap_or(u64::MAX),
            hard: r.maximum.unwrap_or(u64::MAX),
        }
    }

    fn rlimit(self) -> rustix::process::Rlimit {
        let v = |x: u64| (x != u64::MAX).then_some(x);
        rustix::process::Rlimit {
            current: v(self.soft),
            maximum: v(self.hard),
        }
    }

    /// This process's open-file limit.
    pub fn of_this_process() -> Self {
        Self::from_rlimit(rustix::process::getrlimit(
            rustix::process::Resource::Nofile,
        ))
    }

    /// Makes this the open-file limit of this process; its children
    /// inherit it from then on.
    pub fn set_for_this_process(self) -> std::io::Result<()> {
        rustix::process::setrlimit(rustix::process::Resource::Nofile, self.rlimit())
            .map_err(Into::into)
    }

    /// `SOFT:HARD`, `unlimited` for either: the value of `slot __exec
    /// --nofile`.
    pub fn arg(self) -> String {
        format!("{}:{}", limit_text(self.soft), limit_text(self.hard))
    }

    /// Reads [`FileLimit::arg`].
    pub fn parse(s: &str) -> Result<Self, String> {
        let one = |v: &str| match v {
            "unlimited" => Ok(u64::MAX),
            _ => v
                .parse::<u64>()
                .map_err(|_| format!("`{v}` is not a number of files or `unlimited`")),
        };
        let (soft, hard) = s
            .split_once(':')
            .ok_or_else(|| format!("`{s}` is not SOFT:HARD"))?;
        let l = Self {
            soft: one(soft)?,
            hard: one(hard)?,
        };
        if l.soft > l.hard {
            return Err(format!("`{s}`: the soft limit is above the hard limit"));
        }
        Ok(l)
    }

    /// `slot __exec`: makes this the open-file limit of this process, then
    /// executes `program` with `args` in its place, under the name `arg0`.
    /// The pid, the process group and the environment stay, so the daemon
    /// sees the job's own program from here on. Returns only when the
    /// program could not be executed: the code to exit with.
    pub fn exec(
        self,
        arg0: &std::ffi::OsStr,
        program: &std::ffi::OsStr,
        args: &[std::ffi::OsString],
    ) -> i32 {
        use std::os::unix::process::CommandExt as _;
        if let Err(e) = self.set_for_this_process() {
            eprintln!("slot: open-file limit {} not set: {e}", self.arg());
        }
        let e = std::process::Command::new(program)
            .arg0(arg0)
            .args(args)
            .exec();
        eprintln!("slot: {}: {e}", arg0.to_string_lossy());
        127
    }

    /// The limit a job of a caller with this limit gets from a daemon whose
    /// own hard limit is `daemon_hard`: the caller's, capped at
    /// `daemon_hard`. With the warning to print when the caller's soft limit
    /// is above it.
    pub fn for_job(self, daemon_hard: u64) -> (Self, Option<String>) {
        let hard = self.hard.min(daemon_hard);
        let soft = self.soft.min(hard);
        let warning = (self.soft > daemon_hard).then(|| {
            format!(
                "open files: the caller's limit is {}, the daemon's hard limit is {}: the job gets {}",
                limit_text(self.soft),
                limit_text(daemon_hard),
                limit_text(soft)
            )
        });
        (Self { soft, hard }, warning)
    }
}

/// A repository synced to the server.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Workspace {
    /// Stable id: hash of client host and repository root.
    pub id: String,
    /// Working directory relative to the repository root.
    pub rel_cwd: String,
    /// Repository root on the client, for path rewriting.
    pub client_root: String,
    /// Mirrored at `client_root` itself, not under the workspace root
    /// (`SLOT_REMOTE_SAME_PATH`); the server admits it only under a
    /// `--same-path-root`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub same_path: bool,
    /// A same-path job's target directory, the client's own absolute path
    /// (below a `--same-path-root`); empty keeps cargo's default there.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_dir: String,
}

/// One server message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Frame {
    /// Standard output bytes.
    Out {
        /// Bytes as text; invalid UTF-8 is replaced.
        b: String,
    },
    /// Standard error bytes.
    Err {
        /// Bytes as text; invalid UTF-8 is replaced.
        b: String,
    },
    /// The job's process started. A client that loses the connection
    /// before this frame knows the command never ran. Clients older than
    /// 0.3.1 ignore it.
    Start {
        /// Job id.
        #[serde(default)]
        id: i64,
        /// For a synced workspace: the job's target directory on the server
        /// (`CARGO_TARGET_DIR`), where `slot --fetch target/…` reads from.
        /// Empty for local jobs and from daemons older than 0.8.0.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        target_dir: String,
        /// For a synced cargo job: what its target directory held when it
        /// queued, the record's `seeded_from`. Absent for other jobs and
        /// from an older daemon.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seeded_from: Option<String>,
    },
    /// A message from the daemon for the client's stderr.
    Info {
        /// The message.
        msg: String,
    },
    /// The job ended.
    Exit {
        /// Exit code; 128 + signal for a signalled process.
        code: i32,
    },
    /// The job did not start on this server and goes back to the client,
    /// which runs it on its own machine's daemon. Sent only to a client
    /// whose request set `fetch_once`, after its fetch failed.
    Fallback {
        /// Why, for the client's stderr.
        msg: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_from_an_older_client_has_no_job_tokens() {
        let r: Request = serde_json::from_str(r#"{"argv":["sh","build.sh"],"env":[]}"#).unwrap();
        assert_eq!(r.tokens, None);
        assert_eq!(r.checkout, None);
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains("tokens"), "{line}");
        let r = Request {
            tokens: Some(4),
            ..r
        };
        let back: Request = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.tokens, Some(4));
    }

    #[test]
    fn a_request_from_an_older_client_has_no_priority() {
        use crate::types::Priority;
        let r: Request = serde_json::from_str(r#"{"argv":["sh","build.sh"],"env":[]}"#).unwrap();
        assert_eq!(r.priority, None);
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains("priority"), "{line}");
        let r = Request {
            priority: Some(Priority::Low),
            ..r
        };
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.contains(r#""priority":"low""#), "{line}");
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.priority, Some(Priority::Low));
    }

    #[test]
    fn a_request_from_an_older_client_names_no_time() {
        let r: Request = serde_json::from_str(r#"{"argv":["sh","build.sh"],"env":[]}"#).unwrap();
        assert_eq!(r.timeout_s, None);
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains("timeout_s"), "{line}");
        let r = Request {
            timeout_s: Some(1800),
            ..r
        };
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.contains(r#""timeout_s":1800"#), "{line}");
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.timeout_s, Some(1800));
    }

    #[test]
    fn a_request_from_an_older_client_names_no_open_file_limit() {
        let r: Request = serde_json::from_str(r#"{"argv":["sh","build.sh"],"env":[]}"#).unwrap();
        assert_eq!((r.nofile, r.job_nofile()), (None, None));
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains("nofile"), "{line}");
        let limit = FileLimit {
            soft: 4096,
            hard: 1_000_000,
        };
        let r = Request {
            nofile: Some(limit),
            ..r
        };
        let line = serde_json::to_string(&r).unwrap();
        assert!(
            line.contains(r#""nofile":{"soft":4096,"hard":1000000}"#),
            "{line}"
        );
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.job_nofile(), Some(limit));
        let remote = Request {
            workspace: Some(Workspace::default()),
            ..back
        };
        assert_eq!(remote.job_nofile(), None, "remote: the server's limit");
    }

    #[test]
    fn a_job_gets_the_callers_limit_capped_at_the_daemons_hard_limit() {
        let l = |soft, hard| FileLimit { soft, hard };
        assert_eq!(
            l(1_000_000, 1_000_000).for_job(1_000_000),
            (l(1_000_000, 1_000_000), None)
        );
        assert_eq!(
            l(256, 4096).for_job(1_000_000),
            (l(256, 4096), None),
            "lower than the daemon's: the caller's"
        );
        let (job, warning) = l(2_000_000, 2_000_000).for_job(1_000_000);
        assert_eq!(job, l(1_000_000, 1_000_000));
        assert_eq!(
            warning.as_deref(),
            Some(
                "open files: the caller's limit is 2000000, the daemon's hard limit is 1000000: \
                 the job gets 1000000"
            )
        );
        let (job, warning) = l(u64::MAX, u64::MAX).for_job(524_288);
        assert_eq!(job, l(524_288, 524_288));
        assert!(warning.unwrap().contains("limit is unlimited,"));
        assert_eq!(
            l(8192, u64::MAX).for_job(u64::MAX),
            (l(8192, u64::MAX), None)
        );
    }

    #[test]
    fn the_exec_shim_reads_the_limit_it_is_given() {
        let l = |soft, hard| FileLimit { soft, hard };
        for limit in [l(256, 4096), l(1024, u64::MAX), l(u64::MAX, u64::MAX)] {
            assert_eq!(FileLimit::parse(&limit.arg()), Ok(limit));
        }
        assert_eq!(l(1024, u64::MAX).arg(), "1024:unlimited");
        for bad in ["1024", "a:b", "4096:256", ":1"] {
            assert!(FileLimit::parse(bad).is_err(), "{bad}");
        }
    }
}
