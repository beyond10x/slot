//! Cases written against the path decision and the gate's docs scope.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use super::{read, root};

/// The scope `pr-scope.sh --classify` gives one path.
fn classify(path: &str) -> String {
    let mut child = Command::new("sh")
        .arg(root().join(".github/scripts/pr-scope.sh"))
        .arg("--classify")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("run sh");
    writeln!(child.stdin.take().unwrap(), "{path}").unwrap();
    let out = child.wait_with_output().expect("wait for sh");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Every `website/…` path named in a string literal of an integration test of a crate
/// (the ci module itself excluded): the files under website/ that `cargo test` reads.
fn website_paths_read_by_cargo_test() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "ci") {
                    continue;
                }
                walk(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root().join("crates")).unwrap().flatten() {
        let tests = krate.path().join("tests");
        if tests.is_dir() {
            walk(&tests, &mut files);
        }
    }
    let mut paths = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, _) in text.match_indices("website/") {
            let rest = &text[i..];
            let end = rest.find('"').unwrap_or(rest.len());
            let path = rest[..end].to_string();
            if !path.contains(char::is_whitespace) && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

/// A pull request that changes only a file under website/ which a cargo test reads gets
/// the docs scope, and the gate's docs step runs no `cargo test`: the test that reads it
/// first runs on main, after the merge.
#[test]
fn a_website_file_that_cargo_test_reads_is_checked_before_the_merge() {
    let paths = website_paths_read_by_cargo_test();
    assert!(
        paths.contains(&"website/docs/getting-started.md".to_string()),
        "docs_version.rs reads website/docs/getting-started.md: {paths:?}"
    );
    let gate = read(".github/workflows/gate.yml");
    let docs_step = gate
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("run: task site"))
        .unwrap_or_default()
        .to_string();
    for path in paths {
        let scope = classify(&path);
        assert!(
            scope == "scope=code" || docs_step.contains("cargo test"),
            "{path} is read by `cargo test`, but a pull request changing only it gets {scope}, \
             and the gate's docs step (`{docs_step}`) runs no cargo test"
        );
    }
}

/// One step of gate.yml: its `if:`, `uses:` and `run:` values.
struct Step {
    cond: String,
    uses: String,
    run: String,
}

fn gate_steps(text: &str) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::new();
    let mut in_steps = false;
    for line in text.lines() {
        if line == "    steps:" {
            in_steps = true;
            continue;
        }
        if !in_steps {
            continue;
        }
        let body = line.trim().trim_start_matches("- ");
        if line.starts_with("      - ") {
            out.push(Step {
                cond: String::new(),
                uses: String::new(),
                run: String::new(),
            });
        }
        let Some(step) = out.last_mut() else { continue };
        if let Some(v) = body.strip_prefix("if: ") {
            step.cond = v.to_string();
        } else if let Some(v) = body.strip_prefix("uses: ") {
            step.uses = v.to_string();
        } else if let Some(v) = body.strip_prefix("run: ") {
            step.run = v.to_string();
        }
    }
    out
}

/// Whether a step with condition `cond` runs for `scope`; the forms gate.yml uses.
fn runs(cond: &str, scope: &str) -> bool {
    if cond.is_empty() {
        return true;
    }
    if let Some(want) = cond.strip_prefix("steps.scope.outputs.scope == ") {
        return want.trim_matches('\'') == scope;
    }
    if let Some(not) = cond.strip_prefix("steps.scope.outputs.scope != ") {
        return not.trim_matches('\'') != scope;
    }
    panic!("condition of a form this case does not read: {cond}");
}

/// A tool the docs step needs, and how a step that installs it reads.
type Need = (&'static str, fn(&Step) -> bool);

/// What the docs step needs installed before it in the docs scope, missing ones named.
fn docs_scope_gaps(text: &str) -> Vec<&'static str> {
    let steps = gate_steps(text);
    let Some(docs) = steps
        .iter()
        .position(|s| s.run == "task site && task site-ux")
    else {
        return vec!["the docs step"];
    };
    if !runs(&steps[docs].cond, "docs") {
        return vec!["the docs step runs in the docs scope"];
    }
    let before: Vec<&Step> = steps[..docs]
        .iter()
        .filter(|s| runs(&s.cond, "docs"))
        .collect();
    let need: [Need; 5] = [
        ("task", |s| s.uses.starts_with("arduino/setup-task@")),
        ("node", |s| s.uses.starts_with("actions/setup-node@")),
        ("pnpm", |s| s.uses.starts_with("pnpm/action-setup@")),
        ("Chromium headless shell", |s| {
            s.run.contains("playwright install") && s.run.contains("--only-shell chromium")
        }),
        ("DejaVu font", |s| s.run.contains("fonts-dejavu")),
    ];
    need.iter()
        .filter(|(_, ok)| !before.iter().any(|s| ok(s)))
        .map(|(name, _)| *name)
        .collect()
}

/// The existing case only asks that every step after the decision names
/// `steps.scope.outputs.scope`; a step that `task site-ux` needs turned to `== 'code'`
/// passes it. This one evaluates the conditions for the docs scope.
#[test]
fn the_docs_scope_of_the_gate_installs_what_task_site_and_site_ux_need() {
    let gate = read(".github/workflows/gate.yml");
    let gaps = docs_scope_gaps(&gate);
    assert!(
        gaps.is_empty(),
        "gate.yml, docs scope: missing before `task site && task site-ux`: {gaps:?}"
    );
    let browser_if =
        "        if: steps.scope.outputs.scope != 'none'\n        working-directory: ui";
    assert!(
        gate.contains(browser_if),
        "the browser step as this case knows it"
    );
    let mutant = gate.replace(
        browser_if,
        "        if: steps.scope.outputs.scope == 'code'\n        working-directory: ui",
    );
    assert_eq!(
        docs_scope_gaps(&mutant),
        vec!["Chromium headless shell", "DejaVu font"],
        "the browser step restricted to the code scope is a gap in the docs scope"
    );
}
