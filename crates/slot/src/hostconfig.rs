//! The host configuration file, `cluster.toml` (`slot.hostconfig` of the
//! specification): the one registry of a host's build nodes. The daemon
//! reads it at start and again at each poll after it changed. Of its tables
//! this module uses `[remote.<name>]`, the remote nodes the daemon polls,
//! and the routing keys `repositories` and `default_node` that `slot node
//! route` reads; `[node]`, `[provider.*]` and `[template.*]` are accepted
//! and not yet used. The client reads `never_sync_file` from it
//! (`never_sync.rs`). `slot node add` and `remove` edit it in place
//! ([`add`], [`remove`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// slot's configuration directory: `$XDG_CONFIG_HOME/slot`, else
/// `~/.config/slot`.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || {
                std::env::var_os("HOME")
                    .map_or_else(|| PathBuf::from("."), PathBuf::from)
                    .join(".config")
            },
            PathBuf::from,
        );
    base.join("slot")
}

/// Default path: `$XDG_CONFIG_HOME/slot/cluster.toml`, else
/// `~/.config/slot/cluster.toml`.
pub fn default_path() -> PathBuf {
    config_dir().join("cluster.toml")
}

/// The host configuration file a client reads: `SLOT_CLUSTER_FILE`, else
/// [`default_path`]; `None` for `off`.
pub fn client_path() -> Option<PathBuf> {
    match std::env::var_os("SLOT_CLUSTER_FILE").filter(|v| !v.is_empty()) {
        Some(v) if v == "off" => None,
        Some(v) => Some(PathBuf::from(v)),
        None => Some(default_path()),
    }
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
    /// Repositories whose jobs go to the node (`slot node route`), by the
    /// name `slot status` shows.
    #[serde(default)]
    pub repositories: Vec<String>,
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
    /// The never-sync list of remote mode (`never_sync.rs`), an absolute
    /// path; `$XDG_CONFIG_HOME/slot/remote-deny` when absent.
    #[serde(default)]
    pub never_sync_file: Option<String>,
    /// The node of every repository no `[remote.<name>]` entry lists.
    #[serde(default)]
    pub default_node: Option<String>,
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

    /// The command that runs the `output` step on the node: the kept
    /// output of its job `id` from chunk `from` on.
    pub fn output_command(&self, id: i64, from: u64) -> Vec<String> {
        let mut argv = self.state_command();
        argv.pop();
        argv.extend(["output".to_string(), id.to_string(), from.to_string()]);
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
    parse_registry(text).map(|r| (r.nodes, r.spill))
}

/// The file's build nodes and what routes a repository to one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    /// The `[remote.<name>]` entries, in name order.
    pub nodes: Vec<RemoteNode>,
    /// The spill settings.
    pub spill: SpillConf,
    /// The node of every repository no entry lists.
    pub default_node: Option<String>,
}

/// Parses the file's text into its registry. A repository listed under two
/// nodes is refused: a job has one node.
pub fn parse_registry(text: &str) -> Result<Registry, String> {
    let file: ClusterFile = toml::from_str(text).map_err(|e| e.to_string())?;
    let spill = SpillConf {
        after_s: file.spill_after_s.unwrap_or(0),
    };
    let mut out = Vec::new();
    let mut routed: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, table) in &file.remote {
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
        for repo in &table.repositories {
            if repo.is_empty() {
                return Err(format!("[remote.{name}] repositories: an empty name"));
            }
            if let Some(other) = routed.insert(repo, name)
                && other != name
            {
                return Err(format!(
                    "repository `{repo}` is listed under [remote.{other}] and [remote.{name}]"
                ));
            }
        }
        out.push(RemoteNode {
            name: name.clone(),
            ssh,
            table: table.clone(),
        });
    }
    Ok(Registry {
        nodes: out,
        spill,
        default_node: file.default_node,
    })
}

/// The node a job of repository `repo` goes to: the entry whose
/// `repositories` lists it, else `default_node`. `None` is a directory
/// outside a git checkout: it goes to `default_node`.
pub fn route<'a>(reg: &'a Registry, repo: Option<&str>) -> Result<&'a RemoteNode, String> {
    let listed = repo.and_then(|r| {
        reg.nodes
            .iter()
            .find(|n| n.table.repositories.iter().any(|x| x == r))
    });
    if let Some(n) = listed {
        return Ok(n);
    }
    let Some(name) = reg.default_node.as_deref() else {
        return Err(match repo {
            Some(r) => {
                format!("no node for repository `{r}`: no entry lists it and no default_node")
            }
            None => "no node: not in a git checkout and no default_node".to_string(),
        });
    };
    reg.nodes
        .iter()
        .find(|n| n.name == name)
        .ok_or_else(|| format!("default_node `{name}` has no [remote.{name}] entry"))
}

/// `word` as one shell word: as it is when it needs no quotes, else in
/// single quotes.
fn sh_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=@%+,".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// What a client needs to send a job to `node`, as shell assignments for
/// `eval`: `SLOT_HOST` (the last word of its ssh line), `SLOT_SSH` (the
/// words before it), `SLOT_REMOTE` (`slot` on the node), `SLOT_NODE_END`
/// (its deadline, Unix seconds; left out without one), then an `export`
/// line. Refused for an ssh line without a word before the host, or with a
/// word a client could not split again.
pub fn assignments(node: &RemoteNode) -> Result<String, String> {
    use std::fmt::Write as _;
    let name = &node.name;
    let (host, ssh) = node
        .ssh
        .split_last()
        .filter(|(_, ssh)| !ssh.is_empty())
        .ok_or_else(|| format!("[remote.{name}] ssh: no command before the host"))?;
    if let Some(w) = ssh.iter().find(|w| w.contains(char::is_whitespace)) {
        return Err(format!("[remote.{name}] ssh: the word `{w}` holds a space"));
    }
    let remote = node.table.slot_path.as_deref().unwrap_or("slot");
    let mut out = format!(
        "SLOT_HOST={}\nSLOT_SSH={}\nSLOT_REMOTE={}\n",
        sh_quote(host),
        sh_quote(&ssh.join(" ")),
        sh_quote(remote)
    );
    let mut names = "SLOT_HOST SLOT_SSH SLOT_REMOTE".to_string();
    if let Some(d) = &node.table.deadline {
        let end = time::OffsetDateTime::parse(d, &time::format_description::well_known::Rfc3339)
            .map_err(|_| format!("[remote.{name}] deadline: `{d}` is not RFC 3339"))?;
        let _ = writeln!(out, "SLOT_NODE_END={}", end.unix_timestamp());
        names += " SLOT_NODE_END";
    }
    let _ = writeln!(out, "export {names}");
    Ok(out)
}

/// `slot node route`: the assignments of the node a job in `dir` goes to,
/// by the file's text. The repository is the one `slot status` names: the
/// directory of the checkout's common git directory.
pub fn route_in(text: &str, dir: &Path) -> Result<String, String> {
    let reg = parse_registry(text)?;
    let (_, repo) = crate::client::git_info(dir);
    route(&reg, repo.as_deref()).and_then(assignments)
}

/// `slot node list`: one line per registered node with its state in the
/// daemon's `rows` (`-` when the daemon did not answer or does not show
/// it yet), whether it takes queued local jobs, whether it is the default,
/// and its repositories.
pub fn render_list(reg: &Registry, rows: &[crate::types::Node]) -> String {
    use std::fmt::Write as _;
    if reg.nodes.is_empty() {
        return "no build node registered\n".to_string();
    }
    let mut out = format!(
        "{:<20} {:<8} {:<5} {:<7} {}\n",
        "node", "state", "spill", "default", "repositories"
    );
    for n in &reg.nodes {
        let state = rows
            .iter()
            .find(|r| r.node_id == n.name)
            .map_or_else(|| "-".to_string(), |r| format!("{:?}", r.state));
        let yes = |b: bool| if b { "yes" } else { "no" };
        let _ = writeln!(
            out,
            "{:<20} {:<8} {:<5} {:<7} {}",
            n.name,
            state,
            yes(n.table.spill != Some(false)),
            yes(reg.default_node.as_deref() == Some(n.name.as_str())),
            n.table.repositories.join(", ")
        );
    }
    if let Some(d) = &reg.default_node
        && !reg.nodes.iter().any(|n| &n.name == d)
    {
        let _ = writeln!(out, "default_node `{d}` has no [remote.{d}] entry");
    }
    out
}

/// Writes `text` to `path` in one step: a new file beside it, then a
/// rename, so a reader sees the old file or the new one.
pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// A node `slot node add` writes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewNode {
    /// Node name.
    pub name: String,
    /// The ssh command line.
    pub ssh: String,
    /// Path of `slot` on the node.
    pub slot_path: Option<String>,
    /// End of the node's lease, RFC 3339.
    pub deadline: Option<String>,
    /// Price per hour, cents.
    pub price_per_hour_cents: Option<u32>,
    /// Whether the daemon hands queued local jobs to the node.
    pub spill: Option<bool>,
    /// Repositories routed to the node.
    pub repositories: Vec<String>,
    /// Whether the node becomes `default_node`.
    pub default: bool,
}

/// `text` parsed for an edit: refused unless it is a valid file.
fn editable(text: &str) -> Result<(Registry, toml_edit::DocumentMut), String> {
    let reg = parse_registry(text)?;
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    Ok((reg, doc))
}

/// The `[remote]` table of `doc`, made when the file has none.
fn remote_table(doc: &mut toml_edit::DocumentMut) -> Result<&mut dyn toml_edit::TableLike, String> {
    if doc.get("remote").is_none() {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc.insert("remote", toml_edit::Item::Table(t));
    }
    doc.get_mut("remote")
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| "remote: not a table".to_string())
}

/// `text` with the node `n` added as a `[remote.<name>]` entry at the end,
/// each repository of `n` taken out of the entry that listed it, and
/// `default_node` set when `n.default`. The rest of the text stays as it
/// was. Refused for a text that is not a valid file, a name that is taken
/// or not lower-case letters, digits and `-`, and an entry the file would
/// then refuse.
pub fn add(text: &str, n: &NewNode) -> Result<String, String> {
    let (reg, mut doc) = editable(text)?;
    let name = n.name.as_str();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "node name `{name}`: lower-case letters, digits and `-`"
        ));
    }
    if name == crate::types::LOCAL_NODE || reg.nodes.iter().any(|x| x.name == name) {
        return Err(format!("node `{name}` is already registered"));
    }
    let remote = remote_table(&mut doc)?;
    for (_, item) in remote.iter_mut() {
        if let Some(list) = item
            .as_table_like_mut()
            .and_then(|t| t.get_mut("repositories"))
            .and_then(toml_edit::Item::as_array_mut)
        {
            list.retain(|v| {
                v.as_str()
                    .is_none_or(|r| !n.repositories.iter().any(|x| x == r))
            });
        }
    }
    let mut t = toml_edit::Table::new();
    t.insert("ssh", toml_edit::value(n.ssh.as_str()));
    if let Some(p) = &n.slot_path {
        t.insert("slot_path", toml_edit::value(p.as_str()));
    }
    if let Some(d) = &n.deadline {
        t.insert("deadline", toml_edit::value(d.as_str()));
    }
    if let Some(c) = n.price_per_hour_cents {
        t.insert("price_per_hour_cents", toml_edit::value(i64::from(c)));
    }
    if let Some(s) = n.spill {
        t.insert("spill", toml_edit::value(s));
    }
    if !n.repositories.is_empty() {
        let list: toml_edit::Array = n.repositories.iter().map(String::as_str).collect();
        t.insert("repositories", toml_edit::value(list));
    }
    remote.insert(name, toml_edit::Item::Table(t));
    if n.default {
        doc.insert("default_node", toml_edit::value(name));
    }
    let out = doc.to_string();
    parse_registry(&out)?;
    Ok(out)
}

/// `text` without the `[remote.<name>]` entry, and without `default_node`
/// when it named the node. Refused for a text that is not a valid file and
/// a name it has no entry for.
pub fn remove(text: &str, name: &str) -> Result<String, String> {
    let (reg, mut doc) = editable(text)?;
    if !reg.nodes.iter().any(|x| x.name == name) {
        return Err(format!("no node `{name}` is registered"));
    }
    remote_table(&mut doc)?.remove(name);
    if reg.default_node.as_deref() == Some(name) {
        doc.remove("default_node");
    }
    let out = doc.to_string();
    parse_registry(&out)?;
    Ok(out)
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
        assert_eq!(
            nodes[1].output_command(7, 12),
            ["ssh", "build-2.example", "slot", "output", "7", "12"]
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

    const ROUTED: &str = r#"# Build nodes of this host.
default_node = "build-1"

[remote.build-1]
ssh = "ssh -o BatchMode=yes build-1.example"
repositories = ["repo-a"]

# The second node: no hand-off.
[remote.build-2]
ssh = "ssh build-2.example"
slot_path = "/opt/slot/bin/slot"
deadline = "2030-01-01T12:00:00Z"
spill = false
repositories = ["repo-b", "repo-c"]
"#;

    #[test]
    fn a_repository_goes_to_the_node_that_lists_it_else_to_the_default() {
        let reg = parse_registry(ROUTED).unwrap();
        assert_eq!(reg.default_node.as_deref(), Some("build-1"));
        assert_eq!(reg.nodes[1].table.repositories, ["repo-b", "repo-c"]);
        assert_eq!(route(&reg, Some("repo-c")).unwrap().name, "build-2");
        assert_eq!(route(&reg, Some("repo-a")).unwrap().name, "build-1");
        assert_eq!(route(&reg, Some("other")).unwrap().name, "build-1");
        assert_eq!(route(&reg, None).unwrap().name, "build-1");
        assert_eq!(
            assignments(route(&reg, Some("repo-b")).unwrap()).unwrap(),
            "SLOT_HOST=build-2.example\nSLOT_SSH=ssh\nSLOT_REMOTE=/opt/slot/bin/slot\n\
             SLOT_NODE_END=1893499200\nexport SLOT_HOST SLOT_SSH SLOT_REMOTE SLOT_NODE_END\n"
        );
        assert_eq!(
            assignments(route(&reg, Some("repo-a")).unwrap()).unwrap(),
            "SLOT_HOST=build-1.example\nSLOT_SSH='ssh -o BatchMode=yes'\nSLOT_REMOTE=slot\n\
             export SLOT_HOST SLOT_SSH SLOT_REMOTE\n"
        );

        let no_default =
            parse_registry(&ROUTED.replace("default_node = \"build-1\"\n", "")).unwrap();
        assert!(route(&no_default, Some("other")).is_err());
        let missing = parse_registry(&ROUTED.replace("\"build-1\"", "\"build-9\"")).unwrap();
        let e = route(&missing, Some("other")).unwrap_err();
        assert!(e.contains("build-9"), "{e}");
        let twice = ROUTED.replace("[\"repo-b\", ", "[\"repo-a\", ");
        let e = parse_registry(&twice).unwrap_err();
        assert!(e.contains("repo-a"), "{e}");
    }

    /// `slot node route` in a checkout: a linked worktree of `repo-b` goes
    /// to the node that lists `repo-b`, another repository and a directory
    /// outside git to the default node, and nothing matches without one.
    #[test]
    fn route_in_a_checkout_names_the_repository_of_its_common_git_directory() {
        let t = tempfile::tempdir().unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
        };
        let b = t.path().join("repo-b");
        let other = t.path().join("other");
        let plain = t.path().join("plain");
        for d in [&b, &other, &plain] {
            std::fs::create_dir(d).unwrap();
        }
        git(&b, &["init", "-q"]);
        git(&other, &["init", "-q"]);
        git(
            &b,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        );
        let wt = t.path().join("wt-1");
        git(&b, &["worktree", "add", "-q", wt.to_str().unwrap()]);
        assert!(
            route_in(ROUTED, &wt)
                .unwrap()
                .starts_with("SLOT_HOST=build-2.example\n")
        );
        assert!(
            route_in(ROUTED, &other)
                .unwrap()
                .starts_with("SLOT_HOST=build-1.example\n")
        );
        assert!(
            route_in(ROUTED, &plain)
                .unwrap()
                .starts_with("SLOT_HOST=build-1.example\n")
        );
        let bare = ROUTED.replace("default_node = \"build-1\"\n", "");
        assert!(route_in(&bare, &other).unwrap_err().contains("`other`"));
        assert!(route_in("", &other).is_err());
    }

    #[test]
    fn the_list_names_each_node_with_its_state_spill_default_and_repositories() {
        let reg = parse_registry(ROUTED).unwrap();
        let rows = [crate::types::Node {
            node_id: "build-2".into(),
            state: crate::types::NodeState::Ready,
            ..crate::types::Node::default()
        }];
        let text = render_list(&reg, &rows);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        let words = |l: &str| l.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(words(lines[1]), ["build-1", "-", "yes", "yes", "repo-a"]);
        assert_eq!(
            words(lines[2]),
            ["build-2", "Ready", "no", "no", "repo-b,", "repo-c"]
        );
        assert_eq!(
            render_list(&Registry::default(), &[]),
            "no build node registered\n"
        );
    }

    #[test]
    fn adding_then_removing_a_node_leaves_the_file_as_it_was() {
        let added = add(
            ROUTED,
            &NewNode {
                name: "build-3".into(),
                ssh: "ssh build-3.example".into(),
                slot_path: Some("/usr/local/bin/slot".into()),
                deadline: Some("2030-02-01T00:00:00Z".into()),
                price_per_hour_cents: Some(120),
                spill: Some(false),
                repositories: vec!["repo-d".into()],
                default: false,
            },
        )
        .unwrap();
        assert!(
            added.starts_with("# Build nodes of this host.\n"),
            "{added}"
        );
        assert!(added.contains("# The second node: no hand-off."), "{added}");
        let reg = parse_registry(&added).unwrap();
        let n = reg.nodes.iter().find(|n| n.name == "build-3").unwrap();
        assert_eq!(n.table.repositories, ["repo-d"]);
        assert_eq!(n.table.spill, Some(false));
        assert_eq!(n.table.price_per_hour_cents, Some(120));
        assert_eq!(route(&reg, Some("repo-d")).unwrap().name, "build-3");
        assert_eq!(remove(&added, "build-3").unwrap(), ROUTED);

        // A default node added to a file without one, and taken out again.
        let bare = ROUTED.replace("default_node = \"build-1\"\n", "");
        let added = add(
            &bare,
            &NewNode {
                name: "build-3".into(),
                ssh: "ssh build-3.example".into(),
                default: true,
                ..NewNode::default()
            },
        )
        .unwrap();
        let reg = parse_registry(&added).unwrap();
        assert_eq!(reg.default_node.as_deref(), Some("build-3"));
        assert_eq!(remove(&added, "build-3").unwrap(), bare);
    }

    #[test]
    fn add_moves_a_repository_and_remove_clears_the_default() {
        let added = add(
            ROUTED,
            &NewNode {
                name: "build-3".into(),
                ssh: "ssh build-3.example".into(),
                repositories: vec!["repo-b".into()],
                ..NewNode::default()
            },
        )
        .unwrap();
        let reg = parse_registry(&added).unwrap();
        assert_eq!(route(&reg, Some("repo-b")).unwrap().name, "build-3");
        assert_eq!(reg.nodes[1].table.repositories, ["repo-c"]);

        let removed = remove(ROUTED, "build-1").unwrap();
        let reg = parse_registry(&removed).unwrap();
        assert_eq!(reg.default_node, None);
        assert_eq!(reg.nodes.len(), 1);
        assert!(remove(ROUTED, "build-9").unwrap_err().contains("build-9"));
        assert!(
            add(
                ROUTED,
                &NewNode {
                    name: "build-1".into(),
                    ssh: "ssh x".into(),
                    ..NewNode::default()
                }
            )
            .is_err()
        );
        assert!(
            add(
                ROUTED,
                &NewNode {
                    name: "local".into(),
                    ssh: "ssh x".into(),
                    ..NewNode::default()
                }
            )
            .is_err()
        );
        assert!(
            add(
                ROUTED,
                &NewNode {
                    name: "b".into(),
                    ssh: "ssh x; id".into(),
                    ..NewNode::default()
                }
            )
            .is_err()
        );
        assert!(
            add(
                "[remote.a\n",
                &NewNode {
                    name: "b".into(),
                    ssh: "ssh x".into(),
                    ..NewNode::default()
                }
            )
            .is_err()
        );
        assert!(remove("[remote.a\n", "a").is_err());
    }

    #[test]
    fn no_file_is_no_remote_node() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(&d.path().join("cluster.toml")), Ok(Vec::new()));
    }
}
