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

    fn contains(&self, text: &str) -> bool {
        self.lines.iter().any(|l| l.contains(text))
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

/// What each step of the job that runs `task check` does before it, in order: the action
/// a step uses or the command it runs.
fn setup_before_the_gate(rel: &str) -> Vec<String> {
    let all = steps(rel);
    let job = all
        .iter()
        .find(|s| s.value("run") == Some("task check"))
        .unwrap_or_else(|| panic!("{rel}: a step runs `task check`"))
        .job
        .clone();
    all.iter()
        .filter(|s| s.job == job)
        .take_while(|s| s.value("run") != Some("task check"))
        .map(|s| {
            s.value("uses")
                .or_else(|| s.value("run"))
                .unwrap_or_else(|| panic!("{rel}, job {job}: a step uses an action or runs"))
                .to_string()
        })
        .collect()
}

/// The release of a tag runs `task check` on a runner prepared like the pull request's:
/// a tool one workflow installs and the other lacks fails the release after the merge
/// (`"ess": executable file not found in $PATH` in the release run of 0.39.0).
#[test]
fn the_release_prepares_its_runner_for_the_gate_like_a_pull_request() {
    let scope = SCOPE_RUN.trim_start_matches("run: ");
    let pull_request: Vec<String> = setup_before_the_gate(".github/workflows/gate.yml")
        .into_iter()
        .filter(|s| s != scope)
        .collect();
    assert!(
        pull_request.iter().any(|s| s.contains("install-ess.sh")),
        "gate.yml installs ess before `task check`: {pull_request:?}"
    );
    assert_eq!(
        setup_before_the_gate(".github/workflows/release.yml"),
        pull_request,
        "release.yml has the steps of gate.yml before `task check`, the scope step left out"
    );
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

const RELEASE: &str = ".github/workflows/release.yml";

/// The targets every release carries an archive for, sorted: both architectures, each
/// linked against glibc and as a static musl binary.
const RELEASE_TARGETS: [&str; 4] = [
    "aarch64-unknown-linux-gnu",
    "aarch64-unknown-linux-musl",
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
];

/// The condition of a step that only the musl targets of the package matrix run.
const MUSL_ONLY: &str = "endsWith(matrix.target, '-musl')";

/// The entries of the package matrix of `release.yml`: target and runner.
fn release_matrix() -> Vec<(String, String)> {
    let text = read(RELEASE);
    let mut out = Vec::new();
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if let Some(target) = line.strip_prefix("- target: ") {
            let runner = lines
                .next()
                .and_then(|l| l.strip_prefix("runner: "))
                .unwrap_or_else(|| panic!("release.yml: `runner:` follows `- target: {target}`"));
            out.push((target.to_string(), runner.to_string()));
        }
    }
    out
}

/// The steps of the job that builds one target of a release, in order.
fn package_steps() -> Vec<Step> {
    steps(RELEASE)
        .into_iter()
        .filter(|s| s.job == "package")
        .collect()
}

/// The place of the one step of `steps` that holds `text`.
fn place(steps: &[Step], text: &str) -> usize {
    let found: Vec<usize> = (0..steps.len())
        .filter(|i| steps[*i].contains(text))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "release.yml, job package: one step holds `{text}`"
    );
    found[0]
}

/// A release builds, smoke-tests and packages four targets, and both targets of an
/// architecture are built the same way: on that architecture's own runner, where the
/// binary runs.
#[test]
fn the_release_builds_smoke_tests_and_packages_four_targets() {
    let matrix = release_matrix();
    let mut targets: Vec<&str> = matrix.iter().map(|(t, _)| t.as_str()).collect();
    targets.sort_unstable();
    assert_eq!(
        targets, RELEASE_TARGETS,
        "release.yml: the package matrix names every release target once"
    );
    for arch in ["x86_64", "aarch64"] {
        let runners: std::collections::BTreeSet<&str> = matrix
            .iter()
            .filter(|(t, _)| t.starts_with(arch))
            .map(|(_, r)| r.as_str())
            .collect();
        assert_eq!(
            runners.len(),
            1,
            "release.yml: the {arch} targets are built on one runner: {runners:?}"
        );
    }

    let steps = package_steps();
    let install = steps
        .iter()
        .find(|s| {
            s.value("uses")
                .is_some_and(|u| u.starts_with("dtolnay/rust-toolchain@"))
        })
        .expect("release.yml, job package: installs a rustc");
    assert!(
        install.has("targets: ${{ matrix.target }}"),
        "release.yml, job package: the rustc install adds the target of the matrix entry"
    );
    let build = place(
        &steps,
        r#"cargo build --locked --release --bin slot --target "$TARGET""#,
    );
    let smoke = place(&steps, r#"printed=$("$bin" --version | awk '{print $NF}')"#);
    let package = place(&steps, r#"package="slot-$GITHUB_REF_NAME-$TARGET""#);
    let upload = place(&steps, "name: release-${{ matrix.target }}");
    assert!(
        build <= smoke && smoke <= package && package < upload,
        "release.yml, job package: build, smoke test, package, upload, in that order"
    );
    for i in [build, smoke, package, upload] {
        let step = &steps[i];
        assert_eq!(
            step.value("if"),
            None,
            "release.yml, job package: every target runs {:?}",
            step.lines.first()
        );
    }
    for i in [build, smoke, package] {
        assert!(
            steps[i].has("TARGET: ${{ matrix.target }}"),
            "release.yml, job package: {:?} reads the target of the matrix entry",
            steps[i].lines.first()
        );
    }
    assert!(
        steps[smoke].contains(r#"[ "$printed" != "$GITHUB_REF_NAME" ]"#),
        "release.yml, job package: the built binary answers the version of the tag"
    );
    assert!(
        steps[package].contains(r#"tar -C dist -czf "dist/$package.tar.gz" "$package""#),
        "release.yml, job package: the archive is slot-<version>-<target>.tar.gz"
    );
    assert!(
        steps[upload].has("path: dist/*.tar.gz") && steps[upload].has("if-no-files-found: error"),
        "release.yml, job package: the archive is uploaded, and a missing one fails the job"
    );
}

/// A musl binary is proven static and runs in a container without glibc before it is
/// packaged; the image is named by digest.
#[test]
fn the_release_proves_each_musl_binary_static_and_runs_it_in_alpine() {
    let steps = package_steps();
    let proofs: Vec<usize> = (0..steps.len())
        .filter(|i| steps[*i].value("if") == Some(MUSL_ONLY))
        .collect();
    assert_eq!(
        proofs.len(),
        1,
        "release.yml, job package: one step has `if: {MUSL_ONLY}`"
    );
    let proof = &steps[proofs[0]];
    let build = place(
        &steps,
        r#"cargo build --locked --release --bin slot --target "$TARGET""#,
    );
    let package = place(&steps, r#"package="slot-$GITHUB_REF_NAME-$TARGET""#);
    assert!(
        build < proofs[0] && proofs[0] < package,
        "release.yml, job package: the musl proof runs after the build and before the packaging"
    );
    assert!(
        proof.has("TARGET: ${{ matrix.target }}"),
        "release.yml: the musl proof reads the target of the matrix entry"
    );
    assert!(
        proof.has(r#"sh .github/scripts/static-binary.sh "$bin""#),
        "release.yml: the musl proof runs static-binary.sh on the built binary"
    );
    let image = proof
        .value("ALPINE")
        .expect("release.yml: the musl proof names its image in `ALPINE:`");
    let image = image.split(" #").next().unwrap_or_default().trim();
    let digest = image
        .strip_prefix("docker.io/library/alpine@sha256:")
        .unwrap_or_else(|| panic!("release.yml: the Alpine image is named by digest: `{image}`"));
    assert!(
        digest.len() == 64
            && digest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "release.yml: the Alpine digest is 64 hex digits: `{digest}`"
    );
    let run = proof
        .lines
        .iter()
        .find_map(|l| {
            let l = l.trim();
            l.starts_with("printed=$(docker run ").then_some(l)
        })
        .expect("release.yml: the musl proof reads the output of `docker run`");
    assert!(
        run.contains(r#""$ALPINE""#) && run.contains("/slot --version"),
        "release.yml: the musl proof runs `slot --version` in the Alpine image: `{run}`"
    );
    assert!(
        proof.contains(r#"[ "$printed" != "$GITHUB_REF_NAME" ]"#),
        "release.yml: the binary answers the version of the tag in Alpine"
    );
    for line in &proof.lines {
        assert!(
            !line.contains("alpine:"),
            "release.yml: the musl proof names no image by tag: `{}`",
            line.trim()
        );
    }
}

/// The bundle job fails when an archive of a release target is missing, and writes
/// `SHA256SUMS` over every archive.
#[test]
fn the_release_bundle_requires_and_lists_an_archive_of_every_target() {
    let steps: Vec<Step> = steps(RELEASE)
        .into_iter()
        .filter(|s| s.job == "bundle")
        .collect();
    let verify = steps
        .iter()
        .find(|s| s.contains("SHA256SUMS"))
        .expect("release.yml, job bundle: a step writes SHA256SUMS");
    let listed = verify
        .lines
        .iter()
        .find_map(|l| {
            l.trim()
                .strip_prefix("for target in ")
                .and_then(|l| l.strip_suffix("; do"))
        })
        .expect("release.yml, job bundle: `for target in <targets>; do`");
    let mut listed: Vec<&str> = listed.split_whitespace().collect();
    listed.sort_unstable();
    assert_eq!(
        listed, RELEASE_TARGETS,
        "release.yml, job bundle: the archive of every release target is required"
    );
    let mut built: Vec<String> = release_matrix().into_iter().map(|(t, _)| t).collect();
    built.sort_unstable();
    assert_eq!(
        listed, built,
        "release.yml: the bundle requires the targets the package matrix builds"
    );
    assert!(
        verify.contains(r#"test -f "dist/slot-$GITHUB_REF_NAME-$target.tar.gz""#),
        "release.yml, job bundle: a missing archive fails the job"
    );
    assert!(
        verify.contains("sha256sum slot-*.tar.gz > SHA256SUMS"),
        "release.yml, job bundle: SHA256SUMS lists every archive"
    );
}

/// The install page tells a reader which archive a host takes: it names every target.
#[test]
fn the_install_page_names_every_release_target() {
    let page = read("website/docs/getting-started.md");
    for target in RELEASE_TARGETS {
        assert!(
            page.contains(&format!("`{target}`")),
            "website/docs/getting-started.md names `{target}`"
        );
    }
}

/// `static-binary.sh` passes a binary that loads no shared library and refuses every
/// other file. The test binary itself is the case: static when its target links the C
/// library statically (a musl target), dynamic otherwise.
#[test]
fn the_static_check_passes_only_a_binary_that_loads_no_library() {
    let script = root().join(".github/scripts/static-binary.sh");
    let check = |file: &Path| {
        Command::new("sh")
            .arg(&script)
            .arg(file)
            .output()
            .expect("run sh")
    };
    let this = std::env::current_exe().expect("the path of this test binary");
    let out = check(&this);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if cfg!(target_feature = "crt-static") {
        assert!(out.status.success(), "a static binary passes: {stderr}");
        assert!(
            String::from_utf8_lossy(&out.stdout).starts_with("static: "),
            "a static binary is named on standard output"
        );
    } else {
        assert_eq!(
            out.status.code(),
            Some(1),
            "a binary that loads shared libraries is refused: {stderr}"
        );
        assert!(
            stderr.contains("is not static") && stderr.contains("NEEDED"),
            "the refusal shows the libraries the binary names: {stderr}"
        );
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let text = dir.path().join("slot");
    std::fs::write(&text, "#!/bin/sh\necho slot 0.0.0\n").expect("write a script");
    let out = check(&text);
    assert!(
        !out.status.success(),
        "a file that is no ELF binary is refused"
    );
    let out = check(&dir.path().join("missing"));
    assert!(!out.status.success(), "a missing file is refused");
}

/// The parent of the home directories. The tests build every path under it from this, so
/// this public source names no home path itself.
const HOMES: &str = "/home";

/// The release build step's lines that set `RUSTFLAGS`: from `export RUSTFLAGS=` to the
/// line before the cargo build.
fn release_rustflags_lines() -> Vec<String> {
    let steps = package_steps();
    let build = &steps[place(
        &steps,
        r#"cargo build --locked --release --bin slot --target "$TARGET""#,
    )];
    let lines: Vec<String> = build
        .lines
        .iter()
        .map(|l| l.trim().to_string())
        .skip_while(|l| !l.starts_with("export RUSTFLAGS="))
        .take_while(|l| !l.starts_with("cargo build "))
        .collect();
    assert!(
        !lines.is_empty(),
        "release.yml, job package: the build step sets `export RUSTFLAGS=` before `cargo build`"
    );
    lines
}

/// What the release build step sets `RUSTFLAGS` to, for one `HOME` and `CARGO_HOME`, run in
/// `dir`.
fn release_rustflags(dir: &Path, home: &str, cargo_home: Option<&str>) -> String {
    let mut script = release_rustflags_lines().join("\n");
    script.push_str("\nprintf '%s' \"$RUSTFLAGS\"\n");
    let mut cmd = Command::new("bash");
    cmd.arg("-euc")
        .arg(&script)
        .current_dir(dir)
        .env("HOME", home)
        .env("PWD", dir)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_HOME");
    if let Some(c) = cargo_home {
        cmd.env("CARGO_HOME", c);
    }
    let out = cmd.output().expect("run bash");
    assert!(
        out.status.success(),
        "the RUSTFLAGS lines of release.yml fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("RUSTFLAGS is UTF-8")
}

/// A release binary names no path of the build machine's home: the build remaps the home,
/// a cargo home outside it and the checkout to paths outside `/home`, and the checkout's
/// remap comes last, so rustc (which applies the last matching remap) names a source file
/// of the workspace `/src/…` even when the checkout lies under the home.
#[test]
fn the_release_build_remaps_the_home_and_the_checkout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let checkout = dir.path().canonicalize().expect("canonical tempdir");
    let pwd = checkout.to_str().expect("UTF-8 tempdir");
    let runner = format!("{HOMES}/runner");
    let runner_cargo = format!("{runner}/.cargo");
    let remaps = |flags: &str| -> Vec<(String, String)> {
        flags
            .split_whitespace()
            .map(|f| {
                let pair = f
                    .strip_prefix("--remap-path-prefix=")
                    .unwrap_or_else(|| panic!("RUSTFLAGS holds only remaps: `{f}`"));
                let (from, to) = pair
                    .rsplit_once('=')
                    .unwrap_or_else(|| panic!("a remap is FROM=TO: `{f}`"));
                (from.to_string(), to.to_string())
            })
            .collect()
    };

    for (cargo_home, extra) in [
        (None, None),
        (Some(runner_cargo.as_str()), None),
        (Some("/opt/cargo"), Some("/opt/cargo")),
    ] {
        let flags = release_rustflags(&checkout, &runner, cargo_home);
        let pairs = remaps(&flags);
        let from: Vec<&str> = pairs.iter().map(|(f, _)| f.as_str()).collect();
        assert!(
            from.contains(&runner.as_str()),
            "the runner's home is remapped (CARGO_HOME {cargo_home:?}): {flags}"
        );
        assert_eq!(
            from.last(),
            Some(&pwd),
            "the checkout's remap comes last (CARGO_HOME {cargo_home:?}): {flags}"
        );
        assert_eq!(
            extra.is_some_and(|e| from.contains(&e)),
            extra.is_some(),
            "a CARGO_HOME outside the home is remapped too, one inside is not again: {flags}"
        );
        for (_, to) in &pairs {
            assert!(
                !to.starts_with(HOMES) && to.starts_with('/'),
                "a remap names an absolute path outside /home: `{to}` in {flags}"
            );
        }
    }
}

/// After the build, every release binary is checked for a path of a home directory, on
/// every target, before it is packaged.
#[test]
fn the_release_checks_every_binary_for_a_home_path_before_packaging() {
    let steps = package_steps();
    let build = place(
        &steps,
        r#"cargo build --locked --release --bin slot --target "$TARGET""#,
    );
    let check = place(&steps, r#"sh .github/scripts/no-home-path.sh "$bin""#);
    let package = place(&steps, r#"package="slot-$GITHUB_REF_NAME-$TARGET""#);
    assert!(
        build <= check && check < package,
        "release.yml, job package: the home-path check runs after the build, before packaging"
    );
    assert_eq!(
        steps[check].value("if"),
        None,
        "release.yml, job package: every target is checked for a home path"
    );
    let lines: Vec<&str> = steps[check].lines.iter().map(|l| l.trim()).collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("sh .github/scripts/no-home-path.sh"))
        .expect("the check line");
    if build == check {
        let built = lines
            .iter()
            .position(|l| l.starts_with("cargo build "))
            .expect("the build line");
        assert!(built < at, "the check runs after `cargo build` in its step");
    }
    assert!(
        lines[at].contains("::error::"),
        "release.yml: a binary that names a home path fails the step with `::error::`: `{}`",
        lines[at]
    );
}

/// `no-home-path.sh` refuses a file that holds `/home/` or the caller's `$HOME/`, and
/// passes one that holds neither.
#[test]
fn the_home_path_check_refuses_a_file_that_names_a_home_directory() {
    let script = root().join(".github/scripts/no-home-path.sh");
    let runner = format!("{HOMES}/runner");
    let someone = format!("{HOMES}/someone/x");
    let dir = tempfile::tempdir().expect("tempdir");
    let check = |name: &str, bytes: &[u8], home: &str| {
        let file = dir.path().join(name);
        std::fs::write(&file, bytes).expect("write a case");
        Command::new("sh")
            .arg(&script)
            .arg(&file)
            .env("HOME", home)
            .output()
            .expect("run sh")
    };

    let out = check(
        "under-home",
        format!("\x7fELF\0src/lib.rs\0{someone}\0tail").as_bytes(),
        "/var/lib/builder",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a file with {someone} is refused: {stderr}"
    );
    assert!(
        stderr.contains(&someone),
        "the refusal shows the path it found: {stderr}"
    );

    let out = check(
        "callers-home",
        b"\x7fELF\0/var/lib/builder/.cargo/registry/src/a.rs\0",
        "/var/lib/builder",
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a file with a path under $HOME outside /home is refused: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let clean =
        b"\x7fELF\0/src/crates/slot/src/lib.rs\0/cargo-home/.cargo/registry/a.rs\0/homeless\0";
    for home in [runner.as_str(), "/", ""] {
        let out = check("clean", clean, home);
        assert!(
            out.status.success(),
            "a file with no home path passes with HOME={home:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).starts_with("no home path: "),
            "a passing file is named on standard output"
        );
    }

    let out = check("ignored", b"", &runner);
    assert!(out.status.success(), "an empty file passes");
    let out = Command::new("sh")
        .arg(&script)
        .arg(dir.path().join("missing"))
        .output()
        .expect("run sh");
    assert!(!out.status.success(), "a missing file is refused");
}
