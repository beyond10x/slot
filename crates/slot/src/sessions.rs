//! `slot sessions`: which agent sessions build outside the pool, and with
//! what command.
//!
//! Cores come from the daemon's history (`GET /api/history`, one tick per
//! second), averaged over a window. Commands come from a local `/proc` scan:
//! every build process without `SLOT_JOB` in its environment, grouped under
//! the command the session's shell started.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::census::{self, class_of};
use crate::procstat::{self, Proc, Snapshot};

/// One session that builds outside the pool.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Unmanaged {
    /// Session id; empty for processes outside any agent session.
    pub session: String,
    /// Session title, when the transcript names one.
    pub title: String,
    /// Average cores of its builds outside slot over the window.
    pub unmanaged_cores: f64,
    /// Average cores of all its builds over the window.
    pub cores: f64,
    /// Commands running outside slot now.
    pub commands: Vec<Command>,
}

/// One command a session's shell started that runs build processes outside slot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Command {
    /// Pid of the command.
    pub pid: i32,
    /// Its command line, at most 200 characters.
    pub cmdline: String,
    /// Build processes under it.
    pub procs: u32,
}

/// The report: sessions over the threshold, busiest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    /// Seconds of history averaged.
    pub window_s: usize,
    /// False when the daemon could not be read; cores are then 0.
    pub daemon: bool,
    /// Sessions.
    pub sessions: Vec<Unmanaged>,
}

#[derive(serde::Deserialize)]
struct Tick {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    census: Option<crate::types::HostCensus>,
}

/// Average cores per session over the last `window` ticks of history.
pub fn averages(history: &str, window: usize) -> Result<BTreeMap<String, (f64, f64, String)>> {
    let ticks: Vec<serde_json::Value> = serde_json::from_str(history).context("/api/history")?;
    let ticks: Vec<Tick> = ticks
        .into_iter()
        .filter_map(|t| serde_json::from_value(t).ok())
        .filter(|t: &Tick| t.kind == "tick" && t.census.is_some())
        .collect();
    let recent = &ticks[ticks.len().saturating_sub(window)..];
    let n = recent.len().max(1) as f64;
    let mut out: BTreeMap<String, (f64, f64, String)> = BTreeMap::new();
    for t in recent {
        for s in &t.census.as_ref().expect("filtered").sessions {
            let e = out.entry(s.session.clone()).or_default();
            e.0 += s.unmanaged_cores / n;
            e.1 += s.cores / n;
            if !s.title.is_empty() {
                e.2.clone_from(&s.title);
            }
        }
    }
    Ok(out)
}

const SHELLS: &[&str] = &[
    "bash",
    "zsh",
    "sh",
    "dash",
    "fish",
    "claude",
    "systemd",
    "init",
    "slot",
    "containerd-shim",
    "tini",
    "docker-init",
];

/// Groups build processes outside slot under the command their shell started.
/// `class` and `in_job` are injected so the grouping can be tested.
pub fn commands(
    procs: &[Proc],
    class: impl Fn(&Proc) -> bool,
    in_job: impl Fn(&Proc) -> bool,
    session: impl Fn(i32) -> String,
    cmdline: impl Fn(i32) -> String,
) -> BTreeMap<String, Vec<Command>> {
    let by_pid: HashMap<i32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut roots: BTreeMap<i32, u32> = BTreeMap::new();
    for p in procs.iter().filter(|p| class(p) && !in_job(p)) {
        let mut cur = p;
        for _ in 0..16 {
            match by_pid.get(&cur.ppid) {
                Some(parent) if !SHELLS.contains(&parent.comm.as_str()) && parent.pid > 1 => {
                    cur = parent;
                }
                _ => break,
            }
        }
        *roots.entry(cur.pid).or_default() += 1;
    }
    let mut out: BTreeMap<String, Vec<Command>> = BTreeMap::new();
    for (pid, n) in roots {
        let mut line = cmdline(pid);
        if line.chars().count() > 200 {
            line = line.chars().take(199).collect::<String>() + "…";
        }
        out.entry(session(pid)).or_default().push(Command {
            pid,
            cmdline: line,
            procs: n,
        });
    }
    out
}

/// Builds the report: daemon averages joined with the local scan.
pub fn report(addr: &str, window: usize, min_cores: f64) -> Report {
    let history = crate::status::get(addr, "/api/history").and_then(|h| averages(&h, window));
    let daemon = history.is_ok();
    let mut avg = history.unwrap_or_default();
    let snap = Snapshot::scan();
    let procs: Vec<Proc> = snap.procs().cloned().collect();
    let cmds = commands(
        &procs,
        |p| class_of(&p.comm, &census::cmdline(p.pid)).is_some(),
        |p| procstat::env_value(p.pid, crate::server::JOB_ENV).is_some(),
        census::session_of,
        |pid| census::cmdline(pid).join(" "),
    );
    let mut sessions: Vec<Unmanaged> = Vec::new();
    for (session, commands) in cmds {
        let (u, c, title) = avg.remove(&session).unwrap_or_default();
        sessions.push(Unmanaged {
            title: if title.is_empty() {
                census::session_title(&session)
            } else {
                title
            },
            session,
            unmanaged_cores: u,
            cores: c,
            commands,
        });
    }
    for (session, (u, c, title)) in avg {
        if u >= min_cores {
            sessions.push(Unmanaged {
                session,
                title,
                unmanaged_cores: u,
                cores: c,
                commands: Vec::new(),
            });
        }
    }
    if daemon {
        sessions.retain(|s| {
            s.unmanaged_cores >= min_cores || (!s.session.is_empty() && !s.commands.is_empty())
        });
    }
    sessions.sort_by(|a, b| b.unmanaged_cores.total_cmp(&a.unmanaged_cores));
    Report {
        window_s: window,
        daemon,
        sessions,
    }
}

/// JSON for `slot sessions --json`.
pub fn json(r: &Report) -> String {
    serde_json::to_string_pretty(r).unwrap_or_default()
}

/// Text for `slot sessions`.
pub fn render(r: &Report) -> String {
    let mut out = String::new();
    if !r.daemon {
        out.push_str("slot: daemon not reachable; commands only, no cores\n");
    }
    if r.sessions.is_empty() {
        out.push_str("no builds outside slot\n");
        return out;
    }
    for s in &r.sessions {
        let id = if s.session.is_empty() {
            "(no session)"
        } else {
            &s.session
        };
        let title = if s.title.is_empty() {
            String::new()
        } else {
            format!(" \"{}\"", s.title)
        };
        let _ = writeln!(
            out,
            "{id}{title}: {:.1} cores outside slot, {:.1} total (avg {} s)",
            s.unmanaged_cores, s.cores, r.window_s
        );
        for c in &s.commands {
            let _ = writeln!(out, "  pid {} ({} procs): {}", c.pid, c.procs, c.cmdline);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: i32, ppid: i32, comm: &str) -> Proc {
        Proc {
            pid,
            ppid,
            ticks: 0,
            rss: 0,
            comm: comm.into(),
            start: 0,
        }
    }

    #[test]
    fn groups_under_the_command_the_shell_started() {
        let procs = vec![
            p(10, 1, "zsh"),
            p(11, 10, "cargo"),
            p(12, 11, "rustc"),
            p(13, 11, "rustc"),
            p(20, 1, "bash"),
            p(21, 20, "slot"),
            p(22, 21, "cargo"),
            p(23, 22, "rustc"),
        ];
        let got = commands(
            &procs,
            |p| p.comm == "rustc",
            |p| p.pid >= 20,
            |pid| {
                if pid == 11 {
                    "s1".into()
                } else {
                    String::new()
                }
            },
            |pid| format!("cmd{pid}"),
        );
        assert_eq!(got.len(), 1);
        assert_eq!(
            got["s1"],
            vec![Command {
                pid: 11,
                cmdline: "cmd11".into(),
                procs: 2
            }]
        );
    }

    #[test]
    fn averages_the_last_ticks() {
        let tick = |u: f64| {
            format!(
                r#"{{"type":"tick","census":{{"host_cores":0,"build_cores":0,"unmanaged_cores":{u},"psi_cpu":0,"psi_memory":0,"psi_io":0,"load1":0,"classes":[],"sessions":[{{"session":"a","title":"T","cores":{u},"unmanaged_cores":{u},"rss":0,"procs":1}}],"recommended_tokens":0,"unmanaged_smoothed":0}}}}"#
            )
        };
        let h = format!(
            "[{},{},{},{{\"type\":\"other\"}}]",
            tick(9.0),
            tick(2.0),
            tick(4.0)
        );
        let a = averages(&h, 2).unwrap();
        assert!((a["a"].0 - 3.0).abs() < 1e-9, "{a:?}");
        assert_eq!(a["a"].2, "T");
    }

    #[test]
    fn renders_one_line_per_session_and_command() {
        let r = Report {
            window_s: 60,
            daemon: true,
            sessions: vec![Unmanaged {
                session: "a".into(),
                title: "T".into(),
                unmanaged_cores: 3.0,
                cores: 4.0,
                commands: vec![Command {
                    pid: 5,
                    cmdline: "npx vitest run".into(),
                    procs: 7,
                }],
            }],
        };
        assert_eq!(
            render(&r),
            "a \"T\": 3.0 cores outside slot, 4.0 total (avg 60 s)\n  pid 5 (7 procs): npx vitest run\n"
        );
    }
}
