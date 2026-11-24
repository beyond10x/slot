//! The host configuration file, `cluster.toml` (`slot.hostconfig` of the
//! specification): read by the daemon at start. Of its tables this module
//! uses `[remote.<name>]`, the remote nodes the daemon polls; `[node]`,
//! `[provider.*]` and `[template.*]` are accepted and not yet used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Default path: `$XDG_CONFIG_HOME/slot/cluster.toml`, else
/// `~/.config/slot/cluster.toml`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map_or_else(
        || {
            std::env::var_os("HOME")
                .map_or_else(|| PathBuf::from("."), PathBuf::from)
                .join(".config")
        },
        PathBuf::from,
    );
    base.join("slot").join("cluster.toml")
}

/// One `[remote.<name>]` table (`slot.hostconfig.RemoteNodeTable`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteNodeTable {
    /// The ssh command line that reaches the node, split into words
    /// without a shell.
    pub ssh: String,
    /// Path of `slot` on the node; `slot` when absent.
    #[serde(default)]
    pub slot_path: Option<String>,
    /// End of the node's lease, RFC 3339.
    #[serde(default)]
    pub deadline: Option<String>,
    /// Price per hour, cents.
    #[serde(default)]
    pub price_per_hour_cents: Option<u32>,
    /// Where the node runs.
    #[serde(default)]
    pub location: Option<String>,
    /// The node's machine type.
    #[serde(default)]
    pub machine_type: Option<String>,
    /// Whether the daemon hands queued local jobs to the node; `true`
    /// when absent.
    #[serde(default)]
    pub spill: Option<bool>,
}

/// What the file says about handing local jobs to remote nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpillConf {
    /// Seconds a job waits in the local queue before it may go to a node.
    pub after_s: u64,
}

/// The whole file (`slot.hostconfig.ClusterFile`). Every table is closed:
/// a key the shape does not declare is refused.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterFile {
    /// `[node.local]`.
    #[serde(default)]
    pub node: Option<toml::Table>,
    /// `[provider.<name>]`.
    #[serde(default)]
    pub provider: BTreeMap<String, toml::Table>,
    /// `[template.<name>]`.
    #[serde(default)]
    pub template: BTreeMap<String, toml::Table>,
    /// `[remote.<name>]`.
    #[serde(default)]
    pub remote: BTreeMap<String, RemoteNodeTable>,
    /// Seconds a job waits in the local queue before it may go to a
    /// remote node; 0 when absent.
    #[serde(default)]
    pub spill_after_s: Option<u64>,
}

/// A remote node of the file, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNode {
    /// Node name.
    pub name: String,
    /// The ssh command, as words.
    pub ssh: Vec<String>,
    /// The table as written.
    pub table: RemoteNodeTable,
}

impl RemoteNode {
    /// The command that runs the `state` step on the node.
    pub fn state_command(&self) -> Vec<String> {
        let mut argv = self.ssh.clone();
        argv.push(
            self.table
                .slot_path
                .clone()
                .unwrap_or_else(|| "slot".to_string()),
        );
        argv.push("state".to_string());
        argv
    }
}

/// Parses the file's text into its remote nodes, in name order.
pub fn parse(text: &str) -> Result<Vec<RemoteNode>, String> {
    parse_all(text).map(|(nodes, _)| nodes)
}

/// Parses the file's text into its remote nodes, in name order, and its
/// spill settings.
pub fn parse_all(text: &str) -> Result<(Vec<RemoteNode>, SpillConf), String> {
    let file: ClusterFile = toml::from_str(text).map_err(|e| e.to_string())?;
    let spill = SpillConf {
        after_s: file.spill_after_s.unwrap_or(0),
    };
    let mut out = Vec::new();
    for (name, table) in file.remote {
        if name.is_empty() || name == crate::types::LOCAL_NODE {
            return Err(format!("[remote.{name}]: the name `{name}` is taken"));
        }
        let ssh =
            crate::shell::split(&table.ssh).map_err(|e| format!("[remote.{name}] ssh: {e}"))?;
        if ssh.is_empty() {
            return Err(format!("[remote.{name}] ssh: empty command"));
        }
        if let Some(d) = &table.deadline
            && time::OffsetDateTime::parse(d, &time::format_description::well_known::Rfc3339)
                .is_err()
        {
            return Err(format!("[remote.{name}] deadline: `{d}` is not RFC 3339"));
        }
        out.push(RemoteNode { name, ssh, table });
    }
    Ok((out, spill))
}

/// Reads the file at `path`: no file is no remote node.
pub fn load(path: &Path) -> Result<Vec<RemoteNode>, String> {
    load_all(path).map(|(nodes, _)| nodes)
}

/// Reads the file at `path` with its spill settings: no file is no
/// remote node.
pub fn load_all(path: &Path) -> Result<(Vec<RemoteNode>, SpillConf), String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_all(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[node.local]
cpu_percent = 50

[remote.build-1]
ssh = "ssh -o BatchMode=yes build-1.example"
slot_path = "/opt/slot/bin/slot"
deadline = "2030-01-01T12:00:00Z"
price_per_hour_cents = 250
location = "region-a"
machine_type = "64-vcpu"

[remote.build-2]
ssh = "ssh build-2.example"
"#;

    #[test]
    fn remote_tables_are_read_in_name_order() {
        let nodes = parse(EXAMPLE).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "build-1");
        assert_eq!(
            nodes[0].state_command(),
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "build-1.example",
                "/opt/slot/bin/slot",
                "state"
            ]
        );
        assert_eq!(nodes[0].table.price_per_hour_cents, Some(250));
        assert_eq!(
            nodes[1].state_command(),
            ["ssh", "build-2.example", "slot", "state"]
        );
    }

    #[test]
    fn a_broken_table_is_refused_by_name() {
        for (bad, says) in [
            ("[remote.a]\nssh = \"ssh a\"\nsecret = \"x\"\n", "secret"),
            ("[remote.a]\n", "ssh"),
            ("[remote.a]\nssh = \"ssh a; id\"\n", "shell character"),
            ("[remote.a]\nssh = \"\"\n", "empty"),
            ("[remote.local]\nssh = \"ssh a\"\n", "taken"),
            (
                "[remote.a]\nssh = \"ssh a\"\ndeadline = \"soon\"\n",
                "RFC 3339",
            ),
            ("[other]\nx = 1\n", "other"),
        ] {
            let e = parse(bad).unwrap_err();
            assert!(e.contains(says), "{bad:?}: {e}");
        }
    }

    #[test]
    fn no_file_is_no_remote_node() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(&d.path().join("cluster.toml")), Ok(Vec::new()));
    }
}
