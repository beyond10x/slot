//! Prometheus text exposition for `GET /metrics`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::types::{Job, JobState};

const WAIT_BOUNDS: &[f64] = &[0.1, 0.5, 1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0];
const RUN_BOUNDS: &[f64] = &[
    1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1200.0, 3600.0,
];

#[derive(Debug, Clone)]
struct Histogram {
    bounds: &'static [f64],
    counts: Vec<u64>,
    sum: f64,
    count: u64,
}

impl Histogram {
    fn new(bounds: &'static [f64]) -> Self {
        Self {
            bounds,
            counts: vec![0; bounds.len()],
            sum: 0.0,
            count: 0,
        }
    }

    fn observe(&mut self, v: f64) {
        for (i, b) in self.bounds.iter().enumerate() {
            if v <= *b {
                self.counts[i] += 1;
            }
        }
        self.sum += v;
        self.count += 1;
    }

    fn write(&self, out: &mut String, name: &str, toolchain: &str) {
        for (b, c) in self.bounds.iter().zip(&self.counts) {
            let _ = writeln!(
                out,
                "{name}_bucket{{toolchain=\"{toolchain}\",le=\"{b}\"}} {c}"
            );
        }
        let _ = writeln!(
            out,
            "{name}_bucket{{toolchain=\"{toolchain}\",le=\"+Inf\"}} {}",
            self.count
        );
        let _ = writeln!(out, "{name}_sum{{toolchain=\"{toolchain}\"}} {}", self.sum);
        let _ = writeln!(
            out,
            "{name}_count{{toolchain=\"{toolchain}\"}} {}",
            self.count
        );
    }
}

/// Counters over jobs finished since the daemon started.
#[derive(Debug, Default)]
pub struct Metrics {
    finished: BTreeMap<(String, &'static str), u64>,
    cpu: BTreeMap<String, f64>,
    compile_server_cpu: BTreeMap<String, f64>,
    wait: BTreeMap<String, Histogram>,
    run: BTreeMap<String, Histogram>,
}

fn result(state: JobState) -> &'static str {
    match state {
        JobState::Ok => "ok",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
        JobState::Queued | JobState::Running => "unfinished",
    }
}

/// Gauges read at scrape time.
#[derive(Debug, Default)]
pub struct Gauges {
    pub tokens: usize,
    pub free: usize,
    pub waiting: usize,
    /// Lost tokens written back since the daemon started.
    pub restored: u64,
    pub queued: BTreeMap<String, usize>,
    pub running: BTreeMap<String, usize>,
    pub job_cores: f64,
    pub sys_cpu: f64,
    pub census: crate::types::HostCensus,
}

impl Metrics {
    /// Records one finished job.
    pub fn record(&mut self, job: &Job) {
        let tc = job.toolchain.clone();
        *self
            .finished
            .entry((tc.clone(), result(job.state)))
            .or_default() += 1;
        *self.cpu.entry(tc.clone()).or_default() += job.cpu_s;
        *self.compile_server_cpu.entry(tc.clone()).or_default() += job.compile_server_cpu_s;
        let start = if job.started_ms > 0 {
            job.started_ms
        } else {
            job.ended_ms
        };
        let wait = (start - job.enqueued_ms).max(0) as f64 / 1000.0;
        self.wait
            .entry(tc.clone())
            .or_insert_with(|| Histogram::new(WAIT_BOUNDS))
            .observe(wait);
        if job.started_ms > 0 {
            let run = (job.ended_ms - job.started_ms).max(0) as f64 / 1000.0;
            self.run
                .entry(tc)
                .or_insert_with(|| Histogram::new(RUN_BOUNDS))
                .observe(run);
        }
    }

    /// The exposition text.
    pub fn render(&self, g: &Gauges) -> String {
        let mut s = String::new();
        let gauge = |s: &mut String, name: &str, help: &str, v: f64| {
            let _ = writeln!(s, "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {v}");
        };
        gauge(
            &mut s,
            "slot_tokens",
            "Build slots in the pool.",
            g.tokens as f64,
        );
        gauge(
            &mut s,
            "slot_tokens_free",
            "Build slots free now.",
            g.free as f64,
        );
        gauge(
            &mut s,
            "slot_waiters",
            "Requests waiting for slots in the jobserver pool.",
            g.waiting as f64,
        );
        let _ = writeln!(
            s,
            "# HELP slot_tokens_restored_total Lost slots written back to the pool.\n\
             # TYPE slot_tokens_restored_total counter\nslot_tokens_restored_total {}",
            g.restored
        );
        gauge(
            &mut s,
            "slot_job_cores",
            "Cores used by all running jobs over the last second.",
            g.job_cores,
        );
        gauge(
            &mut s,
            "slot_host_cpu_ratio",
            "Busy fraction of all host CPUs over the last second.",
            g.sys_cpu,
        );
        let c = &g.census;
        gauge(
            &mut s,
            "slot_host_cores",
            "Cores used by every process on the host.",
            c.host_cores,
        );
        gauge(
            &mut s,
            "slot_host_load1",
            "One-minute load average.",
            c.load1,
        );
        gauge(
            &mut s,
            "slot_unmanaged_build_cores",
            "Cores used by build processes slot does not run.",
            c.unmanaged_cores,
        );
        let _ = writeln!(
            s,
            "# HELP slot_pressure_some_avg10 Pressure stall information, some avg10, percent.\n# TYPE slot_pressure_some_avg10 gauge"
        );
        for (r, v) in [
            ("cpu", c.psi_cpu),
            ("memory", c.psi_memory),
            ("io", c.psi_io),
        ] {
            let _ = writeln!(s, "slot_pressure_some_avg10{{resource=\"{r}\"}} {v}");
        }
        let _ = writeln!(
            s,
            "# HELP slot_build_cores Cores of build processes on the host by tool family.\n# TYPE slot_build_cores gauge"
        );
        for k in &c.classes {
            let _ = writeln!(
                s,
                "slot_build_cores{{class=\"{}\",managed=\"{}\"}} {}",
                k.class, k.managed, k.cores
            );
        }
        for (name, help, map) in [
            ("slot_jobs_queued", "Jobs waiting for slots.", &g.queued),
            ("slot_jobs_running", "Jobs running.", &g.running),
        ] {
            let _ = writeln!(s, "# HELP {name} {help}\n# TYPE {name} gauge");
            for (tc, n) in map {
                let _ = writeln!(s, "{name}{{toolchain=\"{tc}\"}} {n}");
            }
        }
        let _ = writeln!(
            s,
            "# HELP slot_jobs_finished_total Jobs finished since the daemon started.\n\
             # TYPE slot_jobs_finished_total counter"
        );
        for ((tc, r), n) in &self.finished {
            let _ = writeln!(
                s,
                "slot_jobs_finished_total{{toolchain=\"{tc}\",result=\"{r}\"}} {n}"
            );
        }
        for (name, help, map) in [
            (
                "slot_job_cpu_seconds_total",
                "CPU seconds of finished jobs, compile-server work included.",
                &self.cpu,
            ),
            (
                "slot_job_compile_server_cpu_seconds_total",
                "CPU seconds of finished jobs spent in compile servers such as sccache.",
                &self.compile_server_cpu,
            ),
        ] {
            let _ = writeln!(s, "# HELP {name} {help}\n# TYPE {name} counter");
            for (tc, v) in map {
                let _ = writeln!(s, "{name}{{toolchain=\"{tc}\"}} {v}");
            }
        }
        for (name, help, map) in [
            (
                "slot_job_wait_seconds",
                "Time from enqueue until a job started (or gave up).",
                &self.wait,
            ),
            (
                "slot_job_run_seconds",
                "Time from start until a job ended.",
                &self.run,
            ),
        ] {
            let _ = writeln!(s, "# HELP {name} {help}\n# TYPE {name} histogram");
            for (tc, h) in map {
                h.write(&mut s, name, tc);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(state: JobState, enq: i64, start: i64, end: i64) -> Job {
        Job {
            id: 1,
            argv: vec![],
            cwd: String::new(),
            repo: String::new(),
            project: String::new(),
            session: String::new(),
            client: String::new(),
            toolchain: "rust".into(),
            remote: false,
            tokens: 1,
            state,
            enqueued_ms: enq,
            started_ms: start,
            ended_ms: end,
            exit: 0,
            cpu_s: 2.5,
            compile_server_cpu_s: 1.0,
            peak_rss: 0,
            wait_reason: String::new(),
            target_dir: String::new(),
        }
    }

    #[test]
    fn renders_counters_and_histograms() {
        let mut m = Metrics::default();
        m.record(&job(JobState::Ok, 0, 2_000, 12_000));
        m.record(&job(JobState::Cancelled, 0, 0, 40_000));
        let mut g = Gauges {
            tokens: 20,
            free: 19,
            ..Gauges::default()
        };
        g.running.insert("go".into(), 1);
        let s = m.render(&g);
        for want in [
            "slot_tokens 20\n",
            "slot_tokens_free 19\n",
            "slot_jobs_running{toolchain=\"go\"} 1\n",
            "slot_jobs_finished_total{toolchain=\"rust\",result=\"ok\"} 1\n",
            "slot_jobs_finished_total{toolchain=\"rust\",result=\"cancelled\"} 1\n",
            "slot_job_cpu_seconds_total{toolchain=\"rust\"} 5\n",
            "slot_job_wait_seconds_bucket{toolchain=\"rust\",le=\"5\"} 1\n",
            "slot_job_wait_seconds_bucket{toolchain=\"rust\",le=\"60\"} 2\n",
            "slot_job_wait_seconds_count{toolchain=\"rust\"} 2\n",
            "slot_job_run_seconds_bucket{toolchain=\"rust\",le=\"15\"} 1\n",
            "slot_job_run_seconds_count{toolchain=\"rust\"} 1\n",
        ] {
            assert!(s.contains(want), "missing {want:?} in\n{s}");
        }
    }
}
