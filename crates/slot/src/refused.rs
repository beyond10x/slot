//! The compiler calls the sccache servers refused to cache, per repository
//! or project, with what a repository can do about each reason
//! (`slot cache refused`).
//!
//! The counters in `cache.jsonl` are per server, not per job, so each
//! minute's calls ([`cache_minutes`]) are given to the jobs that ran in it.
//! The jobs of a minute are those whose run overlaps the minute before its
//! reading and whose compile server worked for them
//! (`compile_server_cpu_s > 0`). When they all belong to one group, the
//! minute is that group's exactly. Otherwise its calls are split across
//! the groups in proportion to each job's compile-server CPU rate
//! (`compile_server_cpu_s` over its run time) times its time in the minute,
//! and counted as shared. A minute with calls and no such job is counted
//! apart.

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sccache::{CacheMinute, Counters, cache_minutes};
use crate::types::Job;
use crate::usage::By;

/// The span of one reading: the minute before it.
pub const MINUTE_MS: i64 = 60_000;

/// What a repository can do about a reason sccache gives for not caching a
/// call. One line per reason; [`OTHER_REMEDY`] for any other reason.
pub const REMEDIES: &[(&str, &str)] = &[
    (
        "incremental",
        "set CARGO_INCREMENTAL=0 for jobs that do not edit and rebuild, as a CI runner does",
    ),
    (
        "crate-type",
        "bin, proc-macro, dylib and cdylib crates are not cached by sccache; \
         no remedy in the repository",
    ),
    (
        "multiple input files",
        "a compiler call with more than one input file, often a build script or a wrapper; \
         name the call",
    ),
    (
        "missing input",
        "the input file does not exist where sccache looks, as with generated sources",
    ),
];

/// The remedy of a reason that is not in [`REMEDIES`].
pub const OTHER_REMEDY: &str = "named as sccache prints it; no remedy known";

/// The remedy of `reason`: its line in [`REMEDIES`], else [`OTHER_REMEDY`].
pub fn remedy(reason: &str) -> &'static str {
    REMEDIES
        .iter()
        .find(|(r, _)| *r == reason)
        .map_or(OTHER_REMEDY, |(_, text)| text)
}

/// Compiler calls: hits, misses and the refused ones by reason. Shared
/// counts are fractions of a minute's calls, so every count is a number.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Calls {
    /// Calls answered from the cache.
    pub hits: f64,
    /// Calls compiled after a miss.
    pub misses: f64,
    /// Calls the server refused to cache: the sum of `reasons`.
    pub not_cached: f64,
    /// The refused calls by the server's reason.
    pub reasons: BTreeMap<String, f64>,
}

impl Calls {
    /// Adds `part` (0 to 1) of the calls of `m`.
    fn add(&mut self, m: &CacheMinute, part: f64) {
        self.hits += m.hits as f64 * part;
        self.misses += m.misses as f64 * part;
        self.not_cached += m.not_cached as f64 * part;
        for (reason, n) in &m.reasons {
            *self.reasons.entry(reason.clone()).or_default() += *n as f64 * part;
        }
    }
}

/// One group's calls in the window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RefusedRow {
    /// Repository or project; empty for jobs with none.
    pub key: String,
    /// Calls of minutes in which only this group's jobs ran.
    pub exact: Calls,
    /// This group's part of minutes in which jobs of several groups ran.
    pub shared: Calls,
    /// Minutes that were this group's alone.
    pub exact_minutes: u32,
    /// Minutes this group shared with others.
    pub shared_minutes: u32,
}

/// A reason of the window and what a repository can do about it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Remedy {
    /// The reason as sccache names it.
    pub reason: String,
    /// Calls refused for it in the window, every group and no job together.
    pub refused: f64,
    /// [`remedy`] of the reason.
    pub remedy: String,
}

/// The refused calls of a window, per group.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Refused {
    /// Window start, Unix milliseconds.
    pub from_ms: i64,
    /// Window end, Unix milliseconds.
    pub to_ms: i64,
    /// What the rows are grouped by.
    pub by: By,
    /// Minutes in the window with at least one call.
    pub minutes: u32,
    /// One row per group, most refused calls first.
    pub rows: Vec<RefusedRow>,
    /// Calls of minutes in which no job with compile-server CPU ran.
    pub unexplained: Calls,
    /// Those minutes.
    pub unexplained_minutes: u32,
    /// Every reason in the window with its remedy, most refused first.
    pub remedies: Vec<Remedy>,
}

/// The end of `job`'s run: its end, or `to_ms` while it runs.
fn end_of(job: &Job, to_ms: i64) -> i64 {
    if job.ended_ms > 0 {
        job.ended_ms
    } else {
        to_ms
    }
}

/// The weight of each group in the minute that ends at `t`: per job with
/// compile-server CPU that ran in it, its compile-server CPU rate times
/// its seconds in the minute.
fn weights(jobs: &[Job], t: i64, to_ms: i64, by: By) -> BTreeMap<String, f64> {
    let (lo, hi) = (t - MINUTE_MS, t);
    let mut out: BTreeMap<String, f64> = BTreeMap::new();
    for job in jobs {
        if job.started_ms <= 0 || job.compile_server_cpu_s <= 0.0 {
            continue;
        }
        let end = end_of(job, to_ms);
        let overlap_ms = end.min(hi) - job.started_ms.max(lo);
        if overlap_ms <= 0 {
            continue;
        }
        let run_s = (end - job.started_ms).max(1) as f64 / 1000.0;
        let rate = job.compile_server_cpu_s / run_s;
        let key = crate::usage::key_of(job, by, &mut |_| String::new());
        *out.entry(key).or_default() += rate * overlap_ms as f64 / 1000.0;
    }
    out
}

/// The calls of `records` whose reading lies in `(from_ms, to_ms]`, given
/// to the groups of `jobs` as the module says. `records` may hold readings
/// before the window: they are what the first minute in it is counted from.
/// A job with no end counts as running up to `to_ms`. A job logged without
/// a project is grouped by its repository under `By::Project`.
pub fn attribute(records: &[Counters], jobs: &[Job], from_ms: i64, to_ms: i64, by: By) -> Refused {
    let mut sorted = records.to_vec();
    sorted.sort_by_key(|c| c.t);
    let mut out = Refused {
        from_ms,
        to_ms,
        by,
        ..Refused::default()
    };
    let mut rows: BTreeMap<String, RefusedRow> = BTreeMap::new();
    for m in cache_minutes(&sorted) {
        if m.t <= from_ms || m.t > to_ms || m.hits + m.misses + m.not_cached == 0 {
            continue;
        }
        out.minutes += 1;
        let w = weights(jobs, m.t, to_ms, by);
        let total: f64 = w.values().sum();
        if w.is_empty() {
            out.unexplained.add(&m, 1.0);
            out.unexplained_minutes += 1;
            continue;
        }
        let alone = w.len() == 1;
        for (key, weight) in w {
            let row = rows.entry(key.clone()).or_insert_with(|| RefusedRow {
                key,
                ..RefusedRow::default()
            });
            if alone {
                row.exact.add(&m, 1.0);
                row.exact_minutes += 1;
            } else {
                row.shared.add(&m, weight / total);
                row.shared_minutes += 1;
            }
        }
    }
    let mut by_reason: BTreeMap<String, f64> = BTreeMap::new();
    for calls in rows
        .values()
        .flat_map(|r| [&r.exact, &r.shared])
        .chain([&out.unexplained])
    {
        for (reason, n) in &calls.reasons {
            *by_reason.entry(reason.clone()).or_default() += n;
        }
    }
    out.remedies = by_reason
        .into_iter()
        .map(|(reason, refused)| Remedy {
            remedy: remedy(&reason).to_string(),
            reason,
            refused,
        })
        .collect();
    out.remedies.sort_by(|a, b| {
        b.refused
            .total_cmp(&a.refused)
            .then_with(|| a.reason.cmp(&b.reason))
    });
    out.rows = rows.into_values().collect();
    let refused = |r: &RefusedRow| r.exact.not_cached + r.shared.not_cached;
    out.rows.sort_by(|a, b| {
        refused(b)
            .total_cmp(&refused(a))
            .then_with(|| a.key.cmp(&b.key))
    });
    out
}

/// Reads what [`attribute`] needs for `(from_ms, to_ms]` from the state
/// directory: the readings in the window with the last one of each port
/// before it (`cache.jsonl`), and the jobs with compile-server CPU whose run
/// overlaps the window or the minute before it (`jobs.jsonl`). Lines that
/// do not parse are skipped; a missing file reads as empty.
pub fn read(state_dir: &Path, from_ms: i64, to_ms: i64) -> (Vec<Counters>, Vec<Job>) {
    let lines = |name: &str| {
        std::fs::File::open(state_dir.join(name))
            .ok()
            .map(|f| std::io::BufReader::new(f).lines().map_while(Result::ok))
            .into_iter()
            .flatten()
    };
    let mut before: HashMap<u16, Counters> = HashMap::new();
    let mut records = Vec::new();
    for line in lines("cache.jsonl") {
        let Ok(c) = serde_json::from_str::<Counters>(&line) else {
            continue;
        };
        if c.t <= 0 || c.port == 0 || c.t > to_ms {
            continue;
        }
        if c.t > from_ms {
            records.push(c);
        } else if before.get(&c.port).is_none_or(|b| b.t <= c.t) {
            before.insert(c.port, c);
        }
    }
    records.extend(before.into_values());
    records.sort_by_key(|c| c.t);
    let jobs = lines("jobs.jsonl")
        .filter_map(|line| serde_json::from_str::<Job>(&line).ok())
        .filter(|j| {
            j.started_ms > 0
                && j.compile_server_cpu_s > 0.0
                && j.started_ms < to_ms
                && end_of(j, to_ms) > from_ms - MINUTE_MS
        })
        .collect();
    (records, jobs)
}

/// `slot cache refused --json`.
pub fn json(r: &Refused) -> String {
    serde_json::to_string_pretty(r).unwrap_or_default()
}

/// A count as text: shared counts are fractions, shown whole.
fn n(v: f64) -> String {
    format!("{v:.0}")
}

/// `1 minute`, `2 minutes`.
fn minutes(n: u32) -> String {
    format!("{n} minute{}", if n == 1 { "" } else { "s" })
}

/// A window length in days (from two), hours or minutes.
fn span(ms: i64) -> String {
    match ms {
        d if d % 86_400_000 == 0 && d > 86_400_000 => format!("{}d", d / 86_400_000),
        h if h % 3_600_000 == 0 => format!("{}h", h / 3_600_000),
        m => format!("{}m", m / 60_000),
    }
}

/// `slot cache refused` as text: per group its hits, misses and refused
/// calls by reason, exact and shared; the minutes no job explains; the
/// remedy of every reason.
pub fn render(r: &Refused) -> String {
    use std::fmt::Write as _;
    let by = match r.by {
        By::Project => "project",
        By::Repo => "repo",
        By::Session => "session",
    };
    let mut out = format!(
        "compiler calls in the last {}, by {by}: {} with calls, {} no job explains\n",
        span(r.to_ms - r.from_ms),
        minutes(r.minutes),
        minutes(r.unexplained_minutes),
    );
    if r.minutes == 0 {
        return out;
    }
    let _ = writeln!(
        out,
        "{:<32} {:>9} {:>9}",
        format!("{by} / calls"),
        "exact",
        "shared"
    );
    let line = |out: &mut String, label: String, a: f64, b: f64| {
        let _ = writeln!(out, "{label:<32} {:>9} {:>9}", n(a), n(b));
    };
    for row in &r.rows {
        let mut key = if row.key.is_empty() {
            "(no repository)".to_string()
        } else {
            row.key.clone()
        };
        if key.chars().count() > 32 {
            key = key.chars().take(31).chain(std::iter::once('…')).collect();
        }
        let _ = writeln!(
            out,
            "{key}  ({} alone, {} shared)",
            minutes(row.exact_minutes),
            minutes(row.shared_minutes)
        );
        line(&mut out, "  hits".into(), row.exact.hits, row.shared.hits);
        line(
            &mut out,
            "  misses".into(),
            row.exact.misses,
            row.shared.misses,
        );
        line(
            &mut out,
            "  refused".into(),
            row.exact.not_cached,
            row.shared.not_cached,
        );
        let mut reasons: Vec<(&String, f64, f64)> = row
            .exact
            .reasons
            .keys()
            .chain(row.shared.reasons.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|k| {
                let get = |c: &Calls| c.reasons.get(k).copied().unwrap_or(0.0);
                (k, get(&row.exact), get(&row.shared))
            })
            .collect();
        reasons.sort_by(|a, b| (b.1 + b.2).total_cmp(&(a.1 + a.2)).then(a.0.cmp(b.0)));
        for (reason, exact, shared) in reasons {
            if exact.round() == 0.0 && shared.round() == 0.0 {
                continue;
            }
            line(&mut out, format!("    {reason}"), exact, shared);
        }
    }
    let u = &r.unexplained;
    let detail: Vec<String> = u
        .reasons
        .iter()
        .map(|(reason, c)| format!("{reason} {}", n(*c)))
        .collect();
    let _ = writeln!(
        out,
        "no job: {}, {} hits, {} misses, {} refused{}",
        minutes(r.unexplained_minutes),
        n(u.hits),
        n(u.misses),
        n(u.not_cached),
        if detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", detail.join(", "))
        }
    );
    if !r.remedies.is_empty() {
        out.push_str("remedies:\n");
        for m in &r.remedies {
            let _ = writeln!(out, "  {}: {}", m.reason, m.remedy);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JobState;

    fn record(t: i64, hits: u64, misses: u64, reasons: &[(&str, u64)]) -> Counters {
        Counters {
            t,
            port: 4226,
            rust_hits: hits,
            rust_misses: misses,
            not_cached: reasons
                .iter()
                .map(|(r, n)| ((*r).to_string(), *n))
                .collect(),
            ..Counters::default()
        }
    }

    fn job(id: i64, repo: &str, project: &str, run: (i64, i64), server_cpu: f64) -> Job {
        let line = serde_json::json!({
            "id": id,
            "argv": ["cargo", "build"],
            "cwd": format!("/src/{repo}"),
            "repo": repo,
            "checkout": {"kind": "repository"},
            "project": project,
            "toolchain": "rust",
            "remote": false,
            "tokens": 1,
            "exit": 0,
            "cpu_s": server_cpu,
            "peak_rss": 0,
            "state": "ok",
            "enqueued_ms": run.0,
            "started_ms": run.0,
            "ended_ms": run.1,
            "compile_server_cpu_s": server_cpu,
        });
        let j: Job = serde_json::from_value(line).unwrap();
        assert_eq!(j.state, JobState::Ok);
        j
    }

    fn calls(hits: f64, misses: f64, reasons: &[(&str, f64)]) -> Calls {
        Calls {
            hits,
            misses,
            not_cached: reasons.iter().map(|(_, n)| n).sum(),
            reasons: reasons
                .iter()
                .map(|(r, n)| ((*r).to_string(), *n))
                .collect(),
        }
    }

    /// Four readings of one port (the first one is where counting starts):
    /// the minute to 120 s has jobs of `api` only, the minute to 180 s jobs
    /// of `api` and `web`, the minute to 240 s only a job whose compile
    /// server did nothing.
    fn fixture() -> (Vec<Counters>, Vec<Job>) {
        let records = vec![
            record(60_000, 0, 0, &[]),
            record(120_000, 10, 4, &[("incremental", 6), ("crate-type", 2)]),
            record(
                180_000,
                30,
                4,
                &[
                    ("incremental", 6),
                    ("crate-type", 10),
                    ("multiple input files", 4),
                ],
            ),
            record(
                240_000,
                33,
                4,
                &[
                    ("incremental", 6),
                    ("crate-type", 10),
                    ("multiple input files", 4),
                    ("-", 5),
                ],
            ),
        ];
        let jobs = vec![
            // In the first minute only.
            job(1, "api", "team-a", (70_000, 110_000), 4.0),
            // 50 s at 0.2 CPU: weight 10 in the second minute.
            job(2, "api", "team-a", (125_000, 175_000), 10.0),
            // 20 s at 1.5 CPU: weight 30 in the second minute.
            job(3, "web", "team-a", (150_000, 170_000), 30.0),
            // In the third minute, but its compile server did nothing.
            job(4, "web", "team-a", (185_000, 235_000), 0.0),
        ];
        (records, jobs)
    }

    fn assert_calls(got: &Calls, want: &Calls, what: &str) {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(
            close(got.hits, want.hits)
                && close(got.misses, want.misses)
                && close(got.not_cached, want.not_cached),
            "{what}: {got:?} != {want:?}"
        );
        let keys: Vec<&String> = got.reasons.keys().collect();
        let want_keys: Vec<&String> = want.reasons.keys().collect();
        assert_eq!(keys, want_keys, "{what}");
        for (k, v) in &want.reasons {
            assert!(close(got.reasons[k], *v), "{what} {k}: {got:?} != {want:?}");
        }
    }

    #[test]
    fn a_minute_of_one_repository_is_exact_and_a_minute_of_two_is_shared() {
        let (records, jobs) = fixture();
        let r = attribute(&records, &jobs, 0, 300_000, By::Repo);
        assert_eq!(r.minutes, 3, "{r:?}");
        let keys: Vec<&str> = r.rows.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(keys, ["api", "web"], "most refused first: {r:?}");
        let api = &r.rows[0];
        assert_eq!((api.exact_minutes, api.shared_minutes), (1, 1));
        assert_calls(
            &api.exact,
            &calls(10.0, 4.0, &[("incremental", 6.0), ("crate-type", 2.0)]),
            "api exact",
        );
        // A quarter of the shared minute: weight 10 of 40.
        assert_calls(
            &api.shared,
            &calls(
                5.0,
                0.0,
                &[("crate-type", 2.0), ("multiple input files", 1.0)],
            ),
            "api shared",
        );
        let web = &r.rows[1];
        assert_eq!((web.exact_minutes, web.shared_minutes), (0, 1));
        assert_calls(&web.exact, &Calls::default(), "web exact");
        assert_calls(
            &web.shared,
            &calls(
                15.0,
                0.0,
                &[("crate-type", 6.0), ("multiple input files", 3.0)],
            ),
            "web shared",
        );
        assert_eq!(r.unexplained_minutes, 1);
        assert_calls(&r.unexplained, &calls(3.0, 0.0, &[("-", 5.0)]), "no job");
        let reasons: Vec<(&str, f64)> = r
            .remedies
            .iter()
            .map(|m| (m.reason.as_str(), m.refused))
            .collect();
        assert_eq!(
            reasons,
            [
                ("crate-type", 10.0),
                ("incremental", 6.0),
                ("-", 5.0),
                ("multiple input files", 4.0)
            ],
            "every reason of the window, most refused first"
        );
        for m in &r.remedies {
            assert_eq!(m.remedy, remedy(&m.reason));
        }
    }

    #[test]
    fn jobs_of_one_project_in_one_minute_make_it_exact() {
        let (records, jobs) = fixture();
        let r = attribute(&records, &jobs, 0, 300_000, By::Project);
        assert_eq!(r.rows.len(), 1, "{r:?}");
        let row = &r.rows[0];
        assert_eq!(row.key, "team-a");
        assert_eq!((row.exact_minutes, row.shared_minutes), (2, 0));
        assert_calls(
            &row.exact,
            &calls(
                30.0,
                4.0,
                &[
                    ("incremental", 6.0),
                    ("crate-type", 10.0),
                    ("multiple input files", 4.0),
                ],
            ),
            "team-a",
        );
        assert_calls(&row.shared, &Calls::default(), "team-a shared");
    }

    #[test]
    fn the_window_counts_its_minutes_from_the_reading_before_it() {
        let (records, jobs) = fixture();
        let r = attribute(&records, &jobs, 120_000, 180_000, By::Repo);
        assert_eq!(r.minutes, 1, "only the reading at 180 s: {r:?}");
        assert_eq!(r.unexplained_minutes, 0);
        let api = r.rows.iter().find(|row| row.key == "api").unwrap();
        assert_eq!((api.exact_minutes, api.shared_minutes), (0, 1));
        assert_calls(&api.exact, &Calls::default(), "api exact");
        // A job with no end runs to the window's end.
        let mut running = jobs.clone();
        running[2].ended_ms = 0;
        let r = attribute(&records, &running, 120_000, 200_000, By::Repo);
        assert_eq!(r.minutes, 1, "{r:?}");
        let web = r.rows.iter().find(|row| row.key == "web").unwrap();
        // Run from 150 s to 200 s: 50 s at 0.6 CPU, 30 s of them in the
        // minute, weight 18; against 50 s at 0.2, weight 10.
        assert!((web.shared.hits - 20.0 * 18.0 / 28.0).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn every_reason_has_a_remedy_line() {
        assert!(remedy("incremental").contains("CARGO_INCREMENTAL=0"));
        assert!(remedy("incremental").contains("CI runner"));
        for kind in ["bin", "proc-macro", "dylib", "cdylib"] {
            assert!(remedy("crate-type").contains(kind), "{kind}");
        }
        assert!(remedy("crate-type").contains("no remedy in the repository"));
        assert!(remedy("multiple input files").contains("more than one input"));
        assert!(remedy("missing input").contains("does not exist"));
        assert_eq!(remedy("-"), OTHER_REMEDY);
        assert_eq!(remedy("missing output_dir"), OTHER_REMEDY);
        assert!(OTHER_REMEDY.contains("no remedy known"));
        let named: Vec<&str> = REMEDIES.iter().map(|(r, _)| *r).collect();
        assert_eq!(
            named,
            [
                "incremental",
                "crate-type",
                "multiple input files",
                "missing input"
            ]
        );
        for (reason, text) in REMEDIES {
            assert_eq!(remedy(reason), *text);
            assert_ne!(*text, "");
        }
    }

    #[test]
    fn the_text_has_exact_and_shared_columns_the_unexplained_line_and_remedies() {
        let (records, jobs) = fixture();
        let text = render(&attribute(&records, &jobs, 0, 300_000, By::Repo));
        let lines: Vec<&str> = text.lines().collect();
        let header = lines.iter().position(|l| l.contains("exact")).unwrap_or(0);
        assert!(lines[header].contains("shared"), "{text}");
        let line = |start: &str| {
            lines
                .iter()
                .find(|l| l.trim_start().starts_with(start))
                .copied()
                .unwrap_or_else(|| panic!("no line {start:?}: {text}"))
        };
        let numbers = |l: &str| -> Vec<String> {
            l.split_whitespace()
                .filter(|w| w.parse::<f64>().is_ok())
                .map(str::to_string)
                .collect()
        };
        assert!(line("api").contains("api"), "{text}");
        assert_eq!(numbers(line("incremental")), ["6", "0"], "{text}");
        assert!(text.contains("no job"), "{text}");
        let unexplained = line("no job");
        assert!(
            unexplained.contains("1 minute")
                && unexplained.contains("3 hits")
                && unexplained.contains("5 refused"),
            "{text}"
        );
        let remedies = lines
            .iter()
            .position(|l| l.starts_with("remedies"))
            .unwrap_or_else(|| panic!("{text}"));
        for reason in ["crate-type", "incremental", "multiple input files", "-"] {
            let want = format!("{reason}: {}", remedy(reason));
            assert!(
                lines[remedies..].iter().any(|l| l.trim() == want),
                "{want:?}: {text}"
            );
        }
    }

    /// A job outside a repository is named as one, a reason whose calls
    /// round to none in both columns is left out, and a window of hours is
    /// named in hours.
    #[test]
    fn the_text_names_jobs_without_a_repository_and_leaves_out_empty_reasons() {
        let (records, mut jobs) = fixture();
        jobs[0].repo = String::new();
        let mut r = attribute(&records, &jobs, 0, 86_400_000, By::Repo);
        r.rows[0].shared.reasons.insert("-E".into(), 0.2);
        let text = render(&r);
        assert!(text.starts_with("compiler calls in the last 24h"), "{text}");
        assert!(
            text.lines().any(|l| l.starts_with("(no repository)")),
            "{text}"
        );
        assert!(
            !text.lines().any(|l| l.trim_start().starts_with("-E")),
            "{text}"
        );
    }

    #[test]
    fn the_state_directory_gives_the_window_and_the_reading_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let (records, jobs) = fixture();
        let mut cache: Vec<String> = records
            .iter()
            .map(|c| serde_json::to_string(c).unwrap())
            .collect();
        cache.push("not json".into());
        std::fs::write(dir.path().join("cache.jsonl"), cache.join("\n")).unwrap();
        let mut log: Vec<String> = jobs
            .iter()
            .map(|j| serde_json::to_string(j).unwrap())
            .collect();
        log.push(serde_json::to_string(&job(5, "old", "", (0, 10_000), 9.0)).unwrap());
        std::fs::write(dir.path().join("jobs.jsonl"), log.join("\n")).unwrap();
        let (got, got_jobs) = read(dir.path(), 120_000, 180_000);
        let ts: Vec<i64> = got.iter().map(|c| c.t).collect();
        assert_eq!(
            ts,
            [120_000, 180_000],
            "the reading before the window and the one in it"
        );
        let ids: Vec<i64> = got_jobs.iter().map(|j| j.id).collect();
        assert_eq!(
            ids,
            [1, 2, 3],
            "jobs with compile-server CPU from a minute before the window"
        );
        assert_eq!(
            read(&dir.path().join("none"), 0, 1),
            (Vec::new(), Vec::new())
        );
    }
}
