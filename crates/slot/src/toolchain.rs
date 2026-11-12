//! What slot knows about each build tool.
//!
//! Adding a toolchain means adding one type that implements [`Toolchain`] and
//! listing it in [`registry`]; client, server and UI pick it up from there.

use std::path::Path;

/// What the server knows when it starts a job.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// `fifo:/path`, shared by every job.
    pub jobserver_auth: String,
    /// Tokens the server holds for this job (`tokens()`, capped at pool size).
    pub held: usize,
    /// Tokens free in the pool when the job starts.
    pub free_tokens: usize,
    /// Per-toolchain cache root on the server; empty in local mode.
    pub cache_dir: String,
    /// Workspace id; empty in local mode.
    pub workspace_id: String,
    /// Default parallelism of test runners that do not speak the jobserver
    /// protocol (libtest, nextest, Go test binaries); 0 means the free tokens.
    pub test_threads: usize,
}

impl Context {
    fn test_threads(&self) -> usize {
        if self.test_threads > 0 {
            self.test_threads
        } else {
            self.free_tokens.max(2)
        }
    }
}

/// Sets each of `keys` to `n` unless the caller already set it.
fn default_threads(mut env: Vec<String>, keys: &[&str], n: usize) -> Vec<String> {
    for k in keys {
        if get(&env, k).is_none() {
            env.push(format!("{k}={n}"));
        }
    }
    env
}

/// Test-runner variables slot sets when the caller has not.
pub const RUST_TEST_VARS: &[&str] = &["RUST_TEST_THREADS", "NEXTEST_TEST_THREADS"];

/// One build tool family.
pub trait Toolchain: Send + Sync {
    /// Label in the UI and the job log.
    fn name(&self) -> &'static str;
    /// Whether `argv` belongs to this toolchain.
    fn matches(&self, argv: &[String]) -> bool;
    /// Jobserver tokens the server takes before starting the job.
    fn tokens(&self) -> usize;
    /// `env` with the toolchain's settings applied.
    fn env(&self, c: &Context, env: Vec<String>) -> Vec<String>;
    /// Whether a client variable is forwarded to a remote server.
    fn client_env(&self, name: &str) -> bool;
    /// rsync excludes, relative to the repository root.
    fn sync_excludes(&self) -> &'static [&'static str];
}

/// Variables the server owns: client values would bypass the pool.
pub const STRIPPED: &[&str] = &["MAKEFLAGS", "MFLAGS", "CARGO_MAKEFLAGS", "CARGO_BUILD_JOBS"];

/// The registered toolchains, checked in order; [`Generic`] matches anything.
pub fn registry(go_parallel: usize) -> Vec<Box<dyn Toolchain>> {
    vec![
        Box::new(Cargo),
        Box::new(Go {
            parallel: go_parallel,
        }),
        Box::new(NodeTest {
            workers: NODE_TEST_WORKERS,
        }),
        Box::new(Generic),
    ]
}

/// Names of the registered toolchains, in order.
pub fn names() -> Vec<&'static str> {
    registry(1).iter().map(|t| t.name()).collect()
}

/// The toolchain for `argv`.
pub fn for_argv(argv: &[String], go_parallel: usize) -> Box<dyn Toolchain> {
    let tc = registry(go_parallel)
        .into_iter()
        .find(|t| t.matches(argv))
        .unwrap_or_else(|| Box::new(Generic));
    if tc.name() == NodeTest::NAME {
        return Box::new(NodeTest::from_argv(argv));
    }
    tc
}

fn base(argv: &[String]) -> &str {
    argv.first()
        .and_then(|a| Path::new(a).file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("")
}

/// Last value of `key` in `env`.
pub fn get<'a>(env: &'a [String], key: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find_map(|e| e.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// `env` with every `key` removed and `key=value` appended.
pub fn set(env: Vec<String>, key: &str, value: &str) -> Vec<String> {
    let mut out = unset(env, &[key]);
    out.push(format!("{key}={value}"));
    out
}

/// `env` without `keys`.
pub fn unset(env: Vec<String>, keys: &[&str]) -> Vec<String> {
    env.into_iter()
        .filter(|e| {
            !keys
                .iter()
                .any(|k| e.strip_prefix(k).is_some_and(|r| r.starts_with('=')))
        })
        .collect()
}

fn with_jobserver(c: &Context, env: Vec<String>) -> Vec<String> {
    let mut env = unset(env, STRIPPED);
    let flags = format!("-j --jobserver-auth={}", c.jobserver_auth);
    env.push(format!("CARGO_MAKEFLAGS={flags}"));
    env.push(format!("MAKEFLAGS={flags}"));
    env
}

fn common_client_env(name: &str) -> bool {
    matches!(
        name,
        "TERM" | "NO_COLOR" | "CLICOLOR" | "CLICOLOR_FORCE" | "TZ" | "LANG" | "LC_ALL"
    )
}

/// cargo, rustc and cargo subcommands. cargo is a jobserver client: the
/// server takes the implicit token, cargo and rustc draw the rest.
pub struct Cargo;

impl Toolchain for Cargo {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn matches(&self, argv: &[String]) -> bool {
        let b = base(argv);
        b == "cargo" || b == "rustc" || b.starts_with("cargo-")
    }

    fn tokens(&self) -> usize {
        1
    }

    fn env(&self, c: &Context, env: Vec<String>) -> Vec<String> {
        let mut env = with_jobserver(c, env);
        if !c.cache_dir.is_empty() && !c.workspace_id.is_empty() {
            let dir = Path::new(&c.cache_dir).join("target").join(&c.workspace_id);
            env = set(env, "CARGO_TARGET_DIR", &dir.to_string_lossy());
        }
        default_threads(env, RUST_TEST_VARS, c.test_threads())
    }

    fn client_env(&self, name: &str) -> bool {
        if matches!(name, "CARGO_TARGET_DIR" | "CARGO_HOME" | "CARGO_BUILD_JOBS") {
            return false;
        }
        ["CARGO_", "RUST", "NEXTEST_", "INSTA_", "SQLX_OFFLINE"]
            .iter()
            .any(|p| name.starts_with(p))
            || common_client_env(name)
    }

    fn sync_excludes(&self) -> &'static [&'static str] {
        &["/target/"]
    }
}

/// The go command. Not a jobserver client: the server takes `parallel`
/// tokens up front and caps the go command's package parallelism to match.
pub struct Go {
    /// Tokens a go job holds.
    pub parallel: usize,
}

impl Toolchain for Go {
    fn name(&self) -> &'static str {
        "go"
    }

    fn matches(&self, argv: &[String]) -> bool {
        matches!(base(argv), "go" | "gotestsum" | "golangci-lint")
    }

    fn tokens(&self) -> usize {
        self.parallel.max(1)
    }

    fn env(&self, c: &Context, env: Vec<String>) -> Vec<String> {
        let env = unset(env, STRIPPED);
        let n = if c.held > 0 { c.held } else { self.tokens() };
        let flags = match get(&env, "GOFLAGS") {
            Some(v) if !v.is_empty() => format!("{v} -p={n}"),
            _ => format!("-p={n}"),
        };
        let mut env = set(env, "GOFLAGS", &flags);
        if !c.cache_dir.is_empty() {
            let dir = Path::new(&c.cache_dir).join("go-build");
            env = set(env, "GOCACHE", &dir.to_string_lossy());
        }
        default_threads(env, &["GOMAXPROCS"], n)
    }

    fn client_env(&self, name: &str) -> bool {
        if matches!(
            name,
            "GOCACHE" | "GOPATH" | "GOROOT" | "GOMODCACHE" | "GOTMPDIR"
        ) {
            return false;
        }
        name.starts_with("GO") || name == "CGO_ENABLED" || common_client_env(name)
    }

    fn sync_excludes(&self) -> &'static [&'static str] {
        &[]
    }
}

/// Slots a JavaScript test run holds when its command names no worker count.
pub const NODE_TEST_WORKERS: usize = 4;

/// JavaScript and browser test runners: `playwright test`, vitest, jest,
/// also behind npx, yarn, pnpm or bunx. They start about one worker per core
/// unless told otherwise and do not speak the jobserver protocol, so the job
/// holds one slot per worker: the count from `--workers`/`-j` (playwright) or
/// `--maxWorkers`/`--max-workers` (vitest, jest), `N` or `N%` of the cores,
/// else [`NODE_TEST_WORKERS`].
pub struct NodeTest {
    /// Slots the job holds.
    pub workers: usize,
}

impl NodeTest {
    /// Toolchain name.
    pub const NAME: &'static str = "node-test";

    /// The runner `argv` starts, if any.
    pub fn runner(argv: &[String]) -> Option<&'static str> {
        for (i, a) in argv.iter().take(5).enumerate() {
            match Path::new(a).file_name().and_then(|s| s.to_str()) {
                Some("vitest") => return Some("vitest"),
                Some("jest") => return Some("jest"),
                Some("playwright") if argv.get(i + 1).is_some_and(|n| n == "test") => {
                    return Some("playwright");
                }
                _ => {}
            }
        }
        None
    }

    /// The toolchain for one command line.
    pub fn from_argv(argv: &[String]) -> Self {
        let cores = crate::procstat::host_cpus();
        Self {
            workers: worker_flag(argv, cores).unwrap_or(NODE_TEST_WORKERS),
        }
    }
}

/// Worker count named on a test runner's command line.
fn worker_flag(argv: &[String], cores: usize) -> Option<usize> {
    const FLAGS: &[&str] = &["--workers", "-j", "--maxWorkers", "--max-workers"];
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            return None;
        }
        let value = match a.split_once('=') {
            Some((k, v)) if FLAGS.contains(&k) => v.to_string(),
            _ if FLAGS.contains(&a.as_str()) => it.next()?.clone(),
            _ => continue,
        };
        return count(&value, cores);
    }
    None
}

fn count(v: &str, cores: usize) -> Option<usize> {
    if let Some(p) = v.strip_suffix('%') {
        let p: f64 = p.parse().ok()?;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        return Some(((cores as f64 * p / 100.0).floor() as usize).max(1));
    }
    v.parse().ok().filter(|n| *n > 0)
}

impl Toolchain for NodeTest {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn matches(&self, argv: &[String]) -> bool {
        Self::runner(argv).is_some()
    }

    fn tokens(&self) -> usize {
        self.workers.max(1)
    }

    fn env(&self, _c: &Context, env: Vec<String>) -> Vec<String> {
        unset(env, STRIPPED)
    }

    fn client_env(&self, name: &str) -> bool {
        matches!(name, "CI" | "NODE_ENV" | "NODE_OPTIONS")
            || ["PLAYWRIGHT_", "VITEST_", "JEST_"]
                .iter()
                .any(|p| name.starts_with(p))
            || common_client_env(name)
    }

    fn sync_excludes(&self) -> &'static [&'static str] {
        &["/node_modules/"]
    }
}

/// Any other command (task, make, just). Holds one token and passes the
/// jobserver down, so cargo started inside it shares the pool.
pub struct Generic;

impl Toolchain for Generic {
    fn name(&self) -> &'static str {
        "other"
    }

    fn matches(&self, _argv: &[String]) -> bool {
        true
    }

    fn tokens(&self) -> usize {
        1
    }

    fn env(&self, c: &Context, env: Vec<String>) -> Vec<String> {
        let mut env = with_jobserver(c, env);
        if !c.cache_dir.is_empty() && !c.workspace_id.is_empty() {
            let dir = Path::new(&c.cache_dir).join("target").join(&c.workspace_id);
            env = set(env, "CARGO_TARGET_DIR", &dir.to_string_lossy());
            let go = Path::new(&c.cache_dir).join("go-build");
            env = set(env, "GOCACHE", &go.to_string_lossy());
        }
        let n = c.test_threads();
        let env = default_threads(env, RUST_TEST_VARS, n);
        default_threads(env, &["GOMAXPROCS"], n)
    }

    fn client_env(&self, name: &str) -> bool {
        Cargo.client_env(name) || Go { parallel: 1 }.client_env(name)
    }

    fn sync_excludes(&self) -> &'static [&'static str] {
        &["/target/"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn picks_toolchain_by_command() {
        for (cmd, want) in [
            ("cargo test -p x", "rust"),
            ("/usr/bin/cargo build", "rust"),
            ("cargo-nextest run", "rust"),
            ("/opt/rust/bin/rustc", "rust"),
            ("go test ./...", "go"),
            ("golangci-lint run", "go"),
            ("task check", "other"),
            ("make -j8", "other"),
            ("gofmt -l .", "other"),
            ("cargofake build", "other"),
            ("yarn vitest run --maxWorkers=4", "node-test"),
            ("npx playwright test -c a.ts", "node-test"),
            ("pnpm exec jest", "node-test"),
            ("/repo/node_modules/.bin/vitest", "node-test"),
            ("npx playwright install chromium", "other"),
            ("yarn typecheck", "other"),
        ] {
            assert_eq!(for_argv(&argv(cmd), 4).name(), want, "{cmd}");
        }
    }

    #[test]
    fn node_test_holds_one_slot_per_worker() {
        for (cmd, want) in [
            ("yarn vitest run --maxWorkers=3 a.test.ts", 3),
            ("yarn vitest run --maxWorkers 6", 6),
            ("npx playwright test --workers=2", 2),
            ("npx playwright test -j 5", 5),
            ("npx jest --max-workers=50%", 10),
            ("npx playwright test", NODE_TEST_WORKERS),
            ("npx playwright test -- --workers=9", NODE_TEST_WORKERS),
            ("yarn vitest run --maxWorkers=0", NODE_TEST_WORKERS),
            ("yarn vitest run --maxWorkers", NODE_TEST_WORKERS),
        ] {
            let a = argv(cmd);
            let got = worker_flag(&a, 20).unwrap_or(NODE_TEST_WORKERS);
            assert_eq!(got, want, "{cmd}");
        }
        assert_eq!(NodeTest { workers: 0 }.tokens(), 1);
    }

    #[test]
    fn test_runners_get_a_default_thread_count() {
        let c = Context {
            jobserver_auth: "fifo:/p".into(),
            held: 4,
            free_tokens: 18,
            test_threads: 5,
            ..Context::default()
        };
        let cargo = Cargo.env(&c, v(&["RUST_TEST_THREADS=1"]));
        assert!(
            cargo.contains(&"RUST_TEST_THREADS=1".to_string()),
            "{cargo:?}"
        );
        assert!(
            cargo.contains(&"NEXTEST_TEST_THREADS=5".to_string()),
            "{cargo:?}"
        );
        let generic = Generic.env(&c, v(&["PATH=/bin"]));
        for want in [
            "RUST_TEST_THREADS=5",
            "NEXTEST_TEST_THREADS=5",
            "GOMAXPROCS=5",
        ] {
            assert!(
                generic.contains(&want.to_string()),
                "missing {want} in {generic:?}"
            );
        }
        let go = Go { parallel: 4 }.env(&c, v(&[]));
        assert!(go.contains(&"GOMAXPROCS=4".to_string()), "{go:?}");
        let go = Go { parallel: 4 }.env(&c, v(&["GOMAXPROCS=2"]));
        assert!(
            go.contains(&"GOMAXPROCS=2".to_string()) && !go.contains(&"GOMAXPROCS=4".to_string())
        );
    }

    #[test]
    fn cargo_env_owns_jobserver() {
        let c = Context {
            jobserver_auth: "fifo:/p".into(),
            free_tokens: 7,
            cache_dir: "/c".into(),
            workspace_id: "w".into(),
            ..Context::default()
        };
        let got = Cargo.env(
            &c,
            v(&[
                "CARGO_BUILD_JOBS=4",
                "MAKEFLAGS=-j8",
                "PATH=/bin",
                "RUST_TEST_THREADS=3",
            ]),
        );
        for bad in ["CARGO_BUILD_JOBS=4", "MAKEFLAGS=-j8"] {
            assert!(!got.contains(&bad.to_string()), "kept {bad}");
        }
        for want in [
            "CARGO_MAKEFLAGS=-j --jobserver-auth=fifo:/p",
            "MAKEFLAGS=-j --jobserver-auth=fifo:/p",
            "CARGO_TARGET_DIR=/c/target/w",
            "RUST_TEST_THREADS=3",
            "NEXTEST_TEST_THREADS=7",
            "PATH=/bin",
        ] {
            assert!(got.contains(&want.to_string()), "missing {want} in {got:?}");
        }
    }

    #[test]
    fn local_mode_keeps_target_dir() {
        let c = Context {
            jobserver_auth: "fifo:/p".into(),
            ..Context::default()
        };
        let got = Cargo.env(&c, v(&["CARGO_TARGET_DIR=/mine"]));
        assert!(got.contains(&"CARGO_TARGET_DIR=/mine".to_string()));
    }

    #[test]
    fn go_parallelism_follows_held_tokens() {
        let c = Context {
            cache_dir: "/c".into(),
            held: 2,
            ..Context::default()
        };
        let got = Go { parallel: 4 }.env(&c, v(&["GOFLAGS=-mod=mod", "MAKEFLAGS=x"]));
        assert!(
            got.contains(&"GOFLAGS=-mod=mod -p=2".to_string()),
            "{got:?}"
        );
        assert!(got.contains(&"GOCACHE=/c/go-build".to_string()));
        assert!(!got.contains(&"MAKEFLAGS=x".to_string()));
    }

    #[test]
    fn client_env_is_an_allowlist() {
        for name in [
            "AWS_SECRET_ACCESS_KEY",
            "GITLAB_TOKEN",
            "HOME",
            "CARGO_TARGET_DIR",
            "GOCACHE",
            "CI_JOB_TOKEN",
        ] {
            for tc in registry(4) {
                assert!(!tc.client_env(name), "{} forwards {name}", tc.name());
            }
        }
        assert!(Cargo.client_env("RUSTFLAGS"));
        assert!(Go { parallel: 1 }.client_env("GOFLAGS"));
    }
}
