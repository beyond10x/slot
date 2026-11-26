//! `slot status`: the pool and its jobs, read from the daemon's web API.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, bail};

pub use crate::cpu_limit::View as CpuLimitView;
pub use crate::job_parallel::View as JobParallelView;
pub use crate::run_limit::View as RunLimitView;
pub use crate::server::limits::View as LimitsView;
use crate::server::priority::PromoteAfter;
pub use crate::server::priority::View as PriorityView;
use crate::types::{Checkout, HeadroomBase, Job, JobState, Priority, State};

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

/// Sets the promotion delay (`slot priority promote-after`), or with
/// [`PromoteAfter::Default`] returns to the daemon's `--promote-after`.
pub fn promote_after(addr: &str, to: PromoteAfter) -> Result<PriorityView> {
    let body = serde_json::json!({ "promote_after_s": to }).to_string();
    let answer = request(addr, "PUT", "/api/priority", Some(&body)).map_err(|e| {
        if e.to_string().contains("missing field `pattern`") {
            e.context("the daemon is older than this slot and cannot change the delay")
        } else {
            e
        }
    })?;
    serde_json::from_str(&answer).with_context(|| format!("{addr}: /api/priority"))
}

/// The promotion delay of `v` in words: `30m00s`.
fn promote_after_text(v: &PriorityView) -> String {
    secs(
        i64::try_from(v.promote_after_s)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000),
    )
}

/// `slot priority promote-after`: one line with the delay now in force,
/// and after `default` where it comes from.
pub fn render_promote_after(v: &PriorityView, asked: PromoteAfter) -> String {
    let source = match asked {
        PromoteAfter::Default => ", the daemon's --promote-after",
        PromoteAfter::Seconds(_) => "",
    };
    if v.promote_after_s == 0 {
        return format!("no queued job goes as high for waiting{source}\n");
    }
    format!(
        "a job queued longer than {} goes as high{source}\n",
        promote_after_text(v)
    )
}

/// Reads the run-time limit (`slot run-limit`), or first sets the limit,
/// the most a job may name, or both.
pub fn run_limit(
    addr: &str,
    limit: Option<crate::run_limit::Setting>,
    max: Option<u64>,
) -> Result<RunLimitView> {
    use crate::run_limit::{Request, Setting};
    let path = "/api/run-limit";
    let answer = if limit.is_none() && max.is_none() {
        request(addr, "GET", path, None)
    } else {
        let body = serde_json::to_string(&Request {
            run_limit_s: limit.map(|l| match l {
                Setting::Off => 0,
                Setting::Seconds(s) => s,
            }),
            run_limit_max_s: max,
        })?;
        request(addr, "PUT", path, Some(&body))
    };
    let answer = answer.map_err(|e| {
        if e.to_string().contains(" 404") {
            e.context("the daemon is older than this slot and has no run-time limit")
        } else {
            e
        }
    })?;
    serde_json::from_str(&answer).with_context(|| format!("{addr}: {path}"))
}

/// `slot run-limit`: the limit in force and the most a job may name.
pub fn render_run_limit(v: &RunLimitView) -> String {
    let max = crate::run_limit::text(v.run_limit_max_s);
    match v.run_limit_s {
        Some(limit) => format!(
            "a job that runs longer than {} is ended (exit 124); `slot --timeout` names up to {max}\n",
            crate::run_limit::text(limit)
        ),
        None => format!(
            "no run-time limit; a job that names one with `slot --timeout` gets up to {max}\n"
        ),
    }
}

/// The run-time limit line of `slot status`; `None` with no limit.
fn run_limit_line(s: &State) -> Option<String> {
    let limit = crate::run_limit::text(s.run_limit_s?);
    let max = s.run_limit_max_s.unwrap_or(crate::run_limit::DEFAULT_MAX_S);
    Some(format!(
        "run-time limit {limit}: a job that runs longer is ended; `slot --timeout` names up to {}\n",
        crate::run_limit::text(max)
    ))
}

/// Reads the limit on build processes per job (`slot job-parallel`), or
/// first sets it or switches it off.
pub fn job_parallel(
    addr: &str,
    limit: Option<crate::job_parallel::Setting>,
) -> Result<JobParallelView> {
    let path = "/api/job-parallel";
    let answer = match limit {
        None => request(addr, "GET", path, None),
        Some(l) => {
            let body = serde_json::to_string(&crate::job_parallel::Request {
                job_parallel: l.value(),
            })?;
            request(addr, "PUT", path, Some(&body))
        }
    };
    let answer = answer.map_err(|e| {
        if e.to_string().contains(" 404") {
            e.context("the daemon is older than this slot and has no limit per job")
        } else {
            e
        }
    })?;
    serde_json::from_str(&answer).with_context(|| format!("{addr}: {path}"))
}

/// Reads the eviction mark and the idle keep (`slot evict-below`), or first
/// sets or forgets them.
pub fn evict_below(
    addr: &str,
    mark: Option<crate::evict::BelowSetting>,
    idle_s: Option<u64>,
) -> Result<crate::evict::BelowView> {
    let path = "/api/evict-below";
    let answer = match crate::evict::BelowRequest::of(mark, idle_s) {
        None => request(addr, "GET", path, None),
        Some(r) => {
            let body = serde_json::to_string(&r)?;
            request(addr, "PUT", path, Some(&body))
        }
    };
    let answer = answer.map_err(|e| {
        if e.to_string().contains(" 404") {
            e.context("the daemon is older than this slot and has no run-time eviction mark")
        } else {
            e
        }
    })?;
    serde_json::from_str(&answer).with_context(|| format!("{addr}: {path}"))
}

/// `slot job-parallel`: the value in force.
pub fn render_job_parallel(v: &JobParallelView) -> String {
    match v.job_parallel {
        Some(1) => "at most 1 build process per job; it takes no slot of the pool\n".into(),
        Some(n) => {
            format!("at most {n} build processes per job; they take no slots of the pool\n")
        }
        None => "no limit per job: every build process takes a slot of the pool\n".into(),
    }
}

/// The job-parallel line of `slot status`; `None` with no limit.
fn job_parallel_line(s: &State) -> Option<String> {
    let n = s.job_parallel?;
    let processes = if n == 1 { "process" } else { "processes" };
    Some(format!(
        "at most {n} build {processes} per job, outside the pool's slots\n"
    ))
}

/// Reads the CPU limit (`slot cpu-limit`), or first changes it.
pub fn cpu_limit(addr: &str, change: Option<&crate::cpu_limit::Request>) -> Result<CpuLimitView> {
    let path = "/api/cpu-limit";
    let answer = match change {
        None => request(addr, "GET", path, None),
        Some(c) => request(addr, "PUT", path, Some(&serde_json::to_string(c)?)),
    };
    let answer = answer.map_err(|e| {
        if e.to_string().contains(" 404") {
            e.context("the daemon is older than this slot and has no CPU limit")
        } else {
            e
        }
    })?;
    serde_json::from_str(&answer).with_context(|| format!("{addr}: {path}"))
}

/// What a CPU limit of `mark` CPUs with a floor of `floor` jobs and a
/// pressure mark of `pressure` percent does on a host of `cpus` CPUs (0 when
/// the daemon did not say).
fn cpu_limit_text(mark: f64, floor: usize, cpus: u32, pressure: Option<f64>) -> String {
    let run = if floor == 1 { "job runs" } else { "jobs run" };
    let over = cpus > 0 && mark > f64::from(cpus);
    match (over, pressure.map(crate::cpu_limit::percent)) {
        (true, None) => {
            format!("the CPU limit of {mark:.1} CPUs never holds a start: the host has {cpus}")
        }
        (true, Some(p)) => format!(
            "starts held while CPU pressure is at {p} % or more and {floor} {run} \
             (the CPU mark of {mark:.1} is over the host's {cpus} CPUs)"
        ),
        (false, None) => format!(
            "starts held while the pool and the compile cache use more than {mark:.1} CPUs and {floor} {run}"
        ),
        (false, Some(p)) => format!(
            "starts held while the pool and the compile cache use more than {mark:.1} CPUs \
             or CPU pressure is at {p} % or more, and {floor} {run}"
        ),
    }
}

/// `slot cpu-limit`: the values in force and the host's busy CPUs and CPU
/// pressure now.
pub fn render_cpu_limit(v: &CpuLimitView) -> String {
    use std::fmt::Write as _;
    let Some(mark) = v.cpu_busy_max else {
        return "no CPU limit: a queued job starts whatever the host's CPUs do\n".into();
    };
    let floor = v.cpu_busy_floor.unwrap_or(crate::cpu_limit::DEFAULT_FLOOR);
    let mut out = cpu_limit_text(mark, floor, v.cpus, v.cpu_pressure_max);
    let cpu_holds = !(v.cpus > 0 && mark > f64::from(v.cpus));
    if !cpu_holds && v.cpu_pressure_max.is_none() {
        out.push('\n');
        return out;
    }
    if cpu_holds {
        match (v.pool, v.compile_cache, v.outside, v.busy) {
            (Some(pool), Some(cache), outside, _) => {
                let _ = write!(
                    out,
                    "; now pool {pool:.1} + compile cache {cache:.1} of {} CPUs",
                    v.cpus
                );
                if let Some(o) = outside {
                    let _ = write!(out, ", {o:.1} outside both");
                }
            }
            // A daemon that predates the parts counts the whole host.
            (_, _, _, Some(busy)) => {
                let _ = write!(out, "; now {busy:.1} of {} CPUs busy", v.cpus);
            }
            _ => out.push_str("; no sample of the host's CPUs yet"),
        }
    }
    if v.cpu_pressure_max.is_some()
        && let Some(p) = v.pressure
    {
        let lead = if cpu_holds { ", " } else { "; now " };
        let _ = write!(out, "{lead}CPU pressure {} %", crate::cpu_limit::percent(p));
    }
    match v.held {
        0 => {}
        1 => out.push_str(", 1 job held"),
        n => {
            let _ = write!(out, ", {n} jobs held");
        }
    }
    out.push('\n');
    out
}

/// Reads how queued jobs are admitted (`slot admission`), or first
/// changes it.
pub fn admission(
    addr: &str,
    change: Option<&crate::shares::Request>,
) -> Result<crate::shares::View> {
    let path = "/api/admission";
    let answer = match change {
        None => request(addr, "GET", path, None),
        Some(c) => request(addr, "PUT", path, Some(&serde_json::to_string(c)?)),
    };
    let answer = answer.map_err(|e| {
        if e.to_string().contains(" 404") {
            e.context("the daemon is older than this slot and admits by slots only")
        } else {
            e
        }
    })?;
    serde_json::from_str(&answer).with_context(|| format!("{addr}: {path}"))
}

/// `slot admission`: how queued jobs are admitted, and by share the
/// budget, the free share and the bounds now.
pub fn render_admission(v: &crate::shares::View) -> String {
    match v.admission {
        crate::shares::Mode::Slots => format!(
            "admission by slots: a job holds its slots while it runs \
             (by share: {} to {} jobs, only short ones over {:.0} % CPU pressure)\n",
            v.floor, v.max, v.pressure_max
        ),
        crate::shares::Mode::Shares => crate::shares::line(v),
    }
}

/// The CPU limit line of `slot status`; `None` with no limit.
fn cpu_limit_line(s: &State) -> Option<String> {
    let mark = s.cpu_busy_max?;
    let floor = s.cpu_busy_floor.unwrap_or(crate::cpu_limit::DEFAULT_FLOOR);
    Some(cpu_limit_text(mark, floor, s.cores, s.cpu_pressure_max) + "\n")
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
            promote_after_text(v)
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

/// Whether job `j` runs or waits on this machine, not on a node.
fn on_this_machine(j: &Job) -> bool {
    j.node.is_empty() || j.node == crate::types::LOCAL_NODE
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
    let identity = match &j.checkout {
        Some(Checkout::Repository) => j.repo.clone(),
        Some(Checkout::Worktree { name }) => format!("{} (worktree {name})", j.repo),
        Some(Checkout::Directory) => "no repository".into(),
        None if !j.repo.is_empty() => format!("unverified · {}", j.repo),
        None => "repository unknown".into(),
    };
    // By share a job holds no slots: its share, and what it uses, stand
    // in that column.
    let held = match (j.share, j.share_used) {
        (Some(asked), Some(used)) => format!("share {asked:.1}, uses {used:.1}"),
        (Some(asked), None) => format!("share {asked:.1}"),
        (None, _) => format!("{:>2} slot(s)", j.tokens),
    };
    let id = if j.from_node {
        format!("{} on {}", j.id, j.node)
    } else {
        j.id.to_string()
    };
    let mut s = format!(
        "#{:<5} {:<7} {:<9} {} {:>7}  {}  {}",
        id,
        state,
        j.toolchain,
        held,
        secs(now - since),
        identity,
        cmd
    );
    if j.state == JobState::Queued {
        s.push_str("  ");
        s.push_str(j.priority.as_str());
        if j.promoted {
            s.push_str(" (waited)");
        }
        if j.ahead {
            s.push_str(", moved ahead");
        }
        if j.collected > 0 {
            use std::fmt::Write as _;
            let _ = write!(s, ", holds {} of {}", j.collected, j.tokens);
        }
    }
    // The daemon names a reason only while a remote node is configured.
    if on_this_machine(j) && !j.local_reason.is_empty() {
        s.push_str("\n  local: ");
        s.push_str(&j.local_reason);
    }
    if !j.wait_reason.is_empty() {
        s.push_str("\n       waiting: ");
        s.push_str(&j.wait_reason);
    }
    for w in &j.warnings {
        s.push_str("\n       warning: ");
        s.push_str(w);
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

/// `lost slots: 3 that no job and no live process holds, oldest for 12s`: the
/// tokens the daemon's last count found with no job and no live process
/// that has the pool open, and since when. The daemon writes them back when
/// two counts in a row find them. Nothing while there are none.
fn lost_line(s: &State) -> Option<String> {
    (s.tokens_unaccounted > 0).then(|| {
        format!(
            "lost slots: {} that no job and no live process holds, oldest for {}\n",
            s.tokens_unaccounted,
            secs(s.now_ms - s.tokens_unaccounted_since_ms)
        )
    })
}

/// `disk: 32.0 GB free, under 40.0 GB: deleted 2 target dir(s), 31.0 GB`, or in
/// a dry run `disk: … eviction is off (dry run), would free 31.0 GB in 2 dir(s)`;
/// nothing when eviction is off or never looked. A mark the disk hold raised
/// reads `under 50.0 GB, raised from 40.0 GB to the disk hold's mark`. A look
/// that measured nothing names no free space: `disk: no target dir known,
/// nothing measured (eviction below 40.0 GB)`.
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
    if r.unmeasured {
        let what = if r.known == 0 {
            "no target dir known".to_string()
        } else {
            format!("none of the {} known target dir(s) exists", r.known)
        };
        return Some(format!(
            "disk: {what}, nothing measured ({mode}eviction below {}){deleted}\n",
            crate::evict::mark_text(r.floor, r.raised_from)
        ));
    }
    if r.free >= r.floor && r.last.is_empty() {
        return Some(format!(
            "disk: {} free, eviction below {} ({mode}{} target dir(s) known){deleted}\n",
            crate::admission::gb(r.free),
            crate::evict::mark_text(r.floor, r.raised_from),
            r.known
        ));
    }
    if r.dry_run {
        return Some(format!(
            "{}{deleted}\n",
            crate::evict::off_summary(r.free, r.floor, r.raised_from, &r.last)
        ));
    }
    let bytes: u64 = r.last.iter().map(|e| e.bytes).sum();
    Some(format!(
        "disk: {} free, under {}: deleted {} target dir(s), {}{deleted}\n",
        crate::admission::gb(r.free),
        crate::evict::mark_text(r.floor, r.raised_from),
        r.last.len(),
        crate::admission::gb(bytes)
    ))
}

/// Where a queued job stands inside its level: the pool's `queue_class`;
/// from a daemon that does not send it, 1 for a job moved ahead and 2 for
/// any other, which keeps the order of level, then arrival.
fn served_class(j: &Job) -> u8 {
    j.queue_class.unwrap_or(if j.ahead { 1 } else { 2 })
}

/// `spill: N jobs to <node>`, one line per node that runs jobs this
/// daemon handed it: its own jobs with a node, not the rows a node reports.
fn spill_lines(s: &State) -> String {
    use std::fmt::Write as _;
    let mut by: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for j in &s.active {
        if !j.from_node && !on_this_machine(j) {
            *by.entry(j.node.as_str()).or_default() += 1;
        }
    }
    let mut out = String::new();
    for (n, c) in by {
        let _ = writeln!(out, "spill: {c} jobs to {n}");
    }
    out
}

/// The pool line, then one line per queued or running job, queued first,
/// in the order they get slots: by level, then the job's place in its
/// level, then arrival.
pub fn render(s: &State) -> String {
    let mut queued: Vec<&Job> = s
        .active
        .iter()
        .filter(|j| j.state == JobState::Queued)
        .collect();
    queued.sort_by_key(|j| (j.priority, served_class(j), j.id));
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
    let collected: u32 = queued.iter().map(|j| j.collected).sum();
    let collected = if collected > 0 {
        format!(", {collected} collected by queued jobs")
    } else {
        String::new()
    };
    let mut out = format!(
        "{pool}, {} free, {} running, {} queued{collected}\n",
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
    if let Some(l) = run_limit_line(s) {
        out.push_str(&l);
    }
    if let Some(l) = job_parallel_line(s) {
        out.push_str(&l);
    }
    if let Some(l) = cpu_limit_line(s) {
        out.push_str(&l);
    }
    if let Some(v) = &s.shares {
        out.push_str(&crate::shares::line(v));
    }
    if !s.upgrade.is_empty() {
        out.push_str(&s.upgrade);
        out.push('\n');
    }
    if let Some(l) = lost_line(s) {
        out.push_str(&l);
    }
    out.push_str(&spill_lines(s));
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
            checkout: Some(crate::types::Checkout::Repository),
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
            local_reason: String::new(),
            target_dir: String::new(),
            priority: crate::types::Priority::Normal,
            promoted: false,
            cache_env: None,
            warnings: Vec::new(),
            limit_s: None,
            timed_out: false,
            predicted_ms: None,
            collected: 0,
            ahead: false,
            queue_class: None,
            share: None,
            share_used: None,
            lent_s: 0.0,
            admission: crate::shares::Mode::Slots,
            node: String::new(),
            from_node: false,
            sync_ms: None,
            sync_bytes: None,
            back_ms: None,
            compile_ms: None,
            test_ms: None,
            node_target_dir: None,
            seeded_from: None,
            width: None,
            warm: false,
            cancel_reason: String::new(),
        }
    }

    #[test]
    fn status_counts_the_jobs_handed_to_each_node() {
        let mut s: State = serde_json::from_value(serde_json::json!({
            "host": "h", "cores": 1, "tokens": 1, "free": 1, "active": [], "done": [],
            "now_ms": 1, "toolchains": [],
        }))
        .unwrap();
        let on = |id, node: &str| {
            let mut j = job(id, JobState::Running, 1);
            j.node = node.into();
            j
        };
        // Two handed to node-1, one local, one the node runs of its own.
        s.active = vec![
            on(1, "node-1"),
            on(2, "node-1"),
            on(3, crate::types::LOCAL_NODE),
            {
                let mut j = on(7, "node-1");
                j.from_node = true;
                j
            },
        ];
        assert_eq!(spill_lines(&s), "spill: 2 jobs to node-1\n");
        assert!(render(&s).contains("spill: 2 jobs to node-1\n"));
    }

    #[test]
    fn status_preserves_the_unverified_label_of_an_old_job() {
        let mut old = job(2, JobState::Queued, 1);
        old.checkout = None;
        assert!(line(&old, 1000).contains("unverified · demo"));
        old.repo.clear();
        assert!(line(&old, 1000).contains("repository unknown"));
    }

    #[test]
    fn a_dry_run_under_the_floor_says_eviction_is_off() {
        let gb = crate::admission::GB;
        let e = |bytes| crate::evict::Eviction {
            path: "/w/a/target".into(),
            bytes,
            last_ms: 0,
            action: "would delete".into(),
        };
        let mut r = crate::evict::Report {
            floor: 40 * gb,
            dry_run: true,
            known: 35,
            free: 32 * gb,
            checked_ms: 1,
            last: vec![e(20 * gb), e(11 * gb)],
            deleted_bytes: 0,
            raised_from: 0,
            unmeasured: false,
        };
        assert_eq!(
            evict_line(&r).unwrap(),
            "disk: 32.0 GB free, under 40.0 GB: eviction is off (dry run), \
             would free 31.0 GB in 2 dir(s)\n"
        );
        r.dry_run = false;
        r.last.iter_mut().for_each(|e| e.action = "deleted".into());
        r.deleted_bytes = 31 * gb;
        assert_eq!(
            evict_line(&r).unwrap(),
            "disk: 32.0 GB free, under 40.0 GB: deleted 2 target dir(s), 31.0 GB, \
             31.0 GB deleted since start\n"
        );
    }

    #[test]
    fn the_eviction_line_names_the_mark_and_says_when_the_disk_hold_raised_it() {
        let gb = crate::admission::GB;
        let mut r = crate::evict::Report {
            floor: 50 * gb,
            raised_from: 40 * gb,
            known: 12,
            free: 60 * gb,
            checked_ms: 1,
            ..crate::evict::Report::default()
        };
        assert_eq!(
            evict_line(&r).unwrap(),
            "disk: 60.0 GB free, eviction below 50.0 GB, raised from 40.0 GB to the disk \
             hold's mark (12 target dir(s) known)\n"
        );
        r.free = 45 * gb;
        r.last = vec![crate::evict::Eviction {
            path: "/w/a/target".into(),
            bytes: 21 * gb,
            last_ms: 0,
            action: "deleted".into(),
        }];
        assert_eq!(
            evict_line(&r).unwrap(),
            "disk: 45.0 GB free, under 50.0 GB, raised from 40.0 GB to the disk hold's mark: \
             deleted 1 target dir(s), 21.0 GB\n"
        );
        r.raised_from = 0;
        r.floor = 40 * gb;
        r.free = 60 * gb;
        r.last.clear();
        assert_eq!(
            evict_line(&r).unwrap(),
            "disk: 60.0 GB free, eviction below 40.0 GB (12 target dir(s) known)\n",
            "not raised: the mark alone"
        );
    }

    /// A look that found no known target directory on disk measured
    /// nothing: the line names no free space, only the mark. The report of
    /// a daemon older than the flag reads as it did.
    #[test]
    fn the_eviction_line_names_no_free_space_when_the_look_measured_nothing() {
        let gb = crate::admission::GB;
        let report = |more: serde_json::Value| -> crate::evict::Report {
            let mut v = serde_json::json!({
                "floor": 50 * gb, "raised_from": 40 * gb, "free": 0, "checked_ms": 1,
            });
            v.as_object_mut()
                .unwrap()
                .extend(more.as_object().unwrap().clone());
            serde_json::from_value(v).unwrap()
        };
        assert_eq!(
            evict_line(&report(serde_json::json!({"unmeasured": true}))).unwrap(),
            "disk: no target dir known, nothing measured (eviction below 50.0 GB, raised from \
             40.0 GB to the disk hold's mark)\n"
        );
        assert_eq!(
            evict_line(&report(serde_json::json!({
                "unmeasured": true, "known": 12, "dry_run": true,
                "raised_from": 0, "deleted_bytes": 31 * gb,
            })))
            .unwrap(),
            "disk: none of the 12 known target dir(s) exists, nothing measured (dry run, \
             eviction below 50.0 GB), 31.0 GB deleted since start\n"
        );
        assert_eq!(
            evict_line(&report(serde_json::json!({}))).unwrap(),
            "disk: 0.0 GB free, under 50.0 GB, raised from 40.0 GB to the disk hold's mark: \
             deleted 0 target dir(s), 0.0 GB\n",
            "an older daemon's report, without the flag"
        );
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
    fn the_run_time_limit_is_a_header_line_when_one_is_set() {
        let mut s = State {
            tokens: 4,
            limit: 4,
            free: 4,
            run_limit_max_s: Some(3600),
            ..State::default()
        };
        assert_eq!(
            render(&s),
            "4 slots, 4 free, 0 running, 0 queued\n",
            "no limit: no line"
        );
        s.run_limit_s = Some(600);
        assert_eq!(
            render(&s),
            "4 slots, 4 free, 0 running, 0 queued\n\
             run-time limit 10m: a job that runs longer is ended; `slot --timeout` names up to 1h\n"
        );
        let old: State = serde_json::from_str(
            r#"{"host":"h","cores":4,"tokens":4,"free":4,"active":[],"done":[],"now_ms":0,"toolchains":[]}"#,
        )
        .unwrap();
        assert_eq!((old.run_limit_s, old.run_limit_max_s), (None, None));
        let line = serde_json::to_string(&old).unwrap();
        assert!(!line.contains("run_limit"), "{line}");
    }

    #[test]
    fn the_bound_per_job_is_a_header_line_when_it_is_on() {
        let mut s = State {
            tokens: 4,
            limit: 4,
            free: 4,
            ..State::default()
        };
        assert_eq!(
            render(&s),
            "4 slots, 4 free, 0 running, 0 queued\n",
            "off: no line"
        );
        s.job_parallel = Some(4);
        assert_eq!(
            render(&s),
            "4 slots, 4 free, 0 running, 0 queued\n\
             at most 4 build processes per job, outside the pool's slots\n"
        );
        s.job_parallel = Some(1);
        assert_eq!(
            render(&s),
            "4 slots, 4 free, 0 running, 0 queued\n\
             at most 1 build process per job, outside the pool's slots\n"
        );
        s.run_limit_s = Some(600);
        s.run_limit_max_s = Some(3600);
        assert!(
            render(&s).ends_with(
                "names up to 1h\nat most 1 build process per job, outside the pool's slots\n"
            ),
            "{}",
            render(&s)
        );
        let old: State = serde_json::from_str(
            r#"{"host":"h","cores":4,"tokens":4,"free":4,"active":[],"done":[],"now_ms":0,"toolchains":[]}"#,
        )
        .unwrap();
        assert_eq!(old.job_parallel, None);
        let line = serde_json::to_string(&old).unwrap();
        assert!(!line.contains("job_parallel"), "{line}");
        let on: State =
            serde_json::from_str(&line.replacen('{', r#"{"job_parallel":3,"#, 1)).unwrap();
        assert_eq!(on.job_parallel, Some(3));
    }

    #[test]
    fn slot_job_parallel_in_one_line() {
        use crate::job_parallel::View;
        assert_eq!(
            render_job_parallel(&View {
                job_parallel: Some(4)
            }),
            "at most 4 build processes per job; they take no slots of the pool\n"
        );
        assert_eq!(
            render_job_parallel(&View {
                job_parallel: Some(1)
            }),
            "at most 1 build process per job; it takes no slot of the pool\n"
        );
        assert_eq!(
            render_job_parallel(&View { job_parallel: None }),
            "no limit per job: every build process takes a slot of the pool\n"
        );
    }

    #[test]
    fn a_daemon_without_the_limit_per_job_says_so() {
        let addr = answer_once("HTTP/1.0 404 Not Found\r\n\r\nslot: no such API path\n");
        let e = job_parallel(&addr, None).unwrap_err();
        assert!(
            format!("{e:#}").contains("older than this slot and has no limit per job"),
            "{e:#}"
        );
    }

    #[test]
    fn slot_run_limit_in_one_line() {
        use crate::run_limit::View;
        assert_eq!(
            render_run_limit(&View {
                run_limit_s: Some(600),
                run_limit_max_s: 3600
            }),
            "a job that runs longer than 10m is ended (exit 124); `slot --timeout` names up to 1h\n"
        );
        assert_eq!(
            render_run_limit(&View {
                run_limit_s: None,
                run_limit_max_s: 5400
            }),
            "no run-time limit; a job that names one with `slot --timeout` gets up to 1h30m\n"
        );
    }

    #[test]
    fn a_daemon_without_the_run_time_limit_says_so() {
        let addr = answer_once("HTTP/1.0 404 Not Found\r\n\r\nslot: no such API path\n");
        let e = run_limit(&addr, None, None).unwrap_err();
        assert!(
            format!("{e:#}").contains("older than this slot and has no run-time limit"),
            "{e:#}"
        );
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
    fn slots_no_job_and_no_live_process_holds_are_shown_with_their_age() {
        let mut s = State {
            tokens: 6,
            free: 0,
            tokens_unaccounted: 3,
            tokens_unaccounted_since_ms: 100_000,
            now_ms: 250_000,
            ..State::default()
        };
        assert_eq!(
            render(&s),
            "6 slots, 0 free, 0 running, 0 queued\n\
             lost slots: 3 that no job and no live process holds, oldest for 2m30s\n"
        );
        s.tokens_unaccounted = 0;
        s.tokens_unaccounted_since_ms = 0;
        assert_eq!(render(&s), "6 slots, 0 free, 0 running, 0 queued\n");
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
    fn a_node_job_shows_its_nodes_id_and_the_node() {
        let mut theirs = job(7089, JobState::Running, 1);
        theirs.node = "build-1d".into();
        theirs.from_node = true;
        let mut handed = job(12, JobState::Running, 1);
        handed.node = "build-1d".into();
        let out = line(&theirs, 0);
        assert!(out.starts_with("#7089 on build-1d "), "{out}");
        assert!(!out.contains("#-"), "{out}");
        let s = State {
            tokens: 4,
            limit: 4,
            active: vec![theirs, handed],
            ..State::default()
        };
        assert_eq!(spill_lines(&s), "spill: 1 jobs to build-1d\n");
    }

    #[test]
    fn a_local_job_names_why_it_did_not_go_to_a_node() {
        let mut running = job(4, JobState::Running, 1);
        running.node = crate::types::LOCAL_NODE.into();
        running.local_reason = "sync failed: ssh: connection refused".into();
        let mut queued = job(5, JobState::Queued, 1);
        queued.local_reason = "cache rule".into();
        queued.wait_reason = "memory: 1.0 GB free".into();
        let mut on_node = job(6, JobState::Running, 1);
        on_node.node = "node-1".into();
        on_node.local_reason = "stale".into();
        let plain = job(7, JobState::Running, 1);
        let s = State {
            tokens: 4,
            limit: 4,
            active: vec![
                running.clone(),
                queued.clone(),
                on_node.clone(),
                plain.clone(),
            ],
            now_ms: 151_000,
            ..State::default()
        };
        let out = render(&s);
        let lines: Vec<&str> = out
            .lines()
            .filter(|l| !l.starts_with("spill:"))
            .skip(1)
            .collect();
        let first = |j: &Job| line(j, 151_000).lines().next().unwrap().to_string();
        assert_eq!(
            lines,
            [
                first(&queued).as_str(),
                "  local: cache rule",
                "       waiting: memory: 1.0 GB free",
                first(&running).as_str(),
                "  local: sync failed: ssh: connection refused",
                first(&on_node).as_str(),
                first(&plain).as_str(),
            ],
            "{out}"
        );
        // A job record without the field reads as no reason.
        let mut v = serde_json::to_value(&plain).unwrap();
        v.as_object_mut().unwrap().remove("local_reason");
        let j: Job = serde_json::from_value(v).unwrap();
        assert_eq!(j.local_reason, "");
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
    fn a_job_with_warnings_lists_them_under_its_line() {
        let plain_running = job(7, JobState::Running, 1);
        let mut running = plain_running.clone();
        running.warnings = vec![
            "compile cache off: RUSTC_WRAPPER is empty".into(),
            "private sccache server (port 4303)".into(),
        ];
        let mut plain_queued = job(8, JobState::Queued, 1);
        plain_queued.wait_reason = "memory: 1.0 GB free".into();
        let mut queued = plain_queued.clone();
        queued.warnings = vec!["private sccache directory".into()];
        let s = State {
            tokens: 4,
            limit: 4,
            active: vec![running.clone(), queued.clone()],
            now_ms: 151_000,
            ..State::default()
        };
        let out = render(&s);
        let lines: Vec<&str> = out.lines().skip(1).collect();
        let first = |j: &Job| line(j, 151_000).lines().next().unwrap().to_string();
        assert_eq!(
            lines,
            [
                first(&plain_queued).as_str(),
                "       waiting: memory: 1.0 GB free",
                "       warning: private sccache directory",
                first(&plain_running).as_str(),
                "       warning: compile cache off: RUSTC_WRAPPER is empty",
                "       warning: private sccache server (port 4303)",
            ],
            "{out}"
        );
        assert_eq!(
            line(&plain_running, 151_000).lines().count(),
            1,
            "no warning, no extra line"
        );
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

    #[test]
    fn the_promotion_delay_in_one_line() {
        let view = |promote_after_s| PriorityView {
            rules: Vec::new(),
            promote_after_s,
        };
        assert_eq!(
            render_promote_after(&view(300), PromoteAfter::Seconds(300)),
            "a job queued longer than 5m00s goes as high\n"
        );
        assert_eq!(
            render_promote_after(&view(0), PromoteAfter::Seconds(0)),
            "no queued job goes as high for waiting\n"
        );
        assert_eq!(
            render_promote_after(&view(1800), PromoteAfter::Default),
            "a job queued longer than 30m00s goes as high, the daemon's --promote-after\n"
        );
        assert_eq!(
            render_promote_after(&view(0), PromoteAfter::Default),
            "no queued job goes as high for waiting, the daemon's --promote-after\n"
        );
        let far = view(u64::MAX);
        assert!(
            render_priority(&far).ends_with("goes as high\n"),
            "no overflow"
        );
        assert_eq!(
            render_promote_after(&far, PromoteAfter::Seconds(u64::MAX))
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn by_share_the_header_names_the_budget_and_each_job_its_share_and_what_it_uses() {
        use crate::shares::{Mode, View};
        let plain = State {
            cores: 3,
            tokens: 2,
            limit: 2,
            active: vec![job(5, JobState::Running, 2)],
            ..State::default()
        };
        let out = render(&plain);
        assert!(!out.contains("shares:"), "{out}");
        assert!(out.contains("#5     running rust       2 slot(s)"), "{out}");
        let mut run = job(3, JobState::Running, 1);
        run.share = Some(1.0);
        run.share_used = Some(0.02);
        let mut queued = job(4, JobState::Queued, 1);
        queued.share = Some(2.3);
        queued.wait_reason = "waits for a share of 2.3 CPUs: 0.2 of a budget of 2.2 free".into();
        let s = State {
            shares: Some(View {
                admission: Mode::Shares,
                floor: 1,
                max: 48,
                pressure_max: 60.0,
                cpus: 3,
                budget: 2.26,
                cap: 3.0,
                min_budget: 1.5,
                free: 0.26,
                busy: 0.0,
                starting: 2.0,
                running: 2,
                idle: 0,
                lent: 0.0,
                pressure: 5.0,
                braking: false,
            }),
            active: vec![run, queued],
            ..plain
        };
        let out = render(&s);
        assert!(
            out.contains(
                "shares: budget 2.3 of 3.0 CPUs, 0.3 free (0.0 busy, 2.0 for jobs that just \
                 started), 2 running (1 to 48)\n"
            ),
            "{out}"
        );
        assert!(
            out.contains("#3     running rust      share 1.0, uses 0.0"),
            "{out}"
        );
        assert!(out.contains("#4     queued  rust      share 2.3"), "{out}");
        assert!(
            out.contains("waiting: waits for a share of 2.3 CPUs: 0.2 of a budget of 2.2 free"),
            "{out}"
        );
        assert_eq!(
            render_admission(&View {
                admission: Mode::Slots,
                floor: 4,
                max: 48,
                pressure_max: 60.0,
                ..View::default()
            }),
            "admission by slots: a job holds its slots while it runs \
             (by share: 4 to 48 jobs, only short ones over 60 % CPU pressure)\n"
        );
    }

    #[test]
    fn the_cpu_limit_is_a_header_line_when_it_is_on_and_a_held_job_says_why() {
        let pool = "4 slots, 4 free, 0 running, 0 queued\n";
        let mut s = State {
            cores: 20,
            tokens: 4,
            limit: 4,
            free: 4,
            ..State::default()
        };
        assert_eq!(render(&s), pool, "off: no line");
        s.cpu_busy_max = Some(17.0);
        s.cpu_busy_floor = Some(4);
        assert_eq!(
            render(&s),
            format!(
                "{pool}starts held while the pool and the compile cache use more than 17.0 CPUs and 4 jobs run\n"
            )
        );
        s.cpu_busy_floor = Some(1);
        assert_eq!(
            render(&s),
            format!(
                "{pool}starts held while the pool and the compile cache use more than 17.0 CPUs and 1 job runs\n"
            )
        );
        s.cpu_busy_max = Some(25.0);
        assert_eq!(
            render(&s),
            format!("{pool}the CPU limit of 25.0 CPUs never holds a start: the host has 20\n")
        );
        s.cpu_busy_max = Some(17.5);
        s.job_parallel = Some(4);
        assert!(
            render(&s).ends_with(
                "outside the pool's slots\n\
                 starts held while the pool and the compile cache use more than 17.5 CPUs and 1 job runs\n"
            ),
            "{}",
            render(&s)
        );
        let reason = "host busy: 18.4 of 20 CPUs, mark 17.0";
        let mut held = job(8, JobState::Queued, 1);
        held.wait_reason = reason.into();
        s.active = vec![held];
        let out = render(&s);
        let under = out.lines().skip_while(|l| !l.starts_with("#8")).nth(1);
        assert_eq!(
            under.map(str::trim_start),
            Some(format!("waiting: {reason}").as_str()),
            "{out}"
        );

        let old: State = serde_json::from_str(
            r#"{"host":"h","cores":4,"tokens":4,"free":4,"active":[],"done":[],"now_ms":0,"toolchains":[]}"#,
        )
        .unwrap();
        assert_eq!((old.cpu_busy_max, old.cpu_busy_floor), (None, None));
        let line = serde_json::to_string(&old).unwrap();
        assert!(!line.contains("cpu_busy"), "{line}");
        let on: State = serde_json::from_str(&line.replacen(
            '{',
            r#"{"cpu_busy_max":17.5,"cpu_busy_floor":4,"#,
            1,
        ))
        .unwrap();
        assert_eq!((on.cpu_busy_max, on.cpu_busy_floor), (Some(17.5), Some(4)));
    }

    #[test]
    fn slot_cpu_limit_in_one_line() {
        use crate::cpu_limit::View;
        let mut v = View {
            cpu_busy_max: Some(17.0),
            cpu_busy_floor: Some(4),
            cpus: 20,
            busy: Some(18.44),
            held: 3,
            ..View::default()
        };
        assert_eq!(
            render_cpu_limit(&v),
            "starts held while the pool and the compile cache use more than 17.0 CPUs and 4 jobs run; \
             now 18.4 of 20 CPUs busy, 3 jobs held\n"
        );
        v.held = 1;
        v.cpu_busy_floor = Some(1);
        assert_eq!(
            render_cpu_limit(&v),
            "starts held while the pool and the compile cache use more than 17.0 CPUs and 1 job runs; \
             now 18.4 of 20 CPUs busy, 1 job held\n"
        );
        v.held = 0;
        v.busy = None;
        assert_eq!(
            render_cpu_limit(&v),
            "starts held while the pool and the compile cache use more than 17.0 CPUs and 1 job runs; \
             no sample of the host's CPUs yet\n"
        );
        v.cpu_busy_max = Some(25.0);
        v.busy = Some(3.0);
        assert_eq!(
            render_cpu_limit(&v),
            "the CPU limit of 25.0 CPUs never holds a start: the host has 20\n"
        );
        assert_eq!(
            render_cpu_limit(&View {
                cpus: 20,
                busy: Some(3.0),
                ..View::default()
            }),
            "no CPU limit: a queued job starts whatever the host's CPUs do\n"
        );
    }

    #[test]
    fn slot_cpu_limit_prints_the_parts_and_the_cpus_outside_both() {
        use crate::cpu_limit::{Limits, Parts};
        let limits = Limits {
            cpu_busy_max: Some(17.0),
            cpu_busy_floor: Some(6),
            ..Limits::default()
        };
        let parts = Parts {
            pool: 12.0,
            compile_cache: 6.0,
            outside: 1.5,
        };
        assert_eq!(
            render_cpu_limit(&limits.view(20, Some(parts), 1, Some(30.0))),
            "starts held while the pool and the compile cache use more than 17.0 CPUs and 6 jobs run; \
             now pool 12.0 + compile cache 6.0 of 20 CPUs, 1.5 outside both, 1 job held\n",
            "without a pressure mark the line is as before"
        );
        assert_eq!(
            render_cpu_limit(&limits.view(20, None, 0, None)),
            "starts held while the pool and the compile cache use more than 17.0 CPUs and 6 jobs run; \
             no sample of the host's CPUs yet\n"
        );
    }

    #[test]
    fn slot_cpu_limit_prints_the_pressure_mark_and_the_pressure_now() {
        use crate::cpu_limit::{Limits, Parts};
        let limits = Limits {
            cpu_busy_max: Some(17.0),
            cpu_busy_floor: Some(6),
            cpu_pressure_max: Some(80.0),
        };
        let parts = Parts {
            pool: 12.0,
            compile_cache: 6.0,
            outside: 1.5,
        };
        assert_eq!(
            render_cpu_limit(&limits.view(20, Some(parts), 2, Some(85.24))),
            "starts held while the pool and the compile cache use more than 17.0 CPUs \
             or CPU pressure is at 80 % or more, and 6 jobs run; \
             now pool 12.0 + compile cache 6.0 of 20 CPUs, 1.5 outside both, CPU pressure 85.2 %, \
             2 jobs held\n"
        );
        let over = Limits {
            cpu_busy_max: Some(25.0),
            cpu_busy_floor: Some(1),
            cpu_pressure_max: Some(62.5),
        };
        assert_eq!(
            render_cpu_limit(&over.view(20, Some(parts), 0, Some(40.0))),
            "starts held while CPU pressure is at 62.5 % or more and 1 job runs \
             (the CPU mark of 25.0 is over the host's 20 CPUs); now CPU pressure 40 %\n"
        );
        assert_eq!(
            render_cpu_limit(&over.view(20, None, 0, None)),
            "starts held while CPU pressure is at 62.5 % or more and 1 job runs \
             (the CPU mark of 25.0 is over the host's 20 CPUs)\n",
            "a daemon that does not say the pressure"
        );
        let mut s = State {
            cores: 20,
            tokens: 4,
            limit: 4,
            free: 4,
            cpu_busy_max: Some(17.0),
            cpu_busy_floor: Some(4),
            cpu_pressure_max: Some(80.0),
            ..State::default()
        };
        assert!(
            render(&s).ends_with(
                "starts held while the pool and the compile cache use more than 17.0 CPUs \
                 or CPU pressure is at 80 % or more, and 4 jobs run\n"
            ),
            "{}",
            render(&s)
        );
        s.cpu_pressure_max = None;
        let line = serde_json::to_string(&s).unwrap();
        assert!(!line.contains("cpu_pressure_max"), "{line}");
    }

    #[test]
    fn a_daemon_without_the_cpu_limit_says_so() {
        let addr = answer_once("HTTP/1.0 404 Not Found\r\n\r\nslot: no such API path\n");
        let e = cpu_limit(&addr, None).unwrap_err();
        assert!(
            format!("{e:#}").contains("older than this slot and has no CPU limit"),
            "{e:#}"
        );
    }

    /// A queued job names the slots it collected, and one moved ahead of
    /// an earlier job says so; the pool line counts the collected slots.
    #[test]
    fn a_queued_job_names_the_slots_it_collected_and_whether_it_was_moved_ahead() {
        let mut big = job(30036, JobState::Queued, 17);
        big.collected = 11;
        let mut short = job(30040, JobState::Queued, 1);
        short.ahead = true;
        short.predicted_ms = Some(800);
        let s = State {
            tokens: 20,
            limit: 17,
            headroom_percent: 15,
            active: vec![job(7, JobState::Running, 6), big, short],
            now_ms: 151_000,
            ..State::default()
        };
        let out = render(&s);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[0],
            "17 of 20 slots (15% headroom), 0 free, 1 running, 2 queued, 11 collected by queued jobs",
            "{out}"
        );
        assert!(
            lines[2].starts_with("#30036 queued")
                && lines[2].ends_with("cargo test  normal, holds 11 of 17"),
            "{out}"
        );
        assert!(
            lines[1].starts_with("#30040 queued")
                && lines[1].ends_with("cargo test  normal, moved ahead"),
            "{out}"
        );
        let mut none = s.clone();
        none.active.retain(|j| j.state == JobState::Running);
        assert_eq!(
            render(&none).lines().next(),
            Some("17 of 20 slots (15% headroom), 0 free, 1 running, 0 queued")
        );
    }

    // adversary u5

    /// `render` lists queued jobs "in the order they get slots". A short
    /// job moved ahead of an earlier job of its level gets the next slot
    /// first, so it is listed first; it is listed second, under the job it
    /// passes.
    #[test]
    fn adversary_u5_a_job_moved_ahead_is_listed_before_the_job_it_passes() {
        let long = job(4, JobState::Queued, 1);
        let mut short = job(9, JobState::Queued, 1);
        short.ahead = true;
        short.predicted_ms = Some(800);
        let s = State {
            tokens: 1,
            limit: 1,
            active: vec![job(2, JobState::Running, 1), long, short],
            now_ms: 151_000,
            ..State::default()
        };
        let out = render(&s);
        let lines: Vec<&str> = out.lines().skip(1).collect();
        assert!(lines[0].starts_with("#9 "), "{out}");
    }

    /// The class the daemon sends wins over `ahead`: a job that waited the
    /// bound (class 0) is listed before a short one that passed it earlier.
    #[test]
    fn a_job_that_waited_the_bound_is_listed_before_a_short_one() {
        let mut fixed = job(4, JobState::Queued, 1);
        fixed.queue_class = Some(0);
        let mut short = job(9, JobState::Queued, 1);
        short.ahead = true;
        short.queue_class = Some(1);
        let mut plain = job(6, JobState::Queued, 1);
        plain.queue_class = Some(2);
        let s = State {
            tokens: 1,
            limit: 1,
            active: vec![plain, short, fixed],
            now_ms: 151_000,
            ..State::default()
        };
        let out = render(&s);
        let ids: Vec<&str> = out
            .lines()
            .skip(1)
            .map(|l| l.split_whitespace().next().unwrap_or(""))
            .collect();
        assert_eq!(ids, ["#4", "#9", "#6"], "{out}");
    }
}
