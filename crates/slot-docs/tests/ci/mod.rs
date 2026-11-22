//! The CI workflows and the local gate: one rustc, named in `rust-toolchain.toml`,
//! and a pull request of planning or prose files that skips the code's build and tests.

use std::path::{Path, PathBuf};
use std::process::Command;

mod attacks;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// The workflows that install a rustc.
const RUST_WORKFLOWS: [&str; 3] = [
    ".github/workflows/gate.yml",
    ".github/workflows/release.yml",
    ".github/workflows/pages.yml",
];

/// The `toolchain:` input every rustc install takes: the output of the step that reads
/// `rust-toolchain.toml`.
const TOOLCHAIN_FROM_FILE: &str = "toolchain: ${{ steps.rustc.outputs.channel }}";

/// The step that decides which checks a change needs, in `gate.yml` and `pages.yml`.
const SCOPE_RUN: &str = r#"run: sh .github/scripts/pr-scope.sh "$EVENT" >> "$GITHUB_OUTPUT""#;

/// The channel `rust-toolchain.toml` pins.
fn pinned_channel() -> String {
    let text = read("rust-toolchain.toml");
    let channel = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("channel = "))
        .expect("rust-toolchain.toml has a `channel = ` line");
    channel.trim_matches('"').to_string()
}

/// A step of a workflow: its lines, from `- ` to the next step or job.
struct Step {
    job: String,
    lines: Vec<String>,
}

impl Step {
    fn has(&self, line: &str) -> bool {
        self.lines
            .iter()
            .any(|l| l.trim().trim_start_matches("- ") == line)
    }

    fn value(&self, key: &str) -> Option<&str> {
        let prefix = format!("{key}: ");
        self.lines.iter().find_map(|l| {
            l.trim()
                .trim_start_matches("- ")
                .strip_prefix(prefix.as_str())
        })
    }
}

/// The steps of every job, in order. Jobs sit at two spaces under `jobs:`, steps start
/// with `      - ` (the layout every workflow of this repository uses).
fn steps(rel: &str) -> Vec<Step> {
    let text = read(rel);
    let mut out: Vec<Step> = Vec::new();
    let mut in_jobs = false;
    let mut job = String::new();
    let mut in_steps = false;
    for line in text.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if let Some(name) = line.strip_prefix("  ").and_then(|l| l.strip_suffix(':'))
            && !name.starts_with(' ')
        {
            job = name.to_string();
            in_steps = false;
            continue;
        }
        if line == "    steps:" {
            in_steps = true;
            continue;
        }
        if !in_steps {
            continue;
        }
        if line.starts_with("      - ") {
            out.push(Step {
                job: job.clone(),
                lines: vec![line.to_string()],
            });
        } else if line.starts_with("        ") || line.trim().is_empty() {
            if let Some(step) = out.last_mut() {
                step.lines.push(line.to_string());
            }
        } else {
            in_steps = false;
        }
    }
    out
}

#[test]
fn rust_toolchain_pins_one_exact_rustc_with_rustfmt_and_clippy() {
    let text = read("rust-toolchain.toml");
    assert!(
        text.lines().any(|l| l.trim() == "[toolchain]"),
        "rust-toolchain.toml has a [toolchain] table"
    );
    let channel = pinned_channel();
    let parts: Vec<&str> = channel.split('.').collect();
    assert!(
        parts.len() == 3 && parts[0] == "1" && parts.iter().all(|p| p.parse::<u32>().is_ok()),
        "channel must be one exact version 1.x.y, not `{channel}`"
    );
    let components = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("components = "))
        .expect("rust-toolchain.toml has a `components = ` line");
    for c in ["\"rustfmt\"", "\"clippy\""] {
        assert!(
            components.contains(c),
            "components must name {c}: {components}"
        );
    }
}

#[test]
fn workflows_install_rustc_from_rust_toolchain_toml_and_name_no_version() {
    for rel in RUST_WORKFLOWS {
        let steps = steps(rel);
        let installs: Vec<&Step> = steps
            .iter()
            .filter(|s| {
                s.value("uses")
                    .is_some_and(|u| u.starts_with("dtolnay/rust-toolchain@"))
            })
            .collect();
        assert!(!installs.is_empty(), "{rel}: installs no rustc");
        for install in &installs {
            let uses = install.value("uses").unwrap();
            let reference = uses.trim_start_matches("dtolnay/rust-toolchain@");
            let sha = reference.split_whitespace().next().unwrap_or_default();
            assert!(
                sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                "{rel}: dtolnay/rust-toolchain is pinned to a commit, not `{reference}`"
            );
            assert!(
                !["stable", "beta", "nightly", "1."]
                    .iter()
                    .any(|w| reference[sha.len()..].contains(w)),
                "{rel}: the action reference names a rustc: `{reference}`"
            );
            assert!(
                install.has(TOOLCHAIN_FROM_FILE),
                "{rel}, job {}: the rustc install takes `{TOOLCHAIN_FROM_FILE}`",
                install.job
            );
            let reader = steps
                .iter()
                .take_while(|s| !std::ptr::eq(*s, *install))
                .find(|s| s.job == install.job && s.has("id: rustc"));
            assert!(
                reader.is_some(),
                "{rel}, job {}: a step `id: rustc` reads rust-toolchain.toml before the install",
                install.job
            );
        }
        let text = read(rel);
        for line in text.lines() {
            let l = line.trim().trim_start_matches("- ");
            if l.starts_with('#') {
                continue;
            }
            assert!(
                !l.starts_with("toolchain:") || l == TOOLCHAIN_FROM_FILE,
                "{rel}: names a rustc of its own: `{l}`"
            );
            for own in [
                "rustup default",
                "rustup override",
                "rustup toolchain install",
                "RUSTUP_TOOLCHAIN",
                "cargo +",
                "rustc +",
            ] {
                assert!(!l.contains(own), "{rel}: names a rustc of its own: `{l}`");
            }
        }
    }
}

#[test]
fn the_rustc_step_of_every_workflow_reads_the_pinned_channel() {
    let want = format!("channel={}", pinned_channel());
    for rel in RUST_WORKFLOWS {
        for step in steps(rel).iter().filter(|s| s.has("id: rustc")) {
            let run = step
                .value("run")
                .unwrap_or_else(|| panic!("{rel}: the rustc step has a one-line `run:`"));
            let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
                "rustc-step-{}-{}",
                rel.replace('/', "_"),
                step.job
            ));
            let _ = std::fs::remove_file(&out);
            let status = Command::new("sh")
                .arg("-ec")
                .arg(run)
                .current_dir(root())
                .env("GITHUB_OUTPUT", &out)
                .status()
                .expect("run sh");
            assert!(status.success(), "{rel}: `{run}` exits {status}");
            let written = std::fs::read_to_string(&out).expect("the step wrote GITHUB_OUTPUT");
            assert_eq!(
                written.trim(),
                want,
                "{rel}, job {}: the rustc step writes the pinned channel",
                step.job
            );
        }
    }
}

#[test]
fn the_gate_prints_rustc_before_it_builds() {
    let text = read("Taskfile.yml");
    let check = text
        .split("\n  check:\n")
        .nth(1)
        .expect("Taskfile.yml has a check task");
    let first = check
        .lines()
        .skip_while(|l| l.trim() != "cmds:")
        .nth(1)
        .expect("the check task has commands");
    assert_eq!(
        first.trim(),
        "- rustc --version",
        "the first command of `task check` prints the rustc it builds with"
    );
    for rel in [
        ".github/workflows/gate.yml",
        ".github/workflows/release.yml",
    ] {
        assert!(
            steps(rel)
                .iter()
                .any(|s| s.value("run") == Some("task check")),
            "{rel}: runs `task check`, which prints the rustc"
        );
    }
}

#[test]
fn the_path_decision_answers_both_sides_of_its_list() {
    let out = Command::new("sh")
        .arg(root().join(".github/scripts/pr-scope.test.sh"))
        .env("PR_SCOPE_TEST_DIR", env!("CARGO_TARGET_TMPDIR"))
        .output()
        .expect("run sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "pr-scope.test.sh failed:\n{stdout}\n{stderr}"
    );
    assert!(stdout.contains(", 0 failed"), "{stdout}");
}

/// The required checks keep their names and decide inside the job, on pull requests only.
#[test]
fn required_checks_keep_their_names_and_skip_from_inside() {
    let gate = read(".github/workflows/gate.yml");
    let pages = read(".github/workflows/pages.yml");
    for (rel, text) in [("gate.yml", &gate), ("pages.yml", &pages)] {
        assert!(
            !text.contains("paths-ignore") && !text.contains("paths:"),
            "{rel}: the skip happens inside the job, not with a path filter"
        );
    }
    assert!(
        gate.contains("\njobs:\n  gate:\n"),
        "gate.yml: the job `gate` keeps its id"
    );
    assert!(
        !gate.contains("\n    name:"),
        "gate.yml: the job `gate` takes no other name"
    );
    assert!(
        pages.contains("\n    name: Build documentation\n"),
        "pages.yml: the job keeps the name `Build documentation`"
    );

    for rel in [".github/workflows/gate.yml", ".github/workflows/pages.yml"] {
        let steps = steps(rel);
        let checkout = &steps[0];
        assert!(
            checkout
                .value("uses")
                .is_some_and(|u| u.starts_with("actions/checkout@"))
                && checkout.has("fetch-depth: 2"),
            "{rel}: the checkout fetches the merge commit's parents (fetch-depth: 2)"
        );
        let scope = &steps[1];
        assert!(
            scope.has("id: scope")
                && scope.has("EVENT: ${{ github.event_name }}")
                && scope.has(SCOPE_RUN),
            "{rel}: the second step is `id: scope`, running `{SCOPE_RUN}` with EVENT set"
        );
        for step in &steps[2..] {
            let cond = step.value("if").unwrap_or_default();
            assert!(
                cond.contains("steps.scope.outputs.scope"),
                "{rel}: every step after the decision depends on it: {:?}",
                step.lines.first()
            );
        }
    }

    let gate_steps = steps(".github/workflows/gate.yml");
    let check = gate_steps
        .iter()
        .find(|s| s.value("run") == Some("task check"))
        .expect("gate.yml runs task check");
    assert_eq!(
        check.value("if"),
        Some("steps.scope.outputs.scope == 'code'"),
        "gate.yml: `task check` runs for a change to the code"
    );
    let docs = gate_steps
        .iter()
        .find(|s| s.value("run") == Some("task site && task site-ux"))
        .expect("gate.yml runs the docs checks for a change to website/");
    assert_eq!(
        docs.value("if"),
        Some("steps.scope.outputs.scope == 'docs'"),
        "gate.yml: the docs checks, not the code's build and tests, run for website/ alone"
    );
    for step in &gate_steps {
        if step
            .value("uses")
            .is_some_and(|u| u.starts_with("dtolnay/rust-toolchain@"))
            || step.has("id: rustc")
        {
            assert_eq!(
                step.value("if"),
                Some("steps.scope.outputs.scope == 'code'"),
                "gate.yml: rustc is installed only for a change to the code"
            );
        }
    }

    for step in &steps(".github/workflows/pages.yml")[2..] {
        let cond = step.value("if").unwrap_or_default();
        assert!(
            cond.contains("steps.scope.outputs.scope != 'none'"),
            "pages.yml: a planning-only change skips {:?}",
            step.lines.first()
        );
    }
}
