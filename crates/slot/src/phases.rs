//! Where a job's time went, for its record: the sync of a remote job's tree
//! and the copy of its products back (measured by the client), the compile
//! and the tests of a cargo test job (split by the daemon where the job's
//! output shows the first test binary), and what a remote cargo job's
//! target directory on the node held when it queued.
//!
//! A job the daemon hands to a node runs a remote-mode client of this
//! binary. The client writes what it measured to the file the daemon names
//! in [`FILE_ENV`], and the daemon adds it to the job's record when the job
//! ends. A client that talked to a node itself adds its back time to the
//! node's record with an [`crate::proto::Amend`] request.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::types::Job;

/// Set by the daemon in the client of a job it hands to a node: where the
/// client writes its [`Report`].
pub const FILE_ENV: &str = "SLOT_PHASES_FILE";

/// `seeded_from` of a target directory that was absent or empty and was not
/// seeded.
pub const SEED_EMPTY: &str = "empty";

/// `seeded_from` of a target directory that held an earlier job's build.
pub const SEED_KEPT: &str = "kept";

/// cargo subcommands that run test binaries after they compile.
const TESTING: &[&str] = &["test", "t", "bench", "nextest"];

/// Whether `argv` is a cargo command that compiles and then runs test
/// binaries: `cargo test`, `cargo +nightly bench`, `cargo nextest run`,
/// `cargo-nextest nextest run`.
pub fn runs_tests(argv: &[String]) -> bool {
    let name = argv
        .first()
        .and_then(|a| Path::new(a).file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let sub = if name == "cargo" {
        crate::hook::cargo_sub(&argv[1..])
    } else {
        name.strip_prefix("cargo-")
    };
    sub.is_some_and(|s| TESTING.contains(&s))
}

/// `text` without ANSI escape sequences (`ESC [ … letter`).
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// Whether a line of a job's output says that the first test binary is
/// about to run: cargo's `Running unittests …`, `Running tests/…`,
/// `Running benches/…` and `Doc-tests …` lines, nextest's `Starting N tests
/// across …` line, or cargo's JSON `build-finished` message.
fn starts_tests_line(line: &str) -> bool {
    let l = plain(line);
    let l = l.trim_start();
    if let Some(rest) = l.strip_prefix("Running ") {
        // `cargo run` prints `Running `target/debug/x``.
        return !rest.starts_with('`');
    }
    if l.starts_with("Doc-tests ") {
        return true;
    }
    if let Some(rest) = l.strip_prefix("Starting ") {
        return rest.contains(" test") && rest.contains(" across ");
    }
    l.starts_with('{') && l.contains(r#""reason":"build-finished""#)
}

/// Whether a chunk of a job's output holds a line that starts the tests
/// ([`starts_tests_line`]).
pub fn starts_tests(chunk: &str) -> bool {
    let likely = ["Running", "Doc-tests", "Starting", "build-finished"]
        .iter()
        .any(|w| chunk.contains(w));
    likely && chunk.lines().any(starts_tests_line)
}

/// `compile_ms` and `test_ms` of a test job that ran from `started_ms` to
/// `ended_ms` and ran its first test binary at `tests_at` (Unix ms): the
/// whole run is compile time when no test binary ran.
pub fn split(started_ms: i64, tests_at: Option<i64>, ended_ms: i64) -> (Option<u64>, Option<u64>) {
    let ms = |a: i64, b: i64| u64::try_from(b - a).unwrap_or(0);
    match tests_at {
        Some(at) => {
            let at = at.clamp(started_ms, ended_ms.max(started_ms));
            (Some(ms(started_ms, at)), Some(ms(at, ended_ms)))
        }
        None => (Some(ms(started_ms, ended_ms)), None),
    }
}

/// `seeded_from` of a remote cargo job: the sibling `seeded` names, else
/// [`SEED_KEPT`] when its target directory held entries before, else
/// [`SEED_EMPTY`].
pub fn seeded_from(seeded: Option<String>, held_entries: bool) -> String {
    seeded.unwrap_or_else(|| {
        if held_entries {
            SEED_KEPT.into()
        } else {
            SEED_EMPTY.into()
        }
    })
}

/// Whether `dir` is a directory with at least one entry.
pub fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
}

/// What a remote-mode client measured, for the record of the job the daemon
/// handed to a node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    /// The record's `sync_ms`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_ms: Option<u64>,
    /// The record's `sync_bytes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_bytes: Option<u64>,
    /// The record's `extra_trees`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_trees: Vec<String>,
    /// The record's `back_ms`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub back_ms: Option<u64>,
    /// The record's `node_target_dir`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_target_dir: Option<String>,
    /// The record's `seeded_from`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seeded_from: Option<String>,
}

impl Report {
    /// Writes the report to `path`.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let line = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        std::fs::write(path, line)
    }

    /// The report at `path`; `None` when there is none or it does not
    /// parse.
    pub fn read(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// Adds every field the report has to `job`.
    pub fn apply(&self, job: &mut Job) {
        let take = |mine: &Option<u64>, theirs: &mut Option<u64>| {
            if mine.is_some() {
                *theirs = *mine;
            }
        };
        take(&self.sync_ms, &mut job.sync_ms);
        take(&self.sync_bytes, &mut job.sync_bytes);
        take(&self.back_ms, &mut job.back_ms);
        if !self.extra_trees.is_empty() {
            job.extra_trees.clone_from(&self.extra_trees);
        }
        if self.node_target_dir.is_some() {
            job.node_target_dir.clone_from(&self.node_target_dir);
        }
        if self.seeded_from.is_some() {
            job.seeded_from.clone_from(&self.seeded_from);
        }
    }
}

/// A job-log line that adds what a client measured after the job ended to
/// the record of job `amend` written before it. A reader that knows only
/// [`Job`] lines skips it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amended {
    /// The id of the job.
    pub amend: i64,
    /// The record's `back_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back_ms: Option<u64>,
}

impl Amended {
    /// Adds the line's fields to `job`.
    pub fn apply(&self, job: &mut Job) {
        if self.back_ms.is_some() {
            job.back_ms = self.back_ms;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn test_nextest_and_bench_commands_run_tests() {
        for argv in [
            &["cargo", "test", "-p", "slot"][..],
            &["/usr/bin/cargo", "+nightly", "t"],
            &["cargo", "--color", "never", "bench"],
            &["cargo", "nextest", "run"],
            &["cargo-nextest", "nextest", "run"],
        ] {
            assert!(runs_tests(&v(argv)), "{argv:?}");
        }
        for argv in [
            &["cargo", "build"][..],
            &["cargo", "clippy", "--all-targets"],
            &["cargo", "-c", "echo test"],
            &["sh", "-c", "cargo test"],
            &[],
        ] {
            assert!(!runs_tests(&v(argv)), "{argv:?}");
        }
    }

    #[test]
    fn the_first_test_binary_shows_in_cargo_nextest_and_json_output() {
        for chunk in [
            "   Compiling x v0.1.0\n    Finished `test` profile\n     Running unittests src/lib.rs (target/debug/deps/x-1)\n",
            "     Running tests/main.rs (target/debug/deps/main-2)\n",
            "     Running benches/b.rs (target/release/deps/b-3)\n",
            "   Doc-tests x\n",
            "    Starting 12 tests across 3 binaries (1 test skipped)\n",
            "    Starting 1 test across 1 binary\n",
            "\u{1b}[1m\u{1b}[32m     Running\u{1b}[0m unittests src/lib.rs (target/debug/deps/x-1)\n",
            "{\"reason\":\"compiler-artifact\"}\n{\"reason\":\"build-finished\",\"success\":true}\n",
        ] {
            assert!(starts_tests(chunk), "{chunk:?}");
        }
        for chunk in [
            "   Compiling x v0.1.0\n",
            "     Running `target/debug/x`\n",
            "    Finished `test` profile [unoptimized] target(s) in 2.0s\n",
            "Starting the server\n",
            "test parses ... ok\n",
        ] {
            assert!(!starts_tests(chunk), "{chunk:?}");
        }
    }

    #[test]
    fn compile_and_test_time_split_at_the_first_test_binary() {
        assert_eq!(split(1_000, Some(4_000), 5_500), (Some(3_000), Some(1_500)));
        assert_eq!(
            split(1_000, None, 5_500),
            (Some(4_500), None),
            "no test ran"
        );
        assert_eq!(
            split(1_000, Some(500), 5_500),
            (Some(0), Some(4_500)),
            "a mark before the start counts from the start"
        );
    }

    #[test]
    fn a_target_directory_is_seeded_kept_or_empty() {
        assert_eq!(seeded_from(Some("w0".into()), false), "w0");
        assert_eq!(seeded_from(None, true), "kept");
        assert_eq!(seeded_from(None, false), "empty");
        let t = tempfile::tempdir().unwrap();
        assert!(!has_entries(&t.path().join("absent")));
        assert!(!has_entries(t.path()));
        std::fs::create_dir(t.path().join("debug")).unwrap();
        assert!(has_entries(t.path()));
    }

    #[test]
    fn a_report_and_an_amend_line_fill_the_record() {
        let line = r#"{"id":4,"argv":["cargo","test"],"cwd":"/srv/acct/w","repo":"r",
            "toolchain":"rust","remote":false,"tokens":1,"state":"ok","enqueued_ms":1,
            "started_ms":2,"ended_ms":3,"exit":0,"cpu_s":0.0,"compile_server_cpu_s":0.0,
            "peak_rss":0,"node":"node-a"}"#;
        let mut job: Job = serde_json::from_str(line).unwrap();
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("phases.json");
        assert_eq!(Report::read(&path), None);
        let r = Report {
            sync_ms: Some(1_200),
            sync_bytes: Some(4_096),
            extra_trees: vec!["/srv/acct/specs".into()],
            back_ms: Some(300),
            node_target_dir: Some("/srv/acct/target".into()),
            seeded_from: Some(SEED_KEPT.into()),
        };
        r.write(&path).unwrap();
        let read = Report::read(&path).unwrap();
        assert_eq!(read, r);
        read.apply(&mut job);
        assert_eq!(
            (job.sync_ms, job.sync_bytes, job.back_ms),
            (Some(1_200), Some(4_096), Some(300))
        );
        assert_eq!(job.node_target_dir.as_deref(), Some("/srv/acct/target"));
        assert_eq!(job.seeded_from.as_deref(), Some("kept"));
        assert_eq!(job.extra_trees, ["/srv/acct/specs"]);

        let a: Amended = serde_json::from_str(r#"{"amend":4,"back_ms":900}"#).unwrap();
        assert_eq!(
            a,
            Amended {
                amend: 4,
                back_ms: Some(900)
            }
        );
        a.apply(&mut job);
        assert_eq!(job.back_ms, Some(900));
        assert!(
            serde_json::from_str::<Job>(r#"{"amend":4,"back_ms":900}"#).is_err(),
            "a reader of job lines skips it"
        );
    }
}
