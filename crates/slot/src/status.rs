//! `slot status`: the pool and its jobs, read from the daemon's web API.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, bail};

pub use crate::server::limits::View as LimitsView;
pub use crate::server::priority::View as PriorityView;
use crate::types::{HeadroomBase, Job, JobState, Priority, State};

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

/// The body of `GET <path>` from the daemon's web address.
pub fn get(addr: &str, path: &str) -> Result<String> {
    request(addr, "GET", path, None)
}

/// Reads `GET /api/usage` for the last `since_ms` milliseconds, grouped by `by`.
pub fn fetch_usage(addr: &str, since_ms: i64, by: crate::usage::By) -> Result<crate::usage::Usage> {
    let to = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let by = serde_json::to_value(by)?
        .as_str()
        .unwrap_or("project")
        .to_string();
    let path = format!("/api/usage?from={}&to={to}&by={by}", to - since_ms);
    let body = request(addr, "GET", &path, None)?;
    serde_json::from_str(&body).with_context(|| format!("{addr}: /api/usage"))
}

/// Reads `GET /api/state` from the daemon's web address.
pub fn fetch(addr: &str) -> Result<State> {
    let body = request(addr, "GET", "/api/state", None)?;
    serde_json::from_str(&body).with_context(|| format!("{addr}: /api/state"))
}

/// Reads the headroom, or sets the headroom, the call headroom and the base
/// first.
pub fn headroom(
    addr: &str,
    percent: Option<u8>,
    call: Option<u8>,
    base: Option<HeadroomBase>,
) -> Result<LimitsView> {
    let mut fields = Vec::new();
    if let Some(p) = percent {
        fields.push(format!(r#""headroom_percent":{p}"#));
    }
    if let Some(c) = call {
        fields.push(format!(r#""call_headroom_percent":{c}"#));
    }
    if let Some(b) = base {
        fields.push(format!(r#""headroom_base":"{}""#, b.as_str()));
    }
    let body = if fields.is_empty() {
        request(addr, "GET", "/api/limits", None)?
    } else {
        request(
            addr,
            "PUT",
            "/api/limits",
            Some(&format!("{{{}}}", fields.join(","))),
        )?
    };
    serde_json::from_str(&body).with_context(|| format!("{addr}: /api/limits"))
}

/// Reads the priority rules, or first sets (`Some(level)`) or removes
/// (`None`) the rule for a pattern.
pub fn priority(addr: &str, change: Option<(&str, Option<Priority>)>) -> Result<PriorityView> {
    let body = match change {
        None => request(addr, "GET", "/api/priority", None)?,
        Some((pattern, level)) => {
            let body = serde_json::json!({ "pattern": pattern, "level": level }).to_string();
            request(addr, "PUT", "/api/priority", Some(&body))?
        }
    };
    serde_json::from_str(&body).with_context(|| format!("{addr}: /api/priority"))
}

/// `slot priority list`: one line per rule, then the promotion limit.
pub fn render_priority(v: &PriorityView) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if v.rules.is_empty() {
        out.push_str("no priority rules: every job is normal unless it asks otherwise\n");
    }
    let width = v.rules.iter().map(|r| r.pattern.len()).max().unwrap_or(0);
    for r in &v.rules {
        let _ = writeln!(out, "{:<width$}  {}", r.pattern, r.level);
    }
    if v.promote_after_s > 0 {
        let _ = writeln!(
            out,
            "a job queued longer than {} goes as high",
            secs(v.promote_after_s as i64 * 1000)
        );
    }
    out
}

/// The call mode line of `slot headroom` and `slot status`; empty from a
/// daemon without call mode.
pub fn render_call(v: &LimitsView) -> String {
    let c = &v.call;
    if v.call_headroom_percent == 0 {
        return format!("call mode off{}\n", note(&c.note));
    }
    if !c.active {
        return format!(
            "call mode {}%: no call{}\n",
            v.call_headroom_percent,
            note(&c.note)
        );
    }
    let back = if c.manual {
        "set by hand, stays after the call".to_string()
    } else if v.headroom_percent == c.base_percent {
        "unchanged".to_string()
    } else {
        match c.ends_in_s {
            Some(s) => format!("back to {}% in {s}s", c.base_percent),
            None => format!("back to {}% after the call", c.base_percent),
        }
    };
    format!(
        "call mode {}%: call on ({}), headroom {}%, {back}\n",
        v.call_headroom_percent,
        c.apps.join(", "),
        v.headroom_percent
    )
}

fn note(n: &str) -> String {
    if n.is_empty() {
        String::new()
    } else {
        format!(" ({n})")
    }
}

/// `slot headroom`: the headroom line and the call mode line.
pub fn render_headroom(v: &LimitsView) -> String {
    let machine = v.headroom_base == HeadroomBase::Machine;
    let quota = match v.cpu_quota_percent {
        Some(q) if machine => format!(
            "cpu quota {q}% ({:.1} cores) now, following the CPU used outside the pool",
            f64::from(q) / 100.0
        ),
        Some(q) => format!("cpu quota {q}% ({:.1} cores)", f64::from(q) / 100.0),
        None if v.headroom_percent == 0 => "no cpu quota".to_string(),
        None => format!("cpu quota not applied: {}", v.note),
    };
    format!(
        "headroom {}% of the {}: builds get {} of {} slots, {quota}\n{}",
        v.headroom_percent,
        v.headroom_base.as_str(),
        v.effective_tokens,
        v.tokens,
        render_call(v)
    )
}

/// The machine line of `slot status`; `None` on the pool base or with no
/// headroom.
pub fn machine_line(s: &State) -> Option<String> {
    if s.headroom_base != HeadroomBase::Machine || s.target_cores <= 0.0 {
        return None;
    }
    let quota = match s.cpu_quota_percent {
        Some(q) if s.quota_floor => format!("{:.1} (floor)", f64::from(q) / 100.0),
        Some(q) => format!("{:.1}", f64::from(q) / 100.0),
        None => "not applied".to_string(),
    };
    let owners: Vec<String> = s
        .outside
        .iter()
        .map(|o| format!("{} {:.1}", o.name, o.cores))
        .collect();
    Some(format!(
        "machine{}: {:.1} of {} CPUs busy, target {:.1}, pool quota {quota}, outside {:.1}{}\n",
        if s.over_target { " (over target)" } else { "" },
        s.busy_cores,
        s.cores,
        s.target_cores,
        s.outside_cores,
        if owners.is_empty() {
            String::new()
        } else {
            format!(": {}", owners.join(", "))
        }
    ))
}

pub(crate) fn secs(ms: i64) -> String {
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
    let mut s = format!(
        "#{:<5} {:<7} {:<9} {:>2} slot(s) {:>7}  {}  {}",
        j.id,
        state,
        j.toolchain,
        j.tokens,
        secs(now - since),
        j.repo,
        cmd
    );
    if j.state == JobState::Queued {
        s.push_str("  ");
        s.push_str(j.priority.as_str());
        if j.promoted {
            s.push_str(" (waited)");
        }
    }
    if !j.wait_reason.is_empty() {
        s.push_str("\n       waiting: ");
        s.push_str(&j.wait_reason);
    }
    s
}

/// `sccache: 2 server(s), 0 short of tokens, refills since start: 1 (18 tokens, last 4m10s ago)`.
fn sccache_line(w: &crate::types::SccacheWatch, now: i64) -> String {
    let refills = match w.refills {
        0 => "none".to_string(),
        n => format!(
            "{n} ({} tokens, last {} ago)",
            w.tokens,
            secs(now - w.last_refill_ms)
        ),
    };
    format!(
        "sccache: {} server(s), {} short of tokens, refills since start: {refills}\n",
        w.servers, w.short
    )
}

/// `disk: 32.0 GB free, under 40.0 GB: dry run, would delete 2 target dir(s), 31.0 GB`;
/// nothing when eviction is off or never looked.
fn evict_line(r: &crate::evict::Report) -> Option<String> {
    if r.floor == 0 || r.checked_ms == 0 {
        return None;
    }
    let mode = if r.dry_run { "dry run, " } else { "" };
    let deleted = if r.deleted_bytes > 0 {
        format!(
            ", {} deleted since start",
            crate::admission::gb(r.deleted_bytes)
        )
    } else {
        String::new()
    };
    if r.free >= r.floor && r.last.is_empty() {
        return Some(format!(
            "disk: {} free, eviction below {} ({mode}{} target dir(s) known){deleted}\n",
            crate::admission::gb(r.free),
            crate::admission::gb(r.floor),
            r.known
        ));
    }
    let bytes: u64 = r.last.iter().map(|e| e.bytes).sum();
    let verb = if r.dry_run { "would delete" } else { "deleted" };
    Some(format!(
        "disk: {} free, under {}: {mode}{verb} {} target dir(s), {}{deleted}\n",
        crate::admission::gb(r.free),
        crate::admission::gb(r.floor),
        r.last.len(),
        crate::admission::gb(bytes)
    ))
}

/// The pool line, then one line per queued or running job, queued first,
/// in the order they get slots: by level, then by arrival.
pub fn render(s: &State) -> String {
    let mut queued: Vec<&Job> = s
        .active
        .iter()
        .filter(|j| j.state == JobState::Queued)
        .collect();
    queued.sort_by_key(|j| (j.priority, j.id));
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
    if !s.call_apps.is_empty() {
        out.push_str("call mode: ");
        out.push_str(&s.call_apps.join(", "));
        out.push_str(" in a call, headroom raised\n");
    }
    if let Some(l) = machine_line(s) {
        out.push_str(&l);
    }
    if !s.upgrade.is_empty() {
        out.push_str(&s.upgrade);
        out.push('\n');
    }
    let w = &s.sccache;
    if w.enabled && (w.servers > 0 || w.refills > 0) {
        out.push_str(&sccache_line(w, s.now_ms));
    }
    if let Some(l) = evict_line(&s.evict) {
        out.push_str(&l);
    }
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
            project: String::new(),
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
            wait_reason: String::new(),
            target_dir: String::new(),
            priority: crate::types::Priority::Normal,
            promoted: false,
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
            call_headroom_percent: 40,
            call: crate::server::limits::CallView::default(),
            headroom_base: crate::types::HeadroomBase::Pool,
        };
        assert_eq!(
            render_headroom(&v),
            "headroom 30% of the pool: builds get 14 of 20 slots, cpu quota 1400% (14.0 cores)\n\
             call mode 40%: no call\n"
        );
        v.call.active = true;
        v.call.apps = vec!["Chromium".into()];
        v.call.base_percent = 20;
        v.headroom_percent = 40;
        v.call.ends_in_s = Some(75);
        assert_eq!(
            render_call(&v),
            "call mode 40%: call on (Chromium), headroom 40%, back to 20% in 75s\n"
        );
        v.call.manual = true;
        assert!(render_call(&v).ends_with("set by hand, stays after the call\n"));
        v.call_headroom_percent = 0;
        assert_eq!(render_call(&v), "call mode off\n");
        v.headroom_percent = 30;
        v.cpu_quota_percent = None;
        v.note = "not running in a systemd user unit".into();
        assert!(
            render_headroom(&v)
                .contains("cpu quota not applied: not running in a systemd user unit\n")
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
        let e = headroom(&addr, Some(30), None, None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("older than this client"), "{e}");
        let addr = answer_once(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"headroom_percent\":30,\
             \"cpu_quota_percent\":null,\"effective_tokens\":14,\"tokens\":20,\"systemd\":false,\
             \"note\":\"\"}",
        );
        assert_eq!(
            headroom(&addr, None, None, None).unwrap().effective_tokens,
            14
        );
    }

    #[test]
    fn the_sccache_watchdog_line() {
        let mut s = State {
            tokens: 4,
            free: 4,
            now_ms: 400_000,
            ..State::default()
        };
        assert_eq!(
            render(&s),
            "4 slots, 4 free, 0 running, 0 queued\n",
            "off: no line"
        );
        s.sccache = crate::types::SccacheWatch {
            enabled: true,
            servers: 2,
            short: 1,
            refills: 1,
            tokens: 18,
            last_refill_ms: 150_000,
        };
        assert_eq!(
            render(&s).lines().nth(1),
            Some(
                "sccache: 2 server(s), 1 short of tokens, refills since start: 1 (18 tokens, \
                 last 4m10s ago)"
            )
        );
    }

    #[test]
    fn an_older_state_without_a_limit_shows_the_pool() {
        let s = State {
            host: "h".into(),
            cores: 4,
            tokens: 4,
            limit: 0,
            headroom_percent: 0,
            free: 4,
            active: vec![],
            done: vec![],
            ..State::default()
        };
        assert_eq!(render(&s), "4 slots, 4 free, 0 running, 0 queued\n");
    }

    #[test]
    fn the_machine_line_names_the_load_outside_the_pool() {
        use crate::types::{HeadroomBase, OutsideOwner};
        let mut s = State {
            cores: 20,
            tokens: 20,
            limit: 11,
            headroom_percent: 45,
            free: 11,
            headroom_base: HeadroomBase::Machine,
            cpu_quota_percent: Some(200),
            busy_cores: 18.24,
            target_cores: 11.0,
            outside_cores: 9.1,
            outside: vec![
                OutsideOwner {
                    name: "session-3.scope".into(),
                    cores: 5.9,
                },
                OutsideOwner {
                    name: "docker 3f2a1b2c3d4e".into(),
                    cores: 3.2,
                },
            ],
            quota_floor: true,
            over_target: true,
            ..State::default()
        };
        assert_eq!(
            render(&s),
            "11 of 20 slots (45% headroom), 11 free, 0 running, 0 queued\n\
             machine (over target): 18.2 of 20 CPUs busy, target 11.0, pool quota 2.0 (floor), \
             outside 9.1: session-3.scope 5.9, docker 3f2a1b2c3d4e 3.2\n"
        );
        s.over_target = false;
        s.quota_floor = false;
        s.cpu_quota_percent = Some(430);
        s.outside.clear();
        assert!(
            render(&s).contains(
                "machine: 18.2 of 20 CPUs busy, target 11.0, pool quota 4.3, outside 9.1\n"
            )
        );
        s.cpu_quota_percent = None;
        assert!(render(&s).contains("pool quota not applied,"));
        s.headroom_base = HeadroomBase::Pool;
        assert!(!render(&s).contains("machine"));
    }

    #[test]
    fn headroom_shows_its_base() {
        let mut v = LimitsView {
            headroom_percent: 45,
            cpu_quota_percent: Some(430),
            effective_tokens: 11,
            tokens: 20,
            systemd: true,
            note: String::new(),
            call_headroom_percent: 0,
            call: crate::server::limits::CallView::default(),
            headroom_base: crate::types::HeadroomBase::Machine,
        };
        assert_eq!(
            render_headroom(&v).lines().next().unwrap(),
            "headroom 45% of the machine: builds get 11 of 20 slots, cpu quota 430% (4.3 cores) \
             now, following the CPU used outside the pool"
        );
        v.headroom_base = crate::types::HeadroomBase::Pool;
        assert!(render_headroom(&v).starts_with("headroom 45% of the pool: builds get 11"));
    }

    #[test]
    fn a_pending_upgrade_is_shown_under_the_pool() {
        let s = State {
            tokens: 4,
            free: 4,
            upgrade: "upgrade pending: re-executing at the first moment no job runs".into(),
            ..State::default()
        };
        assert_eq!(
            render(&s),
            "4 slots, 4 free, 0 running, 0 queued\n\
             upgrade pending: re-executing at the first moment no job runs\n"
        );
    }

    #[test]
    fn lists_queued_before_running() {
        let s = State {
            host: "h".into(),
            cores: 4,
            tokens: 4,
            limit: 4,
            free: 0,
            active: vec![job(7, JobState::Running, 4), job(8, JobState::Queued, 1)],
            now_ms: 151_000,
            ..State::default()
        };
        let out = render(&s);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "4 slots, 0 free, 1 running, 1 queued");
        assert!(lines[1].starts_with("#8     queued"), "{out}");
        assert!(lines[1].contains("2m30s"), "{out}");
        assert!(lines[2].starts_with("#7     running"), "{out}");
        assert!(lines[2].contains("2m00s"), "{out}");
    }

    #[test]
    fn queued_jobs_show_their_level_in_start_order() {
        let mut high = job(9, JobState::Queued, 1);
        high.priority = Priority::High;
        let mut aged = job(5, JobState::Queued, 1);
        aged.priority = Priority::High;
        aged.promoted = true;
        let mut low = job(3, JobState::Queued, 1);
        low.priority = Priority::Low;
        let normal = job(4, JobState::Queued, 1);
        let mut running = job(2, JobState::Running, 1);
        running.priority = Priority::Low;
        let s = State {
            tokens: 4,
            limit: 4,
            active: vec![running, low, normal, aged, high],
            now_ms: 151_000,
            ..State::default()
        };
        let out = render(&s);
        let lines: Vec<&str> = out.lines().skip(1).collect();
        assert!(
            lines[0].starts_with("#5 ") && lines[0].ends_with("cargo test  high (waited)"),
            "{out}"
        );
        assert!(
            lines[1].starts_with("#9 ") && lines[1].ends_with("cargo test  high"),
            "{out}"
        );
        assert!(
            lines[2].starts_with("#4 ") && lines[2].ends_with("cargo test  normal"),
            "{out}"
        );
        assert!(
            lines[3].starts_with("#3 ") && lines[3].ends_with("cargo test  low"),
            "{out}"
        );
        assert!(
            lines[4].starts_with("#2 ") && lines[4].ends_with("cargo test"),
            "running: {out}"
        );
    }

    #[test]
    fn a_job_from_an_older_daemon_is_normal() {
        let mut v = serde_json::to_value(job(3, JobState::Queued, 1)).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("priority");
        o.remove("promoted");
        let j: Job = serde_json::from_value(v).unwrap();
        assert_eq!((j.priority, j.promoted), (Priority::Normal, false));
    }

    #[test]
    fn the_priority_list() {
        let v = PriorityView {
            rules: vec![
                crate::server::priority::Rule {
                    pattern: "repo-a".into(),
                    level: Priority::High,
                },
                crate::server::priority::Rule {
                    pattern: "web-*".into(),
                    level: Priority::Low,
                },
            ],
            promote_after_s: 1800,
        };
        assert_eq!(
            render_priority(&v),
            "repo-a  high\nweb-*   low\na job queued longer than 30m00s goes as high\n"
        );
        assert_eq!(
            render_priority(&PriorityView::default()),
            "no priority rules: every job is normal unless it asks otherwise\n"
        );
    }
}
