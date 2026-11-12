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
    /// `host:pid` of the client's parent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client: String,
    /// `slot cancel`: cancel this job instead of running `argv`. Only the
    /// user who submitted the job may. Daemons older than 0.5.1 answer
    /// "empty command".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<i64>,
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
}
