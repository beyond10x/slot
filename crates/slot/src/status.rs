//! `slot status`: the pool and its jobs, read from the daemon's web API.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, bail};

pub use crate::server::limits::View as LimitsView;
use crate::types::{Job, JobState, State};

/// One HTTP/1.0 request to the daemon's web address; the body of a 200
/// answer. A daemon older than the client answers an API path it does not
/// have with its web UI page (200, `text/html`), so a body that is not JSON
/// is refused with that explanation.
fn request(addr: &str, method: &str, path: &str, body: Option<&str>) -> Result<String> {
    let mut s = TcpStream::connect(addr).with_context(|| format!("connect {addr}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    match body {
        Some(b) => write!(
            s,
            "{method} {path} HTTP/1.0\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        )?,
        None => write!(s, "{method} {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n")?,
    }
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        bail!("{addr}: no HTTP response");
    };
    if !head.lines().next().is_some_and(|l| l.contains(" 200")) {
        bail!(
            "{addr}: {} {}",
            head.lines().next().unwrap_or(""),
            body.trim()
        );
    }
    let json = head.lines().any(|l| {
        l.split_once(':').is_some_and(|(k, v)| {
            k.eq_ignore_ascii_case("content-type") && v.trim().starts_with("application/json")
        })
    });
    if !json {
        bail!(
            "{addr}{path}: the answer is not JSON; the daemon is older than this client and \
             has no {path} (upgrade it: systemctl --user reload slot)"
        );
    }
    Ok(body.to_string())
}

/// Reads `GET /api/state` from the daemon's web address.
pub fn fetch(addr: &str) -> Result<State> {
    let body = request(addr, "GET", "/api/state", None)?;
    serde_json::from_str(&body).with_context(|| format!("{addr}: /api/state"))
}

/// Reads the headroom, or sets it to `percent` first.
pub fn headroom(addr: &str, percent: Option<u8>) -> Result<LimitsView> {
    let body = match percent {
        Some(p) => request(
            addr,
            "PUT",
            "/api/limits",
            Some(&format!(r#"{{"headroom_percent":{p}}}"#)),
        )?,
        None => request(addr, "GET", "/api/limits", None)?,
    };
    serde_json::from_str(&body).with_context(|| format!("{addr}: /api/limits"))
}

/// One line for `slot headroom`.
pub fn render_headroom(v: &LimitsView) -> String {
    let quota = match v.cpu_quota_percent {
        Some(q) => format!("cpu quota {q}% ({:.1} cores)", f64::from(q) / 100.0),
        None if v.headroom_percent == 0 => "no cpu quota".to_string(),
        None => format!("cpu quota not applied: {}", v.note),
    };
    format!(
        "headroom {}%: builds get {} of {} slots, {quota}\n",
        v.headroom_percent, v.effective_tokens, v.tokens
    )
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
    let pool = if s.limit > 0 && s.limit < s.tokens {
        format!(
            "{} of {} slots ({}% headroom)",
            s.limit, s.tokens, s.headroom_percent
        )
    } else {
        format!("{} slots", s.tokens)
    };
    let mut out = format!(
        "{pool}, {} free, {} running, {} queued\n",
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
    fn headroom_line() {
        let mut v = LimitsView {
            headroom_percent: 30,
            cpu_quota_percent: Some(1400),
            effective_tokens: 14,
            tokens: 20,
            systemd: true,
            note: String::new(),
        };
        assert_eq!(
            render_headroom(&v),
            "headroom 30%: builds get 14 of 20 slots, cpu quota 1400% (14.0 cores)\n"
        );
        v.cpu_quota_percent = None;
        v.note = "not running in a systemd user unit".into();
        assert!(
            render_headroom(&v)
                .ends_with("cpu quota not applied: not running in a systemd user unit\n")
        );
    }

    /// Serves one HTTP answer on a free port and returns the address.
    fn answer_once(resp: &'static str) -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            // Read the whole request: closing with unread bytes resets the
            // connection before the client reads the answer.
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let text = String::from_utf8_lossy(&req);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .map_or(0, |n| n.trim().parse().unwrap());
                    if body.len() >= len {
                        break;
                    }
                }
                match c.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let _ = c.write_all(resp.as_bytes());
        });
        addr
    }

    #[test]
    fn an_older_daemon_answering_with_its_page_is_an_error() {
        let addr = answer_once(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\n\r\n<!doctype html>",
        );
        let e = headroom(&addr, Some(30)).unwrap_err().to_string();
        assert!(e.contains("older than this client"), "{e}");
        let addr = answer_once(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"headroom_percent\":30,\
             \"cpu_quota_percent\":null,\"effective_tokens\":14,\"tokens\":20,\"systemd\":false,\
             \"note\":\"\"}",
        );
        assert_eq!(headroom(&addr, None).unwrap().effective_tokens, 14);
    }

    #[test]
    fn an_older_state_without_a_limit_shows_the_pool() {
        let s = State {
            host: "h".into(),
            cores: 4,
            tokens: 4,
            limit: 0,
            headroom_percent: 0,
            cpu_quota_percent: None,
            free: 4,
            active: vec![],
            done: vec![],
            now_ms: 0,
            toolchains: vec![],
        };
        assert_eq!(render(&s), "4 slots, 4 free, 0 running, 0 queued\n");
    }

    #[test]
    fn lists_queued_before_running() {
        let s = State {
            host: "h".into(),
            cores: 4,
            tokens: 4,
            limit: 4,
            headroom_percent: 0,
            cpu_quota_percent: None,
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
