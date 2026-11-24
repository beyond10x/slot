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
    /// The job's width on a node ([`crate::width`]); `None` for a local
    /// job, which keeps `test_threads`.
    pub width: Option<usize>,
}

impl Context {
    fn test_threads(&self) -> usize {
        if let Some(w) = self.width {
            w
        } else if self.test_threads > 0 {
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

/// Sets each of `keys` to `n` unless the caller set it; for a job with a
/// width (on a node) a caller's count below `n` is raised to `n`. A higher
/// count, or one that is not a number, stays.
fn threads(c: &Context, env: Vec<String>, keys: &[&str], n: usize) -> Vec<String> {
    let mut env = default_threads(env, keys, n);
    if c.width.is_some() {
        for k in keys {
            let have = get(&env, k).and_then(|v| v.trim().parse::<usize>().ok());
            if have.is_some_and(|have| have < n) {
                env = set(env, k, &n.to_string());
            }
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
        Box::new(Ignite { tokens: 1 }),
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
    if tc.name() == Ignite::NAME {
        return Box::new(Ignite::from_argv(argv));
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

/// `env` of a job whose build processes are bounded per job (`slot
/// job-parallel`, [`crate::job_parallel`]), in place of the pool's
/// jobserver: none of [`STRIPPED`] as a toolchain or the client set them,
/// then `CARGO_BUILD_JOBS=<p>` and `MAKEFLAGS=-j<p>`. With no
/// `--jobserver-auth` in its environment `cargo` makes a jobserver of its
/// own with `p` tokens and hands it to rustc and to build scripts in
/// `CARGO_MAKEFLAGS`; `make` runs `p` recipes at once. Applied to the
/// environment of every toolchain, so a `cargo` or `make` started inside a
/// script, a test runner or the go command is bounded too.
pub fn own_jobserver(env: Vec<String>, p: usize) -> Vec<String> {
    let mut env = unset(env, STRIPPED);
    env.push(format!("CARGO_BUILD_JOBS={p}"));
    env.push(format!("MAKEFLAGS=-j{p}"));
    env
}

/// Parts of a name that make it look like a credential, anywhere in it.
pub const CREDENTIAL_PARTS: &[&str] = &[
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "PASS",
    "CREDENTIAL",
    "_KEY",
    "AUTH",
    "SSH_",
    "AWS_SECRET",
];

/// Words of a name (between `_`) that make it look like a credential:
/// `NPM_PAT`, `SENTRY_DSN`, `KEY`, not `PKG_CONFIG_PATH`.
pub const CREDENTIAL_WORDS: &[&str] = &["PAT", "DSN", "KEY"];

/// Words that hold a part of [`CREDENTIAL_PARTS`] and name no credential:
/// `CARGO_PKG_AUTHORS`, which cargo sets in every build script and test.
const NOT_CREDENTIAL_WORDS: &[&str] = &["AUTHOR", "AUTHORS"];

/// Whether a variable name looks like a credential (`CARGO_REGISTRY_TOKEN`,
/// `*_SECRET`, `DB_PASS`, `*_KEY`, `SSH_AUTH_SOCK`, `NPM_PAT`, `*_DSN`).
/// Such a variable never crosses to a remote host, through a toolchain's
/// prefix rule or named in `SLOT_ENV`.
pub fn credential(name: &str) -> bool {
    let n = name
        .to_ascii_uppercase()
        .split('_')
        .map(|w| {
            if NOT_CREDENTIAL_WORDS.contains(&w) {
                ""
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join("_");
    CREDENTIAL_PARTS.iter().any(|w| n.contains(w))
        || n.split('_').any(|w| CREDENTIAL_WORDS.contains(&w))
}

/// What of `vars` a remote job gets: the names `admits` (a toolchain's
/// `client_env`) and the names in `listed` (`SLOT_ENV`), except those that
/// look like a credential. The second value: the names held back for that,
/// sorted, for the caller to name (never their values).
pub fn remote_env(
    vars: impl IntoIterator<Item = (String, String)>,
    admits: impl Fn(&str) -> bool,
    listed: impl Fn(&str) -> bool,
) -> (Vec<String>, Vec<String>) {
    let mut sent = Vec::new();
    let mut held = Vec::new();
    for (k, v) in vars {
        if !(admits(&k) || listed(&k)) {
            continue;
        }
        if credential(&k) {
            held.push(k);
        } else {
            sent.push(format!("{k}={v}"));
        }
    }
    held.sort();
    held.dedup();
    (sent, held)
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
        threads(c, env, RUST_TEST_VARS, c.test_threads())
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
/// tokens up front and caps the go command's package parallelism to match;
/// on a node `-p` and `GOMAXPROCS` follow the job's width instead.
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
        let held = if c.held > 0 { c.held } else { self.tokens() };
        // On a node the job's width, if more than the slots it holds.
        let n = c.width.map_or(held, |w| w.max(held));
        let flags = match get(&env, "GOFLAGS") {
            Some(v) if !v.is_empty() => format!("{v} -p={n}"),
            _ => format!("-p={n}"),
        };
        let mut env = set(env, "GOFLAGS", &flags);
        if !c.cache_dir.is_empty() {
            let dir = Path::new(&c.cache_dir).join("go-build");
            env = set(env, "GOCACHE", &dir.to_string_lossy());
        }
        threads(c, env, &["GOMAXPROCS"], n)
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

/// The Ignite compiler: `ignite build`, `ignite test`, `ignite conformance`.
/// Its LLVM target runs `clang` once per compile and link, which reads no
/// jobserver; its Rust target runs `cargo`, which gets the pool's jobserver
/// from this job. `ignite test` and `ignite conformance` run the built tests
/// `--jobs` at a time, the CPU count by default, so the job holds one slot
/// per test job: `--jobs N`, else [`IGNITE_TEST_JOBS`], and the job's
/// environment sets `IGNITE_TEST_JOBS` (read by ignite 0.16.3 and later) to
/// the slots it holds. Every other subcommand holds one slot.
pub struct Ignite {
    /// Slots the job holds.
    pub tokens: usize,
}

/// Test jobs an `ignite test` without `--jobs` is given.
pub const IGNITE_TEST_JOBS: usize = 4;

impl Ignite {
    /// Toolchain name.
    pub const NAME: &'static str = "ignite";

    /// The subcommand of an `ignite` command line, if `argv` starts `ignite`.
    pub fn subcommand(argv: &[String]) -> Option<&str> {
        if base(argv) != "ignite" {
            return None;
        }
        Some(
            argv[1..]
                .iter()
                .map(String::as_str)
                .find(|a| !a.starts_with('-'))
                .unwrap_or(""),
        )
    }

    /// The toolchain for one command line.
    pub fn from_argv(argv: &[String]) -> Self {
        let tokens = match Self::subcommand(argv) {
            Some("test" | "conformance") => ignite_jobs(argv).unwrap_or(IGNITE_TEST_JOBS),
            _ => 1,
        };
        Self { tokens }
    }
}

/// `--jobs N` or `--jobs=N` on an `ignite test` command line.
fn ignite_jobs(argv: &[String]) -> Option<usize> {
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            return None;
        }
        let value = match a.split_once('=') {
            Some(("--jobs", v)) => v.to_string(),
            _ if a == "--jobs" => it.next()?.clone(),
            _ => continue,
        };
        return value.parse().ok().filter(|n| *n > 0);
    }
    None
}

impl Toolchain for Ignite {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn matches(&self, argv: &[String]) -> bool {
        Self::subcommand(argv).is_some()
    }

    fn tokens(&self) -> usize {
        self.tokens.max(1)
    }

    fn env(&self, c: &Context, env: Vec<String>) -> Vec<String> {
        let env = Generic.env(c, env);
        default_threads(env, &["IGNITE_TEST_JOBS"], c.held.max(1))
    }

    fn client_env(&self, name: &str) -> bool {
        name.starts_with("IGNITE_") || Cargo.client_env(name)
    }

    fn sync_excludes(&self) -> &'static [&'static str] {
        &["/target/"]
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
        let env = threads(c, env, RUST_TEST_VARS, n);
        threads(c, env, &["GOMAXPROCS"], n)
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
            ("ignite build x.ignite --target rust", "ignite"),
            ("/opt/ignite/bin/ignite test a.ignite --jobs 3", "ignite"),
            ("ignite-load run", "other"),
        ] {
            assert_eq!(for_argv(&argv(cmd), 4).name(), want, "{cmd}");
        }
    }

    #[test]
    fn ignite_test_holds_one_slot_per_test_job() {
        for (cmd, want) in [
            ("ignite build a.ignite --release", 1),
            ("ignite check a.ignite", 1),
            ("ignite test a.ignite", IGNITE_TEST_JOBS),
            ("ignite test a.ignite --jobs 6", 6),
            ("ignite test --jobs=2 a.ignite", 2),
            ("ignite conformance a.ignite --jobs 0", IGNITE_TEST_JOBS),
            ("ignite test a.ignite -- --jobs 9", IGNITE_TEST_JOBS),
        ] {
            assert_eq!(for_argv(&argv(cmd), 4).tokens(), want, "{cmd}");
        }
        let c = Context {
            jobserver_auth: "fifo:/p".into(),
            held: 3,
            free_tokens: 10,
            test_threads: 5,
            ..Context::default()
        };
        let env = Ignite { tokens: 3 }.env(&c, v(&["MAKEFLAGS=-j8"]));
        assert!(env.contains(&"IGNITE_TEST_JOBS=3".to_string()), "{env:?}");
        assert!(
            env.contains(&"CARGO_MAKEFLAGS=-j --jobserver-auth=fifo:/p".to_string()),
            "{env:?}"
        );
        assert!(!env.contains(&"MAKEFLAGS=-j8".to_string()), "{env:?}");
        let mine = Ignite { tokens: 3 }.env(&c, v(&["IGNITE_TEST_JOBS=1"]));
        assert_eq!(get(&mine, "IGNITE_TEST_JOBS"), Some("1"));
        assert!(Ignite { tokens: 1 }.client_env("IGNITE_STD"));
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
    fn a_job_on_a_node_gets_its_width_and_a_lower_caller_count_is_raised() {
        let node = Context {
            jobserver_auth: "fifo:/p".into(),
            held: 1,
            free_tokens: 60,
            test_threads: 16,
            width: Some(40),
            ..Context::default()
        };
        let workstation = Context {
            width: None,
            ..node.clone()
        };
        let caller = v(&["NEXTEST_TEST_THREADS=4", "GOMAXPROCS=64"]);
        let cargo = Cargo.env(&node, caller.clone());
        assert_eq!(get(&cargo, "RUST_TEST_THREADS"), Some("40"), "{cargo:?}");
        assert_eq!(get(&cargo, "NEXTEST_TEST_THREADS"), Some("40"), "raised");
        let generic = Generic.env(&node, caller.clone());
        assert_eq!(get(&generic, "NEXTEST_TEST_THREADS"), Some("40"));
        assert_eq!(
            get(&generic, "GOMAXPROCS"),
            Some("64"),
            "a higher count stays"
        );
        let go = Go { parallel: 4 }.env(&node, v(&["GOMAXPROCS=2"]));
        assert_eq!(get(&go, "GOMAXPROCS"), Some("40"), "{go:?}");
        assert_eq!(get(&go, "GOFLAGS"), Some("-p=40"), "{go:?}");

        let cargo = Cargo.env(&workstation, caller);
        assert_eq!(get(&cargo, "RUST_TEST_THREADS"), Some("16"), "{cargo:?}");
        assert_eq!(get(&cargo, "NEXTEST_TEST_THREADS"), Some("4"), "kept");
        let go = Go { parallel: 4 }.env(&workstation, v(&["GOMAXPROCS=2"]));
        assert_eq!(get(&go, "GOMAXPROCS"), Some("2"));
        assert_eq!(get(&go, "GOFLAGS"), Some("-p=1"), "the slots it holds");
    }

    #[test]
    fn a_job_with_its_own_bound_gets_no_pool_jobserver_under_any_toolchain() {
        let c = Context {
            jobserver_auth: "fifo:/p".into(),
            held: 1,
            free_tokens: 7,
            ..Context::default()
        };
        let client = [
            "CARGO_BUILD_JOBS=9",
            "MAKEFLAGS=-j8",
            "MFLAGS=-j8",
            "CARGO_MAKEFLAGS=-j --jobserver-auth=fifo:/theirs",
            "PATH=/bin",
            "RUST_TEST_THREADS=3",
        ];
        for tc in registry(4) {
            let name = tc.name();
            let pool = tc.env(&c, v(&client));
            let own = own_jobserver(pool.clone(), 4);
            for want in ["CARGO_BUILD_JOBS=4", "MAKEFLAGS=-j4"] {
                assert_eq!(
                    own.iter().filter(|e| *e == want).count(),
                    1,
                    "{name}: {want} once in {own:?}"
                );
            }
            for key in ["CARGO_MAKEFLAGS", "MFLAGS"] {
                assert_eq!(get(&own, key), None, "{name}: {key} in {own:?}");
            }
            assert_eq!(get(&own, "CARGO_BUILD_JOBS"), Some("4"), "{name}");
            assert_eq!(get(&own, "MAKEFLAGS"), Some("-j4"), "{name}");
            assert!(
                !own.iter().any(|e| e.contains("jobserver-auth")),
                "{name}: the pool's FIFO is named in {own:?}"
            );
            // Everything else a toolchain sets stays as it is.
            let rest = |env: &[String]| unset(env.to_vec(), STRIPPED);
            assert_eq!(rest(&own), rest(&pool), "{name}");
        }
        let go = own_jobserver(Go { parallel: 4 }.env(&c, v(&[])), 6);
        assert!(go.contains(&"GOFLAGS=-p=1".to_string()), "{go:?}");
        assert!(go.contains(&"CARGO_BUILD_JOBS=6".to_string()), "{go:?}");
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

    #[test]
    fn credentials_are_held_back_even_under_a_toolchain_prefix() {
        for name in [
            "CARGO_REGISTRY_TOKEN",
            "CARGO_REGISTRIES_PRIV_TOKEN",
            "CARGO_REGISTRIES_PRIV_CREDENTIAL_PROVIDER",
            "GOPRIVATE_AUTH",
            "RUST_SECRET",
            "NEXTEST_API_KEY",
        ] {
            assert!(credential(name), "{name}");
        }
        for name in [
            "RUSTFLAGS",
            "CARGO_PROFILE_DEV_DEBUG",
            "GOFLAGS",
            "CARGO_TERM_COLOR",
        ] {
            assert!(!credential(name), "{name}");
        }
    }

    #[test]
    fn names_that_look_like_a_credential_are_known_by_part_and_by_word() {
        for name in [
            "DB_PASS",
            "PGPASSFILE",
            "SSH_AUTH_SOCK",
            "GIT_SSH_COMMAND",
            "AWS_SECRET_ACCESS_KEY",
            "NPM_PAT",
            "GH_PAT_RO",
            "SENTRY_DSN",
            "DATABASE_DSN_RO",
            "KEY",
            "api_token",
            "HTTP_AUTHORIZATION",
            "OAUTH_CLIENT",
        ] {
            assert!(credential(name), "{name}");
        }
        for name in [
            "PKG_CONFIG_PATH",
            "LD_LIBRARY_PATH",
            "PATTERN",
            "AWS_PROFILE",
            "FEATURE_FLAGS",
            "KEYBOARD_LAYOUT",
            "CARGO_PKG_AUTHORS",
        ] {
            assert!(!credential(name), "{name}");
        }
    }

    #[test]
    fn a_listed_credential_is_held_back_and_named() {
        let vars = [
            ("RUSTFLAGS", "-Dwarnings"),
            ("CARGO_REGISTRY_TOKEN", "t0"),
            ("FEATURE_FLAGS", "on"),
            ("DB_PASS", "p1"),
            ("NPM_PAT", "p2"),
            ("UNLISTED", "u"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()));
        let listed: std::collections::HashSet<String> = ["FEATURE_FLAGS", "DB_PASS", "NPM_PAT"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let (sent, held) = remote_env(vars, |k| Cargo.client_env(k), |k| listed.contains(k));
        assert_eq!(sent, ["RUSTFLAGS=-Dwarnings", "FEATURE_FLAGS=on"]);
        assert_eq!(held, ["CARGO_REGISTRY_TOKEN", "DB_PASS", "NPM_PAT"]);
    }
}
