//! `slot status`: the pool and its jobs, read from the daemon's web API.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::types::{Job, JobState, State};

/// Reads `GET /api/state` from the daemon's web address.
pub fn fetch(addr: &str) -> Result<State> {
    let mut s = TcpStream::connect(addr).with_context(|| format!("connect {addr}"))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(s, "GET /api/state HTTP/1.0\r\nHost: {addr}\r\n\r\n")?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        bail!("{addr}: no HTTP response");
    };
    if !head.lines().next().is_some_and(|l| l.contains(" 200")) {
        bail!("{addr}: {}", head.lines().next().unwrap_or(""));
    }
    serde_json::from_str(body).with_context(|| format!("{addr}: /api/state"))
}

fn secs(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, s / 60 % 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn line(j: &Job, now: i64) -> String {
    let (state, since) = match j.state {
        JobState::Queued => ("queued", j.enqueued_ms),
        _ => ("running", j.started_ms),
    };
    let mut cmd = j.argv.join(" ");
    if cmd.chars().count() > 60 {
        cmd = cmd.chars().take(59).collect::<String>() + "…";
    }
    format!(
        "#{:<5} {:<7} {:<9} {:>2} slot(s) {:>7}  {}  {}",
        j.id,
        state,
        j.toolchain,
        j.tokens,
        secs(now - since),
        j.repo,
        cmd
    )
}

/// The pool line, then one line per queued or running job, queued first.
pub fn render(s: &State) -> String {
    let queued: Vec<&Job> = s
        .active
        .iter()
        .filter(|j| j.state == JobState::Queued)
        .collect();
    let running: Vec<&Job> = s
        .active
        .iter()
        .filter(|j| j.state == JobState::Running)
        .collect();
    let mut out = format!(
        "{} slots, {} free, {} running, {} queued\n",
        s.tokens,
        s.free,
        running.len(),
        queued.len()
    );
    for j in queued.iter().chain(running.iter()) {
        out.push_str(&line(j, s.now_ms));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: i64, state: JobState, tokens: u32) -> Job {
        Job {
            id,
            argv: vec!["cargo".into(), "test".into()],
            cwd: "/w".into(),
            repo: "demo".into(),
            session: String::new(),
            client: String::new(),
            toolchain: "rust".into(),
            remote: false,
            tokens,
            state,
            enqueued_ms: 1_000,
            started_ms: 31_000,
            ended_ms: 0,
            exit: 0,
            cpu_s: 0.0,
            compile_server_cpu_s: 0.0,
            peak_rss: 0,
        }
    }

    #[test]
    fn lists_queued_before_running() {
        let s = State {
            host: "h".into(),
            cores: 4,
            tokens: 4,
            free: 0,
            active: vec![job(7, JobState::Running, 4), job(8, JobState::Queued, 1)],
            done: vec![],
            now_ms: 151_000,
            toolchains: vec![],
        };
        let out = render(&s);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "4 slots, 0 free, 1 running, 1 queued");
        assert!(lines[1].starts_with("#8     queued"), "{out}");
        assert!(lines[1].contains("2m30s"), "{out}");
        assert!(lines[2].starts_with("#7     running"), "{out}");
        assert!(lines[2].contains("2m00s"), "{out}");
    }
}
