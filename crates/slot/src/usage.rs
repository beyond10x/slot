//! Usage of the pool over a window: slot time, CPU time and queue wait per
//! project, repository or session, from the job log.
//!
//! Slot time is run time × slots held, clipped to the window: the share of
//! the pool a group took. CPU time is the job's measured CPU seconds; it can
//! be far below slot time for jobs that hold slots while they wait on
//! something else. Both are shown because they answer different questions.

use std::collections::HashMap;
use std::io::BufRead;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::types::Job;

/// What rows are grouped by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum By {
    /// The job's project (`crate::project`); its repository name when unknown.
    #[default]
    Project,
    /// The job's repository name.
    Repo,
    /// The agent session that submitted the job.
    Session,
}

impl std::str::FromStr for By {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "project" => Ok(Self::Project),
            "repo" => Ok(Self::Repo),
            "session" => Ok(Self::Session),
            _ => Err(format!("unknown grouping {s:?}: project, repo or session")),
        }
    }
}

/// One group's use of the pool in the window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct UsageRow {
    /// Project, repository or session id; empty for jobs with none.
    #[serde(default)]
    pub key: String,
    /// Session title, for rows grouped by session.
    #[serde(default)]
    pub title: String,
    /// Jobs that started in the window.
    #[serde(default)]
    pub jobs: u32,
    /// Run seconds × slots held, inside the window.
    #[serde(default)]
    pub slot_s: f64,
    /// CPU seconds of jobs that started in the window.
    #[serde(default)]
    pub cpu_s: f64,
    /// Queue wait seconds of jobs that started in the window.
    #[serde(default)]
    pub wait_s: f64,
    /// Median queue wait, seconds.
    #[serde(default)]
    pub wait_p50_s: f64,
    /// Longest queue wait, seconds.
    #[serde(default)]
    pub wait_max_s: f64,
    /// `slot_s` over the total, 0 to 1.
    #[serde(default)]
    pub slot_share: f64,
    /// `cpu_s` over the total, 0 to 1.
    #[serde(default)]
    pub cpu_share: f64,
}

/// Usage of the pool in `[from_ms, to_ms)`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct Usage {
    /// Window start, Unix milliseconds.
    #[serde(default)]
    #[ts(type = "number")]
    pub from_ms: i64,
    /// Window end, Unix milliseconds.
    #[serde(default)]
    #[ts(type = "number")]
    pub to_ms: i64,
    /// What the rows are grouped by.
    #[serde(default)]
    pub by: By,
    /// All jobs together (`key` empty).
    #[serde(default)]
    pub total: UsageRow,
    /// One row per group, most slot time first.
    #[serde(default)]
    pub rows: Vec<UsageRow>,
}

/// The group key of `job`. `project` resolves the project of a job logged
/// before jobs carried one (from its directory, which may be gone).
pub fn key_of(job: &Job, by: By, project: &mut dyn FnMut(&str) -> String) -> String {
    match by {
        By::Repo => job.repo.clone(),
        By::Session => job.session.clone(),
        By::Project if !job.project.is_empty() => job.project.clone(),
        By::Project => Some(project(&job.cwd))
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| job.repo.clone()),
    }
}

#[derive(Default)]
struct Acc {
    row: UsageRow,
    waits: Vec<f64>,
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    }
}

/// Aggregates the job log `log` over `[from_ms, to_ms)`. Lines that do not
/// parse and jobs that never started are skipped; a running job (no end) is
/// counted up to `to_ms`.
pub fn aggregate(
    log: impl BufRead,
    from_ms: i64,
    to_ms: i64,
    by: By,
    project: &mut dyn FnMut(&str) -> String,
) -> Usage {
    let mut groups: HashMap<String, Acc> = HashMap::new();
    let mut total = Acc::default();
    for line in log.lines() {
        let Ok(line) = line else { break };
        let Ok(job) = serde_json::from_str::<Job>(&line) else {
            continue;
        };
        if job.started_ms <= 0 {
            continue;
        }
        let end = if job.ended_ms > 0 {
            job.ended_ms
        } else {
            to_ms
        };
        let run_ms = (end.min(to_ms) - job.started_ms.max(from_ms)).max(0);
        let started_in = job.started_ms >= from_ms && job.started_ms < to_ms;
        if run_ms == 0 && !started_in {
            continue;
        }
        let slot_s = run_ms as f64 / 1000.0 * f64::from(job.tokens.max(1));
        let key = key_of(&job, by, project);
        for acc in [groups.entry(key.clone()).or_default(), &mut total] {
            acc.row.slot_s += slot_s;
            if started_in {
                let wait = (job.started_ms - job.enqueued_ms).max(0) as f64 / 1000.0;
                acc.row.jobs += 1;
                acc.row.cpu_s += job.cpu_s;
                acc.row.wait_s += wait;
                acc.row.wait_max_s = acc.row.wait_max_s.max(wait);
                acc.waits.push(wait);
            }
        }
    }
    let finish = |key: String, mut a: Acc, slot: f64, cpu: f64| {
        a.row.key = key;
        a.row.wait_p50_s = median(&mut a.waits);
        a.row.slot_share = if slot > 0.0 { a.row.slot_s / slot } else { 0.0 };
        a.row.cpu_share = if cpu > 0.0 { a.row.cpu_s / cpu } else { 0.0 };
        a.row
    };
    let (slot, cpu) = (total.row.slot_s, total.row.cpu_s);
    let mut rows: Vec<UsageRow> = groups
        .into_iter()
        .map(|(k, a)| finish(k, a, slot, cpu))
        .collect();
    rows.sort_by(|a, b| {
        b.slot_s
            .total_cmp(&a.slot_s)
            .then_with(|| a.key.cmp(&b.key))
    });
    Usage {
        from_ms,
        to_ms,
        by,
        total: finish(String::new(), total, slot, cpu),
        rows,
    }
}

/// Default window: the last 24 hours.
pub const DEFAULT_WINDOW_MS: i64 = 24 * 3600 * 1000;

/// The window and grouping of `GET /api/usage?from=&to=&by=` at `now_ms`:
/// `to` defaults to now, `from` to 24 hours before `to`, `by` to project.
pub fn query<S: std::hash::BuildHasher>(
    params: &HashMap<String, String, S>,
    now_ms: i64,
) -> Result<(i64, i64, By), String> {
    let num = |k: &str| -> Result<Option<i64>, String> {
        params
            .get(k)
            .map(|v| {
                v.parse::<i64>()
                    .map_err(|_| format!("{k}: not a number of milliseconds: {v:?}"))
            })
            .transpose()
    };
    let to = num("to")?.unwrap_or(now_ms);
    let from = num("from")?.unwrap_or(to - DEFAULT_WINDOW_MS);
    if from >= to {
        return Err(format!("from {from} is not before to {to}"));
    }
    let by = params.get("by").map_or(Ok(By::Project), |b| b.parse())?;
    Ok((from, to, by))
}

/// Milliseconds in a `--since` value: a number followed by `m`, `h` or `d`.
pub fn parse_since(s: &str) -> Result<i64, String> {
    let bad = || format!("--since {s:?}: a number followed by m, h or d, e.g. 24h");
    let (n, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = n.parse().map_err(|_| bad())?;
    let per = match unit {
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(bad()),
    };
    if n <= 0 {
        return Err(bad());
    }
    Ok(n * per)
}

/// `slot usage --json`.
pub fn json(u: &Usage) -> String {
    serde_json::to_string_pretty(u).unwrap_or_default()
}

/// `slot usage` as text: one line per row, at most `top` rows.
pub fn render(u: &Usage, top: usize) -> String {
    use std::fmt::Write as _;
    let by = match u.by {
        By::Project => "project",
        By::Repo => "repo",
        By::Session => "session",
    };
    let t = &u.total;
    let mut out = format!(
        "{} jobs, {:.1} slot-h, {:.1} cpu-h, by {by}\n",
        t.jobs,
        t.slot_s / 3600.0,
        t.cpu_s / 3600.0
    );
    let _ = writeln!(
        out,
        "{by:<28} {:>5} {:>7} {:>6} {:>6} {:>6} {:>8}",
        "jobs", "slot-h", "share", "cpu-h", "share", "wait p50"
    );
    for r in u.rows.iter().take(top) {
        let mut key = if r.title.is_empty() {
            r.key.clone()
        } else {
            format!("{} ({})", r.title, r.key)
        };
        if key.is_empty() {
            key = "(none)".into();
        }
        if key.chars().count() > 28 {
            key = key.chars().take(27).chain(std::iter::once('…')).collect();
        }
        let _ = writeln!(
            out,
            "{key:<28} {:>5} {:>7.1} {:>5.1}% {:>6.1} {:>5.1}% {:>7.0}s",
            r.jobs,
            r.slot_s / 3600.0,
            r.slot_share * 100.0,
            r.cpu_s / 3600.0,
            r.cpu_share * 100.0,
            r.wait_p50_s
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JobState;

    fn job(
        id: i64,
        repo: &str,
        project: &str,
        session: &str,
        tokens: u32,
        t: (i64, i64, i64),
        cpu: f64,
    ) -> String {
        let j = Job {
            id,
            argv: vec!["make".into()],
            cwd: format!("/src/{repo}"),
            repo: repo.into(),
            project: project.into(),
            session: session.into(),
            client: String::new(),
            toolchain: "other".into(),
            remote: false,
            tokens,
            state: JobState::Ok,
            enqueued_ms: t.0,
            started_ms: t.1,
            ended_ms: t.2,
            exit: 0,
            cpu_s: cpu,
            compile_server_cpu_s: 0.0,
            peak_rss: 0,
            wait_reason: String::new(),
            target_dir: String::new(),
        };
        serde_json::to_string(&j).unwrap()
    }

    fn none(_: &str) -> String {
        String::new()
    }

    #[test]
    fn slot_time_is_run_times_slots_clipped_to_the_window() {
        let log = [
            job(1, "api", "team-a", "s1", 1, (0, 1_000, 11_000), 8.0),
            job(2, "web", "team-a", "s2", 4, (0, 2_000, 4_000), 6.0),
            job(3, "cli", "team-b", "s1", 2, (5_000, 15_000, 30_000), 9.0),
            "not json".to_string(),
            job(4, "cli", "team-b", "s1", 1, (0, 0, 0), 0.0),
        ]
        .join("\n");
        let u = aggregate(log.as_bytes(), 0, 20_000, By::Project, &mut none);
        assert_eq!(u.rows.len(), 2);
        let a = &u.rows[0];
        assert_eq!((a.key.as_str(), a.jobs), ("team-a", 2));
        assert!((a.slot_s - (10.0 + 8.0)).abs() < 1e-9, "{a:?}");
        assert!((a.cpu_s - 14.0).abs() < 1e-9);
        let b = &u.rows[1];
        assert_eq!((b.key.as_str(), b.jobs), ("team-b", 1));
        assert!(
            (b.slot_s - 10.0).abs() < 1e-9,
            "clipped at the window end: {b:?}"
        );
        assert!((u.total.slot_s - 28.0).abs() < 1e-9);
        let share: f64 = u.rows.iter().map(|r| r.slot_share).sum();
        assert!((share - 1.0).abs() < 1e-9);
        let cpu: f64 = u.rows.iter().map(|r| r.cpu_share).sum();
        assert!((cpu - 1.0).abs() < 1e-9);
    }

    #[test]
    fn waits_count_for_jobs_that_started_in_the_window() {
        let log = [
            job(1, "api", "p", "s", 1, (0, 1_000, 2_000), 0.0),
            job(2, "api", "p", "s", 1, (0, 3_000, 4_000), 0.0),
            job(3, "api", "p", "s", 1, (0, 9_000, 10_000), 0.0),
            job(4, "api", "p", "s", 1, (0, 50_000, 51_000), 0.0),
        ]
        .join("\n");
        let u = aggregate(log.as_bytes(), 0, 20_000, By::Repo, &mut none);
        let r = &u.rows[0];
        assert_eq!(r.jobs, 3);
        assert!((r.wait_s - 13.0).abs() < 1e-9);
        assert!((r.wait_p50_s - 3.0).abs() < 1e-9);
        assert!((r.wait_max_s - 9.0).abs() < 1e-9);
    }

    #[test]
    fn a_running_job_counts_up_to_the_window_end() {
        let log = job(1, "api", "p", "s", 2, (0, 10_000, 0), 0.0);
        let u = aggregate(log.as_bytes(), 0, 20_000, By::Repo, &mut none);
        assert!((u.rows[0].slot_s - 20.0).abs() < 1e-9);
    }

    #[test]
    fn keys_fall_back_from_project_to_directory_to_repository() {
        let line = job(1, "api", "", "s1", 1, (0, 1, 2), 0.0);
        let j: Job = serde_json::from_str(&line).unwrap();
        let seen = std::cell::RefCell::new(Vec::new());
        let mut from_dir = |cwd: &str| {
            seen.borrow_mut().push(cwd.to_string());
            "team-c".to_string()
        };
        assert_eq!(key_of(&j, By::Project, &mut from_dir), "team-c");
        assert_eq!(*seen.borrow(), ["/src/api"]);
        assert_eq!(key_of(&j, By::Project, &mut none), "api");
        assert_eq!(key_of(&j, By::Repo, &mut none), "api");
        assert_eq!(key_of(&j, By::Session, &mut none), "s1");
        let logged: Job =
            serde_json::from_str(&job(2, "api", "team-a", "", 1, (0, 1, 2), 0.0)).unwrap();
        assert_eq!(key_of(&logged, By::Project, &mut from_dir), "team-a");
        assert_eq!(seen.borrow().len(), 1, "a logged project needs no lookup");
    }

    #[test]
    fn grouping_parses_and_an_older_log_line_has_no_project() {
        assert_eq!("session".parse::<By>(), Ok(By::Session));
        assert!("team".parse::<By>().is_err());
        let line = job(1, "api", "", "s", 1, (0, 1, 2), 0.0);
        assert!(!line.contains("\"project\""), "{line}");
    }
    #[test]
    fn the_query_defaults_to_the_last_day_by_project() {
        let q = |pairs: &[(&str, &str)]| {
            let m: HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            query(&m, 100_000_000)
        };
        assert_eq!(
            q(&[]),
            Ok((100_000_000 - DEFAULT_WINDOW_MS, 100_000_000, By::Project))
        );
        assert_eq!(
            q(&[("from", "5"), ("to", "9"), ("by", "session")]),
            Ok((5, 9, By::Session))
        );
        assert_eq!(
            q(&[("to", "90000000")]),
            Ok((90_000_000 - DEFAULT_WINDOW_MS, 90_000_000, By::Project))
        );
        assert!(q(&[("from", "x")]).is_err());
        assert!(q(&[("from", "9"), ("to", "5")]).is_err());
        assert!(q(&[("by", "team")]).is_err());
    }

    #[test]
    fn since_takes_minutes_hours_and_days() {
        assert_eq!(parse_since("90m"), Ok(90 * 60_000));
        assert_eq!(parse_since("24h"), Ok(24 * 3_600_000));
        assert_eq!(parse_since("7d"), Ok(7 * 86_400_000));
        assert!(parse_since("7").is_err());
        assert!(parse_since("0h").is_err());
        assert!(parse_since("xh").is_err());
    }

    #[test]
    fn the_text_lists_shares_and_waits_per_row() {
        let log = [
            job(1, "api", "team-a", "s1", 1, (0, 4_000, 3_604_000), 1800.0),
            job(2, "cli", "team-b", "s2", 1, (0, 1_000, 1_801_000), 900.0),
        ]
        .join("\n");
        let u = aggregate(log.as_bytes(), 0, 7_200_000, By::Project, &mut none);
        let text = render(&u, 10);
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].starts_with("2 jobs, 1.5 slot-h, 0.8 cpu-h, by project"),
            "{text}"
        );
        assert!(
            lines[1].contains("project") && lines[1].contains("slot-h"),
            "{text}"
        );
        assert!(
            lines[2].starts_with("team-a") && lines[2].contains("66.7%") && lines[2].contains("4s"),
            "{text}"
        );
        assert!(
            lines[3].starts_with("team-b") && lines[3].contains("33.3%"),
            "{text}"
        );
        assert_eq!(render(&u, 1).lines().count(), 3, "top limits the rows");
    }
}
