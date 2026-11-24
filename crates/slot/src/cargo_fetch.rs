//! Fetch once: the crates of a mirror's `Cargo.lock` are downloaded once on
//! the node, and its cargo jobs then run offline.
//!
//! Parallel cargo jobs on one node took turns on cargo's package-cache lock
//! while one of them downloaded, for up to 28 minutes. Before a cargo job of
//! a synced workspace, the daemon compares the digest of the mirror's
//! `Cargo.lock` with the one recorded for that mirror in
//! `<ws-root>/.cargo-fetch/<id>`. When they differ it runs
//! `cargo fetch --locked` there, under a lock per repository, and records
//! the digest. Every such job then runs with `CARGO_NET_OFFLINE=true`, so it
//! takes the package-cache lock only for the moment it reads the cache. A
//! fetch that fails sends the job back to the client, which runs it on its
//! own machine's daemon.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Directory under the workspace root with each mirror's fetched digest and
/// the per-repository locks.
pub const DIR: &str = ".cargo-fetch";

/// Longest a fetch, the wait for another fetch of the repository included,
/// may take before the job goes back to the client.
pub const LIMIT: Duration = Duration::from_secs(300);

/// What runs offline after the fetch.
pub const OFFLINE: &str = "CARGO_NET_OFFLINE=true";

/// Whether `argv` is a cargo command that resolves and builds the
/// workspace, so a fetch ahead of it lets it run offline. `fetch`, `update`,
/// `add`, `install` and the like need the network themselves.
pub fn applies(argv: &[String]) -> bool {
    crate::artifacts::subcommand(argv).is_some_and(|i| {
        matches!(
            argv[i].as_str(),
            "build"
                | "b"
                | "check"
                | "c"
                | "clippy"
                | "test"
                | "t"
                | "bench"
                | "doc"
                | "d"
                | "rustc"
                | "rustdoc"
                | "nextest"
                | "llvm-cov"
        )
    })
}

/// The nearest directory from `cwd` up to `root` that holds a `Cargo.lock`.
pub fn lock_dir(cwd: &Path, root: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .take_while(|d| d.starts_with(root))
        .find(|d| d.join("Cargo.lock").is_file())
        .map(Path::to_path_buf)
}

/// SHA-256 of `file`, hex.
fn digest(file: &Path) -> Option<String> {
    use std::fmt::Write as _;
    let b = std::fs::read(file).ok()?;
    Some(Sha256::digest(&b).iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    }))
}

/// How a fetch-once check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No `Cargo.lock`: the job runs as it would without the check.
    NoLock,
    /// The recorded digest matches: the job runs offline.
    Current,
    /// `cargo fetch --locked` ran this long; the job runs offline.
    Fetched(Duration),
}

/// What one check needs.
#[derive(Debug, Clone)]
pub struct Check<'a> {
    /// The daemon's workspace root.
    pub ws_root: &'a Path,
    /// Workspace id.
    pub id: &'a str,
    /// The job's working directory in the mirror.
    pub cwd: &'a Path,
    /// `cargo` as the job names it, and a `+toolchain` when it has one.
    pub cargo: Vec<String>,
    /// The job's environment.
    pub env: &'a [String],
    /// Bound on the wait for another fetch and the fetch together.
    pub limit: Duration,
}

/// The job's cargo and its `+toolchain`, from `argv`.
pub fn cargo_of(argv: &[String]) -> Vec<String> {
    argv.iter()
        .take(1)
        .chain(argv.iter().skip(1).take(1).filter(|a| a.starts_with('+')))
        .cloned()
        .collect()
}

/// Brings the mirror's crates into the cargo home once per `Cargo.lock`.
/// `Err` with the reason when the fetch fails or runs past the limit.
pub fn ensure(c: &Check<'_>) -> Result<Outcome, String> {
    let root = c.ws_root.join(c.id);
    let Some(dir) = lock_dir(c.cwd, &root) else {
        return Ok(Outcome::NoLock);
    };
    let Some(want) = digest(&dir.join("Cargo.lock")) else {
        return Ok(Outcome::NoLock);
    };
    let store = c.ws_root.join(DIR);
    let record = store.join(c.id);
    let recorded = || {
        std::fs::read_to_string(&record)
            .ok()
            .map(|s| s.trim().to_string())
    };
    if recorded().as_deref() == Some(want.as_str()) {
        return Ok(Outcome::Current);
    }
    let key = std::fs::read_to_string(c.ws_root.join(crate::mirror::KEYS_DIR).join(c.id))
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| crate::mirror::valid_id(k))
        .unwrap_or_else(|| c.id.to_string());
    let t0 = Instant::now();
    let _lock = crate::sync::lock_workspace(&store, "cargo-fetch", &key, c.limit)
        .map_err(|e| format!("cargo fetch --locked failed: {e:#}"))?;
    if recorded().as_deref() == Some(want.as_str()) {
        return Ok(Outcome::Current);
    }
    let (prog, args) = c.cargo.split_first().ok_or("no cargo")?;
    let mut child = Command::new(prog)
        .args(args)
        .args(["fetch", "--locked"])
        .current_dir(&dir)
        .env_clear()
        .envs(c.env.iter().filter_map(|e| e.split_once('=')))
        .env_remove("CARGO_NET_OFFLINE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cargo fetch --locked failed: {prog}: {e}"))?;
    let mut err = child.stderr.take().ok_or("cargo stderr")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let left = c.limit.saturating_sub(t0.elapsed());
    let st = crate::sync::watch(&mut child, None, Instant::now() + left, c.limit);
    let err = reader.join().unwrap_or_default();
    let tail = || {
        let lines: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(3)..].join(" / ")
    };
    match st {
        Ok(st) if st.success() => {}
        Ok(st) => return Err(format!("cargo fetch --locked failed ({st}): {}", tail())),
        Err(stop) => return Err(format!("cargo fetch --locked failed: {stop}")),
    }
    std::fs::create_dir_all(&store).map_err(|e| format!("{}: {e}", store.display()))?;
    let tmp = store.join(format!(".{}.tmp", c.id));
    std::fs::write(&tmp, &want)
        .and_then(|()| std::fs::rename(&tmp, &record))
        .map_err(|e| format!("{}: {e}", record.display()))?;
    Ok(Outcome::Fetched(t0.elapsed()))
}

/// Forgets mirror `id`'s fetched digest, when the mirror is deleted.
pub fn forget(ws_root: &Path, id: &str) {
    if crate::mirror::valid_id(id) {
        let _ = std::fs::remove_file(ws_root.join(DIR).join(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn builds_and_checks_fetch_first_and_network_commands_do_not() {
        for yes in [
            &["cargo", "build"][..],
            &["cargo", "+nightly", "clippy", "--all-targets"],
            &["/usr/bin/cargo", "--locked", "test"],
            &["cargo", "nextest", "run"],
        ] {
            assert!(applies(&v(yes)), "{yes:?}");
        }
        for no in [
            &["cargo", "fetch"][..],
            &["cargo", "update"],
            &["cargo", "install", "x"],
            &["cargo", "run"],
            &["cargo"],
            &["make", "build"],
        ] {
            assert!(!applies(&v(no)), "{no:?}");
        }
        assert_eq!(
            cargo_of(&v(&["cargo", "+1.90", "build"])),
            v(&["cargo", "+1.90"])
        );
        assert_eq!(cargo_of(&v(&["cargo", "build"])), v(&["cargo"]));
    }

    /// A workspace root with mirror `00aa` holding a `Cargo.lock`, and a
    /// `cargo` that logs each call and exits with `code`.
    fn setup(code: i32) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join("00aa/sub")).unwrap();
        std::fs::write(ws.join("00aa/Cargo.lock"), "version = 3\n").unwrap();
        let cargo = d.path().join("cargo");
        std::fs::write(
            &cargo,
            format!(
                "#!/bin/sh\necho \"$CARGO_NET_OFFLINE|$*\" >> {}\necho 'error: no network' >&2\nexit {code}\n",
                d.path().join("log").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
        (d, ws)
    }

    fn check<'a>(ws: &'a Path, cwd: &'a Path, cargo: &Path, env: &'a [String]) -> Check<'a> {
        Check {
            ws_root: ws,
            id: "00aa",
            cwd,
            cargo: vec![cargo.to_string_lossy().into_owned()],
            env,
            limit: Duration::from_secs(10),
        }
    }

    #[test]
    fn one_fetch_per_lock_file_digest() {
        let (d, ws) = setup(0);
        let cwd = ws.join("00aa/sub");
        let env = v(&["PATH=/usr/bin:/bin", "CARGO_NET_OFFLINE=true"]);
        let cargo = d.path().join("cargo");
        let c = check(&ws, &cwd, &cargo, &env);
        assert!(matches!(ensure(&c), Ok(Outcome::Fetched(_))));
        assert_eq!(ensure(&c), Ok(Outcome::Current));
        std::fs::write(ws.join("00aa/Cargo.lock"), "version = 4\n").unwrap();
        assert!(matches!(ensure(&c), Ok(Outcome::Fetched(_))));
        let log = std::fs::read_to_string(d.path().join("log")).unwrap();
        assert_eq!(log, "|fetch --locked\n|fetch --locked\n", "online, twice");
        forget(&ws, "00aa");
        assert!(matches!(ensure(&c), Ok(Outcome::Fetched(_))));
    }

    #[test]
    fn a_failed_fetch_says_why_and_records_nothing() {
        let (d, ws) = setup(101);
        let cwd = ws.join("00aa");
        let cargo = d.path().join("cargo");
        let c = check(&ws, &cwd, &cargo, &[]);
        let e = ensure(&c).unwrap_err();
        assert!(e.starts_with("cargo fetch --locked failed"), "{e}");
        assert!(e.contains("no network"), "{e}");
        assert!(!ws.join(DIR).join("00aa").exists());
    }

    #[test]
    fn a_mirror_without_a_lock_file_is_left_alone() {
        let (d, ws) = setup(0);
        std::fs::remove_file(ws.join("00aa/Cargo.lock")).unwrap();
        let cwd = ws.join("00aa/sub");
        let cargo = d.path().join("cargo");
        assert_eq!(ensure(&check(&ws, &cwd, &cargo, &[])), Ok(Outcome::NoLock));
        assert!(!d.path().join("log").exists());
    }
}
