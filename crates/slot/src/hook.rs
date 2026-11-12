//! `slot hook claude-pre-tool-use`: a Claude Code `PreToolUse` hook for the
//! Bash tool that refuses a command which compiles or runs tests outside the
//! shared build pool, and tells the agent the `slot` command to run instead.
//!
//! [`verdict`] is pure: it splits the command line into simple commands and
//! checks each. [`run`] is the hook around it: one event on stdin; exit 0 to
//! allow; exit 2 with the advice on stderr to refuse; exit 0 on anything it
//! cannot read, so a broken hook never blocks work.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// What the hook decided about one command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing in it needs the pool, or it already goes through slot.
    Allow,
    /// A simple command that should run through slot, and the advice.
    Refuse(String),
}

/// What the environment tells the hook.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// `cargo` already goes through slot (the PATH shim is installed).
    pub cargo_shim: bool,
    /// `go` already goes through slot.
    pub go_shim: bool,
    /// `ignite` already goes through slot.
    pub ignite_shim: bool,
}

impl Env {
    /// Reads the shim state: a shim on `PATH`, or installed under
    /// `~/.local/share/slot/shims` (agent shells put that first on `PATH`;
    /// the hook's own environment may not have it).
    pub fn detect() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = std::env::var_os("PATH").unwrap_or_default();
        let shim = |tool: &str| {
            if let Some(h) = &home
                && h.join(".local/share/slot/shims").join(tool).exists()
            {
                return true;
            }
            std::env::split_paths(&path)
                .map(|d| d.join(tool))
                .find(|p| p.is_file())
                .is_some_and(|p| is_shim(&p))
        };
        Self {
            cargo_shim: shim("cargo"),
            go_shim: shim("go"),
            ignite_shim: shim("ignite"),
        }
    }
}

fn is_shim(p: &Path) -> bool {
    p.parent().is_some_and(|d| d.ends_with("slot/shims"))
        || std::fs::read_link(p)
            .ok()
            .and_then(|t| t.file_name().map(|n| n == "slot-shim"))
            .unwrap_or(false)
}

/// The hook: read one `PreToolUse` event on stdin and answer it.
pub fn run() -> ExitCode {
    let mode = std::env::var("SLOT_HOOK").unwrap_or_default();
    if mode == "off" || std::env::var_os("SLOT_JOB").is_some_and(|v| !v.is_empty()) {
        return ExitCode::SUCCESS;
    }
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return ExitCode::SUCCESS;
    }
    let Ok(event) = serde_json::from_str::<serde_json::Value>(&input) else {
        return ExitCode::SUCCESS;
    };
    if event.get("tool_name").and_then(|v| v.as_str()) != Some("Bash") {
        return ExitCode::SUCCESS;
    }
    let Some(command) = event
        .pointer("/tool_input/command")
        .and_then(|v| v.as_str())
    else {
        return ExitCode::SUCCESS;
    };
    let env = Env::detect();
    match std::panic::catch_unwind(|| verdict(command, &env)) {
        Ok(Verdict::Refuse(advice)) if mode == "warn" => {
            let out = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "additionalContext": advice,
                }
            });
            println!("{out}");
            ExitCode::SUCCESS
        }
        Ok(Verdict::Refuse(advice)) => {
            eprintln!("{advice}");
            ExitCode::from(2)
        }
        _ => ExitCode::SUCCESS,
    }
}

/// Decide one command line.
pub fn verdict(command: &str, env: &Env) -> Verdict {
    check_script(command, env, 0)
}

fn check_script(script: &str, env: &Env, depth: usize) -> Verdict {
    for words in simple_commands(script) {
        if let v @ Verdict::Refuse(_) = check_command(&words, env, depth) {
            return v;
        }
    }
    Verdict::Allow
}

fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn is_assignment(w: &str) -> bool {
    match w.split_once('=') {
        Some((k, _)) => {
            !k.is_empty()
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !k.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

fn check_command(words: &[String], env: &Env, depth: usize) -> Verdict {
    let mut i = 0;
    let mut shim_off = false;
    // Leading assignments and wrappers that run the rest of the line.
    loop {
        let Some(w) = words.get(i) else {
            return Verdict::Allow;
        };
        if is_assignment(w) {
            if w == "SLOT_SHIM_OFF=1" {
                shim_off = true;
            }
            i += 1;
            continue;
        }
        match base(w) {
            "env" => {
                i += 1;
                while words
                    .get(i)
                    .is_some_and(|a| a.starts_with('-') || is_assignment(a))
                {
                    if words[i] == "SLOT_SHIM_OFF=1" {
                        shim_off = true;
                    }
                    i += 1;
                }
            }
            "time" | "nohup" | "command" | "exec" => i += 1,
            "nice" | "ionice" => {
                i += 1;
                while words.get(i).is_some_and(|a| a.starts_with('-')) {
                    let takes = matches!(words[i].as_str(), "-n" | "-c" | "--adjustment");
                    i += if takes { 2 } else { 1 };
                }
            }
            "timeout" => {
                i += 1;
                while words.get(i).is_some_and(|a| a.starts_with('-')) {
                    i += 1;
                }
                i += 1;
            }
            _ => break,
        }
    }
    if shim_off {
        return Verdict::Allow;
    }
    let cmd = &words[i..];
    let prog = base(&cmd[0]);
    let args = &cmd[1..];
    let line = cmd.join(" ");
    // `slot …` itself falls through to Allow.
    match prog {
        "bash" | "sh" | "zsh" | "dash" if depth < 3 => {
            match args
                .iter()
                .position(|a| a == "-c" || a == "-lc" || a == "-ec")
            {
                Some(p) => args
                    .get(p + 1)
                    .map_or(Verdict::Allow, |s| check_script(s, env, depth + 1)),
                None => Verdict::Allow,
            }
        }
        "cargo" if !env.cargo_shim => match cargo_sub(args) {
            Some(sub) if CARGO_BUILD.contains(&sub) => refuse(&line, None),
            _ => Verdict::Allow,
        },
        "go" if !env.go_shim => match args.first().map(String::as_str) {
            Some("build" | "test" | "vet" | "install") => refuse(&line, None),
            _ => Verdict::Allow,
        },
        "ignite" if !env.ignite_shim => match first_word(args) {
            Some("build" | "test" | "conformance") => refuse(&line, None),
            _ => Verdict::Allow,
        },
        "zig" | "zig.sh" => zig(args, &line),
        "mvn" | "mvnw" | "gradle" | "gradlew" => {
            if !watches_or_prints(args)
                && targets(args, &["-f", "--file", "-p", "--project-dir", "-pl", "-D"])
                    .any(|t| is_build_target(t) || matches!(t, "package" | "assemble"))
            {
                refuse(&line, None)
            } else {
                Verdict::Allow
            }
        }
        "pytest" | "py.test" if !watches_or_prints(args) => refuse(&line, None),
        p if p.starts_with("python") => match args {
            [m, t, ..] if m == "-m" && t == "pytest" && !watches_or_prints(args) => {
                refuse(&line, None)
            }
            _ => Verdict::Allow,
        },
        "npx" | "bunx" => match first_word(args) {
            Some(r) => runner(r, args, &line),
            None => Verdict::Allow,
        },
        "yarn" | "pnpm" | "npm" => package_manager(prog, args, &line),
        "node" => node(args, &line),
        "playwright" | "vitest" | "jest" | "tsc" | "vue-tsc" | "eslint" => {
            runner(&cmd[0], args, &line)
        }
        "make" | "gmake" => {
            const QUIET: &[&str] = &[
                "-n",
                "--dry-run",
                "-q",
                "--question",
                "-p",
                "-v",
                "--version",
                "--help",
                "-h",
            ];
            if args.iter().any(|a| QUIET.contains(&a.as_str())) {
                Verdict::Allow
            } else if args.iter().any(|a| a.starts_with("-j"))
                || targets(args, &["-C", "-f", "--directory", "--file", "-I"]).any(is_build_target)
            {
                refuse(&line, None)
            } else {
                Verdict::Allow
            }
        }
        "task" => {
            const QUIET: &[&str] = &[
                "--list",
                "-l",
                "--list-all",
                "-a",
                "--help",
                "-h",
                "--version",
                "--summary",
                "--init",
                "-i",
                "--dry",
                "-n",
            ];
            if args.iter().any(|a| QUIET.contains(&a.as_str())) {
                Verdict::Allow
            } else if targets(args, &["-d", "--dir", "-t", "--taskfile", "-o", "--output"])
                .any(is_build_target)
            {
                refuse(&line, None)
            } else {
                Verdict::Allow
            }
        }
        _ => Verdict::Allow,
    }
}

/// The target names of a `make` or `task` call: words that are not flags,
/// flag values or assignments.
fn targets<'a>(args: &'a [String], valued: &'a [&str]) -> impl Iterator<Item = &'a str> {
    let mut skip = false;
    args.iter().filter_map(move |a| {
        if std::mem::take(&mut skip) {
            return None;
        }
        if valued.contains(&a.as_str()) {
            skip = true;
            return None;
        }
        (!a.starts_with('-') && !is_assignment(a)).then_some(a.as_str())
    })
}

/// Words of a target name that mean it compiles or runs tests.
const BUILD_WORDS: &[&str] = &[
    "all",
    "bench",
    "build",
    "check",
    "ci",
    "clippy",
    "compile",
    "cov",
    "coverage",
    "e2e",
    "gate",
    "generate",
    "install",
    "lint",
    "nextest",
    "test",
    "tests",
    "typecheck",
    "verify",
];

/// Words of a target name that mean it starts something long-running or
/// talks to another system; those are never sent to the pool.
const OTHER_WORDS: &[&str] = &[
    "deploy", "dev", "down", "logs", "merge", "push", "serve", "server", "shell", "start", "up",
    "watch",
];

/// Whether a `make` or `task` target compiles or runs tests, judged by the
/// words of its name (`ux:check`, `test-xtask`, `build_all`).
fn is_build_target(name: &str) -> bool {
    let words: Vec<String> = name
        .split([':', '-', '_', '.', '/'])
        .map(str::to_ascii_lowercase)
        .collect();
    words.iter().any(|w| BUILD_WORDS.contains(&w.as_str()))
        && !words.iter().any(|w| OTHER_WORDS.contains(&w.as_str()))
}

const CARGO_BUILD: &[&str] = &[
    "build", "b", "check", "c", "test", "t", "clippy", "doc", "d", "bench", "nextest", "install",
    "rustc", "llvm-cov", "miri",
];

/// cargo's subcommand: the first word that is not a global flag or its value.
fn cargo_sub(args: &[String]) -> Option<&str> {
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        match a.as_str() {
            "--color" | "--config" | "-Z" | "-C" | "--manifest-path" => skip = true,
            s if s.starts_with('+') || s.starts_with('-') => {}
            s => return Some(s),
        }
    }
    None
}

fn first_word(args: &[String]) -> Option<&str> {
    args.iter()
        .map(String::as_str)
        .find(|a| !a.starts_with('-'))
}

fn package_manager(prog: &str, args: &[String], line: &str) -> Verdict {
    let mut it = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with('-'));
    let Some(first) = it.next() else {
        return Verdict::Allow;
    };
    match first {
        "test" | "t" => refuse(line, None),
        "run" | "run-script" => match it.next() {
            Some("test") => refuse(line, None),
            Some(r) => runner(r, args, line),
            None => Verdict::Allow,
        },
        "exec" | "dlx" | "x" => match it.next() {
            Some(r) => runner(r, args, line),
            None => Verdict::Allow,
        },
        r if prog != "npm" => runner(r, args, line),
        _ => Verdict::Allow,
    }
}

/// Words of a package.json script name that mean it compiles or runs tests.
/// Narrower than [`BUILD_WORDS`]: `install`, `ci`, `all` and `generate` are
/// package-manager commands or code generators there.
const SCRIPT_WORDS: &[&str] = &[
    "bench",
    "build",
    "check",
    "compile",
    "cov",
    "coverage",
    "e2e",
    "gate",
    "lint",
    "test",
    "tests",
    "typecheck",
    "verify",
];

/// Whether a package.json script, or the subcommand of a repository's node
/// script, compiles or runs tests (`build`, `test:unit`, `ux:gate`), judged
/// by its words; never one that starts a server or a watcher.
fn is_build_script(name: &str) -> bool {
    let words: Vec<String> = name
        .split([':', '-', '_', '.', '/'])
        .map(str::to_ascii_lowercase)
        .collect();
    words.iter().any(|w| SCRIPT_WORDS.contains(&w.as_str()))
        && !words.iter().any(|w| OTHER_WORDS.contains(&w.as_str()))
}

/// Flags that make a tool watch, serve or only print something.
fn watches_or_prints(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--watch" | "-w" | "--help" | "-h" | "--version" | "-v" | "-V" | "--list" | "serve"
        )
    })
}

/// `zig build`, `zig test`, `zig build-exe|lib|obj`, also behind a
/// repository's `zig.sh` wrapper; a `zig build run` step starts the program.
fn zig(args: &[String], line: &str) -> Verdict {
    match first_word(args) {
        Some("build") if !args.iter().any(|a| a == "run") => refuse(line, None),
        Some("test" | "build-exe" | "build-lib" | "build-obj") => refuse(line, None),
        _ => Verdict::Allow,
    }
}

/// `node --test`, a runner under `node_modules/`, or a repository script
/// called with a build or test subcommand (`node tools/ux.mjs gate`).
fn node(args: &[String], line: &str) -> Verdict {
    if args.iter().any(|a| a == "--test") {
        return refuse(line, None);
    }
    let Some(script) = first_word(args) else {
        return Verdict::Allow;
    };
    if script.contains("node_modules/") {
        return runner(script, args, line);
    }
    let rest = args
        .iter()
        .skip_while(|a| a.as_str() != script)
        .skip(1)
        .filter(|a| !a.starts_with('-') && !a.contains(['=', '/', '.', '$']));
    if !watches_or_prints(args) && rest.take(2).any(|a| is_build_script(a)) {
        return refuse(line, None);
    }
    Verdict::Allow
}

/// A JavaScript test runner, compiler or linter word (`vitest`,
/// `node_modules/.bin/jest`, `tsc`), or a package.json script name.
fn runner(word: &str, args: &[String], line: &str) -> Verdict {
    let flag = match base(word) {
        "playwright" if args.iter().any(|a| a == "test") => "--workers=2",
        "vitest" | "jest" => "--maxWorkers=4",
        "tsc" | "vue-tsc" | "eslint" if !watches_or_prints(args) => return refuse(line, None),
        "vite" | "docusaurus" | "webpack" | "next" | "nuxt" | "astro" | "rollup" | "esbuild"
            if args.iter().any(|a| a == "build") =>
        {
            return refuse(line, None);
        }
        "playwright" | "tsc" | "vue-tsc" | "eslint" | "vite" | "docusaurus" | "webpack"
        | "next" | "nuxt" | "astro" | "rollup" | "esbuild" => {
            return Verdict::Allow;
        }
        w if !word.contains('/') && is_build_script(w) && !watches_or_prints(args) => {
            return refuse(line, None);
        }
        _ => return Verdict::Allow,
    };
    let has_flag = args.iter().any(|a| {
        let k = a.split('=').next().unwrap_or(a);
        matches!(k, "--workers" | "-j" | "--maxWorkers" | "--max-workers")
    });
    refuse(line, (!has_flag).then_some(flag))
}

fn refuse(line: &str, add_flag: Option<&str>) -> Verdict {
    let mut cmd = format!("slot {line}");
    if let Some(f) = add_flag {
        cmd.push(' ');
        cmd.push_str(f);
    }
    if cmd.chars().count() > 160 {
        cmd = cmd.chars().take(159).collect::<String>() + "…";
    }
    let short = if line.chars().count() > 60 {
        line.chars().take(59).collect::<String>() + "…"
    } else {
        line.to_string()
    };
    Verdict::Refuse(format!(
        "slot: `{short}` compiles or runs tests outside the shared build pool. Run `{cmd}` instead{}.",
        if add_flag.is_some() {
            " (the job holds one slot per worker, so name the worker count)"
        } else {
            ""
        }
    ))
}

/// Splits a shell command line into simple commands (lists of words), on
/// `;`, `&`, `&&`, `|`, `||`, newlines and parentheses. Quotes and
/// backslashes are honoured; heredoc bodies and comments are dropped.
/// Command substitutions inside double quotes are not examined.
pub fn simple_commands(src: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut heredocs: Vec<String> = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let flush_word = |word: &mut String, in_word: &mut bool, words: &mut Vec<String>| {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '"' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '\\' => {
                if i + 1 < chars.len() && chars[i + 1] != '\n' {
                    word.push(chars[i + 1]);
                    in_word = true;
                }
                i += 1;
            }
            '#' if !in_word => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) != Some(&'<') => {
                flush_word(&mut word, &mut in_word, &mut words);
                i += 2;
                if chars.get(i) == Some(&'-') {
                    i += 1;
                }
                while chars.get(i).is_some_and(|c| *c == ' ' || *c == '\t') {
                    i += 1;
                }
                let mut delim = String::new();
                while let Some(&d) = chars.get(i) {
                    if d.is_whitespace() || ";&|()<>".contains(d) {
                        break;
                    }
                    if d != '\'' && d != '"' && d != '\\' {
                        delim.push(d);
                    }
                    i += 1;
                }
                if !delim.is_empty() {
                    heredocs.push(delim);
                }
                continue;
            }
            '\n' => {
                flush_word(&mut word, &mut in_word, &mut words);
                if !words.is_empty() {
                    out.push(std::mem::take(&mut words));
                }
                i += 1;
                // Skip heredoc bodies that start on the next line.
                for delim in std::mem::take(&mut heredocs) {
                    while i < chars.len() {
                        let end = chars[i..]
                            .iter()
                            .position(|c| *c == '\n')
                            .map_or(chars.len(), |p| i + p);
                        let line: String = chars[i..end].iter().collect();
                        i = (end + 1).min(chars.len());
                        if line.trim() == delim {
                            break;
                        }
                    }
                }
                continue;
            }
            ';' | '&' | '|' | '(' | ')' => {
                flush_word(&mut word, &mut in_word, &mut words);
                if !words.is_empty() {
                    out.push(std::mem::take(&mut words));
                }
            }
            '<' | '>' => {
                // A redirection ends the word; its target is not a command word.
                flush_word(&mut word, &mut in_word, &mut words);
                while chars
                    .get(i + 1)
                    .is_some_and(|c| *c == '>' || *c == '&' || *c == '<')
                {
                    i += 1;
                }
                i += 1;
                while chars.get(i).is_some_and(|c| *c == ' ' || *c == '\t') {
                    i += 1;
                }
                while chars
                    .get(i)
                    .is_some_and(|c| !c.is_whitespace() && !";&|()".contains(*c))
                {
                    i += 1;
                }
                continue;
            }
            c if c.is_whitespace() => flush_word(&mut word, &mut in_word, &mut words),
            c => {
                word.push(c);
                in_word = true;
            }
        }
        i += 1;
    }
    flush_word(&mut word, &mut in_word, &mut words);
    if !words.is_empty() {
        out.push(words);
    }
    // A redirection like `2>&1` leaves a stray digit word before it.
    for w in &mut out {
        if w.last()
            .is_some_and(|l| l.chars().all(|c| c.is_ascii_digit()))
            && w.len() > 1
        {
            w.pop();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare() -> Env {
        Env::default()
    }

    fn shimmed() -> Env {
        Env {
            cargo_shim: true,
            go_shim: true,
            ignite_shim: true,
        }
    }

    #[test]
    fn refuses_the_other_toolchains_outside_the_pool() {
        for cmd in [
            "ignite build x.ignite --release",
            "~/.local/bin/ignite test a.ignite --target all",
            "ignite conformance a.ignite",
            "zig build -Doptimize=ReleaseSafe",
            "scripts/zig.sh build test",
            "./scripts/zig.sh test src/main.zig",
            "zig build-exe main.zig",
            "pytest -q tests",
            "python3 -m pytest -x",
            "mvn -q clean package",
            "./mvnw verify -pl core",
            "./gradlew test",
            "gradle assemble",
            "npx tsc -p tsconfig.json --noEmit",
            "pnpm exec vue-tsc --noEmit",
            "npx eslint .",
            "yarn typecheck",
            "pnpm build",
            "npm run build",
            "pnpm run test:unit",
            "pnpm lint",
            "npx vite build",
            "npx docusaurus build",
            "node --test tests/",
            "node tools/ux/ux.mjs gate",
            "node scripts/run.mjs check --strict",
        ] {
            assert!(refused(cmd, &bare()), "should refuse: {cmd}");
        }
        for cmd in [
            "ignite check a.ignite",
            "ignite emit a.ignite",
            "ignite run a.ignite --clock manual",
            "ignite host --state s",
            "ignite --version",
            "zig fmt src",
            "zig version",
            "scripts/zig.sh fetch",
            "zig build run",
            "python3 -m http.server 8000",
            "mvn spring-boot:run",
            "mvn dependency:tree",
            "./gradlew tasks",
            "./gradlew --version",
            "python3 scripts/report.py",
            "npx tsc --watch",
            "npx tsc --version",
            "pnpm dev",
            "pnpm build --watch",
            "pnpm run test:watch",
            "pnpm add -D vitest",
            "pnpm generate",
            "yarn",
            "npx vite",
            "npx docusaurus start",
            "node server.js",
            "node scripts/build-report.mjs --port 3000 serve",
            "node -e 'console.log(1)'",
            "node tools/ux/ux.mjs serve",
            "node recount.mjs ../../review-coverage/out.txt",
            "node lib/seed.mjs $WK seed-test.tsv",
            "node suite.mjs build/review/probe.json",
            "python3 -m pytest --version",
        ] {
            assert_eq!(verdict(cmd, &bare()), Verdict::Allow, "should allow: {cmd}");
        }
        assert_eq!(
            verdict("ignite test a.ignite", &shimmed()),
            Verdict::Allow,
            "the shim covers ignite"
        );
    }

    fn refused(cmd: &str, env: &Env) -> bool {
        matches!(verdict(cmd, env), Verdict::Refuse(_))
    }

    #[test]
    fn refuses_builds_outside_the_pool() {
        for cmd in [
            "cargo build",
            "cargo test -p foo -- --nocapture",
            "cargo +nightly --locked clippy --all-targets",
            "CARGO_INCREMENTAL=0 cargo t",
            "cd crates/x && cargo nextest run",
            "env RUST_LOG=debug cargo test",
            "time cargo build --release",
            "nice -n 10 cargo check",
            "timeout 600 cargo test",
            "go test ./...",
            "go vet ./... | tee out.txt",
            "npx playwright test --project=chromium",
            "npx vitest run a.test.ts",
            "yarn vitest run",
            "pnpm exec jest",
            "pnpm vitest",
            "bunx vitest",
            "node_modules/.bin/playwright test",
            "./node_modules/.bin/vitest run",
            "node node_modules/.bin/jest",
            "npm test",
            "npm run test",
            "yarn test",
            "pnpm test",
            "make -j8",
            "make all",
            "make -C sub test",
            "task check",
            "task ux:check",
            "task -d crates/x test",
            "task test-xtask",
            "task generate:observe",
            "task build_all",
            "bash -c 'cargo build'",
            "sh -c \"npx vitest run\"",
            "echo start; cargo build 2>&1 | tail -3",
            "(cd ui && npx vitest run)",
            "git pull && cargo test",
        ] {
            assert!(refused(cmd, &bare()), "should refuse: {cmd}");
        }
    }

    #[test]
    fn allows_everything_else() {
        for cmd in [
            "git commit -m \"cargo test\"",
            "git commit -m 'make all && cargo build'",
            "echo make",
            "echo cargo build",
            "cat Makefile",
            "grep vitest package.json",
            "rg 'npx playwright test' docs",
            "slot task check",
            "slot cargo test -p foo",
            "SLOT_SHIM_OFF=1 cargo build",
            "env SLOT_SHIM_OFF=1 go test ./...",
            "cargo --version",
            "cargo fmt --all",
            "cargo metadata --format-version 1",
            "cargo run -- --help",
            "go version",
            "go mod tidy",
            "go run ./cmd/x",
            "npx prettier --check .",
            "yarn install",
            "pnpm install --frozen-lockfile",
            "npm ci",
            "npx playwright install chromium",
            "make",
            "make -n all",
            "make --version",
            "task",
            "task --list",
            "task -l",
            "task deploy:dev",
            "task local:up",
            "task wave:watch",
            "task deploy:check",
            "task stats",
            "task -d check render",
            "make deploy",
            "make -C test clean",
            "ls -la # cargo build",
            "cat > script.sh <<'EOF'\ncargo build\nnpx vitest run\nEOF\nchmod +x script.sh",
            "python3 -c 'import os'",
            "",
        ] {
            assert_eq!(verdict(cmd, &bare()), Verdict::Allow, "should allow: {cmd}");
        }
    }

    #[test]
    fn the_shim_covers_cargo_and_go() {
        for cmd in ["cargo build", "cargo test -p x", "go test ./..."] {
            assert_eq!(verdict(cmd, &shimmed()), Verdict::Allow, "{cmd}");
        }
        assert!(refused("npx vitest run", &shimmed()));
        assert!(refused("task check", &shimmed()));
    }

    #[test]
    fn advice_names_the_slot_command_and_the_worker_flag() {
        let Verdict::Refuse(m) = verdict("npx vitest run a.test.ts", &bare()) else {
            panic!("allowed")
        };
        assert!(
            m.contains("`slot npx vitest run a.test.ts --maxWorkers=4`"),
            "{m}"
        );
        assert!(m.contains("one slot per worker"), "{m}");
        assert_eq!(m.lines().count(), 1, "{m}");
        let Verdict::Refuse(m) = verdict("npx playwright test --workers=3", &bare()) else {
            panic!("allowed")
        };
        assert!(m.contains("`slot npx playwright test --workers=3`"), "{m}");
        assert!(!m.contains("one slot per worker"), "{m}");
        let Verdict::Refuse(m) = verdict("cd x && cargo test -p y", &bare()) else {
            panic!("allowed")
        };
        assert!(m.contains("`slot cargo test -p y`"), "{m}");
    }

    #[test]
    fn splits_simple_commands() {
        let got = simple_commands("A=1 cargo test 2>&1 | tail -3; echo \"a;b\" && x 'y z'");
        let want: Vec<Vec<String>> = vec![
            vec!["A=1".into(), "cargo".into(), "test".into()],
            vec!["tail".into(), "-3".into()],
            vec!["echo".into(), "a;b".into()],
            vec!["x".into(), "y z".into()],
        ];
        assert_eq!(got, want);
    }
}
