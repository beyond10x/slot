//! Build products of a remote cargo job, brought back into the local target
//! directory.
//!
//! A session that builds on a node and then runs `target/debug/<bin>` or a
//! test binary needs those files here. The client adds
//! `--message-format=json-render-diagnostics` to a remote `cargo build`,
//! `test`, `bench` or `rustc` that names no message format: cargo then still
//! renders its diagnostics on stderr as usual and prints one JSON line per
//! compiled unit on stdout. The client takes those lines out of the job's
//! stdout, so the caller sees what cargo would have shown, and keeps the
//! paths of the `compiler-artifact` messages of the workspace's own packages
//! (a `path+` package id): every `executable`, and the files of `bin`,
//! `test`, `cdylib` and `staticlib` targets. A caller that asked for a JSON
//! format gets its stream unchanged and the same paths are read from it.
//!
//! After the job (a build after exit 0; a test or bench run whatever its
//! exit) one rsync copies those paths from the node's target directory to the
//! same relative paths under the local one, with modes and mtimes. Artifacts
//! of dependencies, `deps/*.rlib` and the rest of the target directory stay
//! on the node. Then [`prune`] removes the local files the copied ones
//! supersede (the same target under an older hash), so the local target
//! directory does not grow with every rebuild. `SLOT_NO_PRODUCTS=1` turns
//! all of it off for a job ([`NO_PRODUCTS_ENV`]).

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};

/// The message format the client adds when the caller named none.
pub const FORMAT: &str = "--message-format=json-render-diagnostics";

/// Set and not `0` in the caller's environment: nothing comes back, and the
/// command runs on the node as the caller wrote it.
pub const NO_PRODUCTS_ENV: &str = "SLOT_NO_PRODUCTS";

/// Paths one rsync names at most; more go in further calls.
const BATCH: usize = 200;

/// When the products come back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// After exit 0 (`build`, `rustc`).
    Success,
    /// Whatever the exit (`test`, `bench`): a failed run's binaries are the
    /// ones to run again here.
    AnyExit,
}

/// What the client does with a remote cargo job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The command the node runs.
    pub argv: Vec<String>,
    /// Whether the JSON lines are taken out of stdout (the client added
    /// them).
    pub strip: bool,
    /// When the products come back.
    pub when: When,
}

/// Global cargo options that take a value as the next word.
const VALUE_OPTS: &[&str] = &["--color", "--config", "-Z", "-C", "--manifest-path"];

/// Index of cargo's subcommand in `argv` (after `cargo`, a `+toolchain` and
/// global options).
pub fn subcommand(argv: &[String]) -> Option<usize> {
    let name = Path::new(argv.first()?).file_name()?.to_str()?;
    if name != "cargo" {
        return None;
    }
    let mut skip = false;
    for (i, a) in argv.iter().enumerate().skip(1) {
        if skip {
            skip = false;
        } else if VALUE_OPTS.contains(&a.as_str()) {
            skip = true;
        } else if !a.starts_with('+') && !a.starts_with('-') {
            return Some(i);
        }
    }
    None
}

/// The plan for `argv`, or `None` when it is not a cargo command whose
/// products come back: not `build`, `test`, `bench` or `rustc`, or a
/// caller's `--message-format` that is not JSON (`human`, `short`), which
/// hides the artifact messages.
pub fn plan(argv: &[String]) -> Option<Plan> {
    let i = subcommand(argv)?;
    let when = match argv[i].as_str() {
        "build" | "b" | "rustc" => When::Success,
        "test" | "t" | "bench" => When::AnyExit,
        _ => return None,
    };
    let mut formats = Vec::new();
    let mut it = argv[i + 1..].iter().take_while(|a| *a != "--");
    while let Some(a) = it.next() {
        if a == "--message-format" {
            formats.extend(it.next().cloned());
        } else if let Some(v) = a.strip_prefix("--message-format=") {
            formats.push(v.to_string());
        }
    }
    if formats.is_empty() {
        let mut out = argv.to_vec();
        out.insert(i + 1, FORMAT.to_string());
        return Some(Plan {
            argv: out,
            strip: true,
            when,
        });
    }
    formats
        .iter()
        .flat_map(|f| f.split(','))
        .any(|f| f.trim().starts_with("json"))
        .then(|| Plan {
            argv: argv.to_vec(),
            strip: false,
            when,
        })
}

/// Reads a job's stdout: passes everything on but cargo's JSON lines (when
/// it strips them) and keeps the product paths those lines name.
#[derive(Debug, Default)]
pub struct Collector {
    strip: bool,
    buf: Vec<u8>,
    paths: Vec<String>,
}

impl Collector {
    /// A collector that takes the JSON lines out of the output when `strip`.
    pub fn new(strip: bool) -> Self {
        Self {
            strip,
            ..Self::default()
        }
    }

    /// Feeds output; returns what the caller sees now.
    pub fn push(&mut self, b: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(b);
        let Some(cut) = self.buf.iter().rposition(|&c| c == b'\n') else {
            return Vec::new();
        };
        let rest = self.buf.split_off(cut + 1);
        let lines = std::mem::replace(&mut self.buf, rest);
        let mut out = Vec::with_capacity(lines.len());
        for l in lines.split_inclusive(|&c| c == b'\n') {
            if self.line(l) {
                out.extend_from_slice(l);
            }
        }
        out
    }

    /// The rest of the output, after the job's last frame.
    pub fn finish(&mut self) -> Vec<u8> {
        let l = std::mem::take(&mut self.buf);
        if l.is_empty() || !self.line(&l) {
            Vec::new()
        } else {
            l
        }
    }

    /// Product paths on the node, in the order cargo reported them, each
    /// once.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// Reads one line; whether the caller sees it.
    fn line(&mut self, l: &[u8]) -> bool {
        if !l.starts_with(b"{\"reason\":") {
            return true;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(l) else {
            return true;
        };
        if v.get("reason").and_then(|r| r.as_str()) == Some("compiler-artifact") {
            for p in products(&v) {
                if !self.paths.contains(&p) {
                    self.paths.push(p);
                }
            }
        }
        !self.strip
    }
}

/// Whether a cargo package id names a package of the workspace itself (a
/// path source), in the format of cargo 1.77 and later
/// (`path+file:///…#name@1.0.0`) or before (`name 1.0.0 (path+file:///…)`).
fn own_package(id: &str) -> bool {
    id.starts_with("path+") || id.contains("(path+")
}

/// The files of one `compiler-artifact` message that come back.
fn products(v: &serde_json::Value) -> Vec<String> {
    let own = v
        .get("package_id")
        .and_then(|p| p.as_str())
        .is_some_and(own_package);
    if !own {
        return Vec::new();
    }
    if let Some(exe) = v.get("executable").and_then(|e| e.as_str()) {
        return vec![exe.to_string()];
    }
    let kinds: Vec<&str> = v
        .pointer("/target/kind")
        .and_then(|k| k.as_array())
        .map(|k| k.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    if !kinds
        .iter()
        .any(|k| matches!(*k, "bin" | "test" | "cdylib" | "staticlib"))
    {
        return Vec::new();
    }
    v.get("filenames")
        .and_then(|f| f.as_array())
        .map(|f| {
            f.iter()
                .filter_map(|x| x.as_str())
                .filter(|p| {
                    !Path::new(p)
                        .extension()
                        .is_some_and(|e| e == "rlib" || e == "rmeta" || e == "d")
                })
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// `path` relative to the node's target directory `target`, or to the
/// fixed path a sandboxed job sees it at. `None` for a path outside both or
/// one with `.` or `..`.
pub fn relative(path: &str, target: &str) -> Option<String> {
    let roots = [target, crate::sandbox::FIXED_TARGET];
    let rest = roots.iter().filter(|r| !r.is_empty()).find_map(|r| {
        path.strip_prefix(r.trim_end_matches('/'))?
            .strip_prefix('/')
    })?;
    let p = Path::new(rest);
    (!rest.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_))))
        .then(|| rest.to_string())
}

/// The local target directory for a cargo job run from `cwd` in the
/// repository at `top`: `CARGO_TARGET_DIR` (`target_env`, relative to
/// `cwd`), else `target` beside the nearest `Cargo.lock` from `cwd` up to
/// `top`, else beside the nearest `Cargo.toml` with a `[workspace]`, else
/// beside the nearest `Cargo.toml`, else under `top`.
pub fn local_target(cwd: &Path, top: &Path, target_env: Option<&str>) -> PathBuf {
    if let Some(t) = target_env.filter(|t| !t.is_empty()) {
        return cwd.join(t);
    }
    let ups: Vec<&Path> = cwd.ancestors().take_while(|d| d.starts_with(top)).collect();
    let workspace = |d: &&&Path| {
        std::fs::read_to_string(d.join("Cargo.toml"))
            .is_ok_and(|s| s.lines().any(|l| l.trim() == "[workspace]"))
    };
    ups.iter()
        .find(|d| d.join("Cargo.lock").is_file())
        .or_else(|| ups.iter().find(workspace))
        .or_else(|| ups.iter().find(|d| d.join("Cargo.toml").is_file()))
        .map_or_else(|| top.join("target"), |d| d.join("target"))
}

/// What [`prune`] removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pruned {
    /// Files removed.
    pub files: usize,
    /// Their bytes.
    pub bytes: u64,
}

/// Digits in the hash cargo puts in a unit's file name.
const HASH_LEN: usize = 16;

/// `name` read as `<stem>-<16 lowercase hex>`, optionally followed by an
/// extension (`.d`, `.dwp`, `.pdb`, …): the stem and the hash. Cargo writes
/// `-` in a target's name as `_`, so the first `-` that such a hash follows
/// ends the stem.
fn hashed(name: &str) -> Option<(&str, &str)> {
    let b = name.as_bytes();
    name.match_indices('-').find_map(|(i, _)| {
        let h = b.get(i + 1..i + 1 + HASH_LEN)?;
        let rest = &b[i + 1 + HASH_LEN..];
        (i > 0
            && h.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
            && (rest.is_empty() || rest[0] == b'.'))
            .then(|| (&name[..i], &name[i + 1..i + 1 + HASH_LEN]))
    })
}

/// Removes, beside the products just copied to `local` (`rels`, relative
/// to it), the files they supersede: in each directory a product was copied
/// to, every file named `<stem>-<16 lowercase hex>[.<ext>]` for a stem
/// copied there whose hash is none of the hashes copied there with that
/// stem. A product that is not in `local` removes nothing; a directory is
/// never removed. Each rebuild of a test target writes it under a new hash,
/// so without this the old copies pile up. Errors leave the file in place.
pub fn prune(local: &Path, rels: &[String]) -> Pruned {
    use std::collections::{BTreeMap, BTreeSet};
    let mut kept: BTreeMap<(PathBuf, String), BTreeSet<String>> = BTreeMap::new();
    for r in rels {
        let p = Path::new(r);
        let (Some(dir), Some(name)) = (p.parent(), p.file_name().and_then(|n| n.to_str())) else {
            continue;
        };
        let Some((stem, hash)) = hashed(name) else {
            continue;
        };
        if !std::fs::symlink_metadata(local.join(p)).is_ok_and(|m| m.is_file()) {
            continue;
        }
        kept.entry((local.join(dir), stem.to_string()))
            .or_default()
            .insert(hash.to_string());
    }
    let mut out = Pruned::default();
    for ((dir, stem), hashes) in &kept {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.filter_map(Result::ok) {
            let name = e.file_name();
            let Some((s, h)) = name.to_str().and_then(hashed) else {
                continue;
            };
            if s != stem || hashes.contains(h) {
                continue;
            }
            let path = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                out.files += 1;
                out.bytes += meta.len();
            }
        }
    }
    out
}

/// What one fetch of products did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// Paths fetched.
    pub count: usize,
    /// Bytes rsync received.
    pub bytes: u64,
    /// Wall time of every rsync together.
    pub elapsed: Duration,
    /// Superseded files removed after the copy.
    pub pruned: Pruned,
}

impl Fetched {
    /// The line the job's caller sees on stderr.
    pub fn line(&self) -> String {
        let mut line = format!(
            "slot: fetched {} artifact{}, {:.1} MB, in {:.1} s",
            self.count,
            if self.count == 1 { "" } else { "s" },
            self.bytes as f64 / 1e6,
            self.elapsed.as_secs_f64()
        );
        let p = self.pruned;
        if p.files > 0 {
            let _ = write!(
                line,
                "; removed {} superseded file{}, {:.1} MB",
                p.files,
                if p.files == 1 { "" } else { "s" },
                p.bytes as f64 / 1e6
            );
        }
        line
    }
}

/// `Total bytes received: 1,234` of rsync's `--stats`.
fn bytes_received(out: &str) -> Option<u64> {
    out.lines()
        .find_map(|l| l.trim().strip_prefix("Total bytes received:"))
        .map(|v| v.chars().filter(char::is_ascii_digit).collect::<String>())
        .and_then(|d| d.parse().ok())
}

/// Where one fetch reads and writes.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    /// ssh host.
    pub host: &'a str,
    /// ssh command and options.
    pub ssh: &'a [String],
    /// The job's target directory on the host.
    pub target: &'a str,
}

/// One rsync of `rels` from `src` into `local`; the bytes it received.
fn rsync(
    src: &Source<'_>,
    rels: &[String],
    local: &Path,
    bounds: &crate::sync::Bounds,
) -> Result<u64> {
    let mut rsync = Command::new("rsync");
    rsync
        .args(["-a", "--relative", "--mkpath", "--stats", "-e"])
        .arg(src.ssh.join(" "));
    for r in rels {
        rsync.arg(format!(
            "{}:{}/./{r}",
            src.host,
            src.target.trim_end_matches('/')
        ));
    }
    rsync
        .arg(format!("{}/", local.display()))
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    let mut child = rsync.spawn().map_err(|e| anyhow!("rsync: {e}"))?;
    let mut out = child.stdout.take().ok_or_else(|| anyhow!("rsync stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let st = crate::sync::watch(
        &mut child,
        Some(bounds.stall),
        Instant::now() + bounds.limit,
        bounds.limit,
    );
    let out = reader.join().unwrap_or_default();
    let st = st.map_err(|stop| anyhow!("rsync killed: {stop}"))?;
    if !st.success() {
        bail!("rsync exited with {st}");
    }
    Ok(bytes_received(&out).unwrap_or(0))
}

/// Copies `rels` (relative to the job's target directory) from `src` into
/// `local`, keeping their relative paths, modes and mtimes, in one rsync per
/// [`BATCH`] paths. A batch that fails is copied again one path per rsync:
/// the `slot shell` of an older server admits a fetch of one path only.
/// Each rsync is bounded like a sync.
pub fn fetch(
    src: &Source<'_>,
    rels: &[String],
    local: &Path,
    bounds: &crate::sync::Bounds,
) -> Result<Fetched> {
    let t0 = Instant::now();
    let mut bytes = 0;
    for batch in rels.chunks(BATCH) {
        bytes += match rsync(src, batch, local, bounds) {
            Ok(b) => b,
            Err(e) if batch.len() == 1 => return Err(e),
            Err(_) => {
                let mut sum = 0;
                for r in batch {
                    sum += rsync(src, std::slice::from_ref(r), local, bounds)?;
                }
                sum
            }
        };
    }
    Ok(Fetched {
        count: rels.len(),
        bytes,
        elapsed: t0.elapsed(),
        pruned: Pruned::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_build_gets_the_json_format_after_its_subcommand() {
        let p = plan(&v(&["cargo", "+nightly", "--locked", "build", "-p", "x"])).unwrap();
        assert_eq!(
            p.argv,
            v(&["cargo", "+nightly", "--locked", "build", FORMAT, "-p", "x"])
        );
        assert!(p.strip);
        assert_eq!(p.when, When::Success);
        let p = plan(&v(&[
            "/usr/bin/cargo",
            "-C",
            "sub",
            "t",
            "--",
            "--nocapture",
        ]))
        .unwrap();
        assert_eq!(
            p.argv,
            v(&[
                "/usr/bin/cargo",
                "-C",
                "sub",
                "t",
                FORMAT,
                "--",
                "--nocapture"
            ])
        );
        assert_eq!(p.when, When::AnyExit);
    }

    #[test]
    fn a_callers_format_decides() {
        let json = v(&[
            "cargo",
            "build",
            "--message-format",
            "json-diagnostic-short",
        ]);
        assert_eq!(
            plan(&json),
            Some(Plan {
                argv: json.clone(),
                strip: false,
                when: When::Success
            })
        );
        assert!(plan(&v(&["cargo", "build", "--message-format=short"])).is_none());
        let after = v(&["cargo", "test", "--", "--message-format=short"]);
        assert!(plan(&after).unwrap().strip, "a test binary's argument");
    }

    #[test]
    fn other_commands_bring_nothing_back() {
        for argv in [
            &["cargo", "check"][..],
            &["cargo", "clippy"],
            &["cargo", "run"],
            &["cargo", "--version"],
            &["cargo-nextest", "nextest", "run"],
            &["make", "build"],
        ] {
            assert!(plan(&v(argv)).is_none(), "{argv:?}");
        }
    }

    const BIN: &str = r#"{"reason":"compiler-artifact","package_id":"path+file:///n/ws/abc#tiny@0.1.0","target":{"kind":["bin"],"name":"tiny"},"filenames":["/n/cache/target/abc/debug/tiny"],"executable":"/n/cache/target/abc/debug/tiny","fresh":false}"#;
    const LIB: &str = r#"{"reason":"compiler-artifact","package_id":"tiny 0.1.0 (path+file:///n/ws/abc)","target":{"kind":["cdylib","rlib"],"name":"tiny"},"filenames":["/n/cache/target/abc/debug/libtiny.so","/n/cache/target/abc/debug/libtiny.rlib"],"executable":null,"fresh":true}"#;
    const DEP: &str = r#"{"reason":"compiler-artifact","package_id":"registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0","target":{"kind":["bin"],"name":"x"},"filenames":["/n/cache/target/abc/debug/x"],"executable":"/n/cache/target/abc/debug/x","fresh":false}"#;
    const BUILD_RS: &str = r#"{"reason":"compiler-artifact","package_id":"path+file:///n/ws/abc#tiny@0.1.0","target":{"kind":["custom-build"],"name":"build-script-build"},"filenames":["/n/cache/target/abc/debug/build/tiny-1/build-script-build"],"executable":null,"fresh":false}"#;

    #[test]
    fn the_collector_keeps_own_products_and_strips_its_lines() {
        let mut c = Collector::new(true);
        let mut out = Vec::new();
        let stream = format!(
            "running 1 test\n{BIN}\n{LIB}\n{DEP}\n{BUILD_RS}\n{{\"reason\":\"build-finished\",\"success\":true}}\n{{\"not\":1}}\ntail"
        );
        for chunk in stream.as_bytes().chunks(7) {
            out.extend(c.push(chunk));
        }
        out.extend(c.finish());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "running 1 test\n{\"not\":1}\ntail"
        );
        assert_eq!(
            c.paths(),
            [
                "/n/cache/target/abc/debug/tiny",
                "/n/cache/target/abc/debug/libtiny.so"
            ]
        );
        let mut keep = Collector::new(false);
        let out = keep.push(format!("{BIN}\n{BIN}\n").as_bytes());
        assert_eq!(out.len(), 2 * (BIN.len() + 1), "the caller's own JSON");
        assert_eq!(keep.paths().len(), 1, "each path once");
    }

    #[test]
    fn paths_map_from_the_node_target_dir_only() {
        let t = "/n/cache/target/abc";
        assert_eq!(
            relative("/n/cache/target/abc/debug/deps/it-1f", t).as_deref(),
            Some("debug/deps/it-1f")
        );
        assert_eq!(
            relative("/work/target/release/tiny", t).as_deref(),
            Some("release/tiny")
        );
        for bad in [
            "/n/cache/target/abcd/debug/x",
            "/n/cache/target/abc",
            "/n/cache/target/abc/../x",
            "/n/ws/abc/target/debug/x",
        ] {
            assert_eq!(relative(bad, t), None, "{bad}");
        }
    }

    #[test]
    fn the_local_target_dir_is_beside_the_lock_file() {
        let d = tempfile::tempdir().unwrap();
        let top = d.path();
        let sub = top.join("rust/crates/a/src");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(local_target(&sub, top, None), top.join("target"));
        std::fs::write(top.join("rust/crates/a/Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(
            local_target(&sub, top, None),
            top.join("rust/crates/a/target")
        );
        std::fs::write(top.join("rust/Cargo.toml"), "[workspace]\n").unwrap();
        assert_eq!(local_target(&sub, top, None), top.join("rust/target"));
        std::fs::write(top.join("Cargo.lock"), "").unwrap();
        assert_eq!(local_target(&sub, top, None), top.join("target"));
        assert_eq!(local_target(&sub, top, Some("/t")), PathBuf::from("/t"));
        assert_eq!(local_target(&sub, top, Some("out")), sub.join("out"));
    }

    const H1: &str = "0123456789abcdef";
    const H2: &str = "fedcba9876543210";
    const H3: &str = "00112233445566ff";

    /// Files under `dir`, relative, sorted.
    fn files(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                let rel = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                if p.is_dir() {
                    out.push(format!("{rel}/"));
                    todo.push(p);
                } else {
                    out.push(rel);
                }
            }
        }
        out.sort();
        out
    }

    fn put(root: &Path, rel: &str, len: usize) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![0u8; len]).unwrap();
    }

    #[test]
    fn a_second_copy_of_a_rebuilt_test_target_leaves_one_binary() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path();
        // First copy-back: the test binary under its first hash.
        let first = vec![format!("debug/deps/it-{H1}")];
        put(t, &first[0], 100);
        assert_eq!(prune(t, &first), Pruned::default(), "nothing older yet");
        // A local build left its dep-info beside it.
        put(t, &format!("debug/deps/it-{H1}.d"), 10);
        // Second copy-back: the same target under a new hash.
        let second = vec![format!("debug/deps/it-{H2}")];
        put(t, &second[0], 200);
        assert_eq!(
            prune(t, &second),
            Pruned {
                files: 2,
                bytes: 110
            }
        );
        assert_eq!(
            files(t),
            ["debug/", "debug/deps/", &format!("debug/deps/it-{H2}")]
        );
    }

    #[test]
    fn pruning_touches_only_older_hashes_of_a_stem_just_copied() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path();
        let keep = [
            // Copied now: a unit-test binary and an integration test of the
            // same name, both live; and an unhashed binary.
            format!("debug/deps/tiny-{H1}"),
            format!("debug/deps/tiny-{H2}"),
            "debug/tiny".to_string(),
        ];
        for k in &keep {
            put(t, k, 1);
        }
        let left = [
            // The new hash's own sibling file.
            format!("debug/deps/tiny-{H1}.d"),
            // Other stems, and names that are not `<stem>-<16 hex>`.
            format!("debug/deps/tiny_more-{H3}"),
            format!("debug/deps/libtiny-{H3}.rlib"),
            format!("debug/deps/tiny-{}", H3.to_uppercase()),
            format!("debug/deps/tiny-{}", &H3[..15]),
            format!("debug/deps/tiny-{H3}-x"),
            format!("debug/deps/tiny-{H3}0"),
            // An older hash of the unhashed binary's name.
            format!("debug/tiny-{H3}"),
            // The same stem in another directory.
            format!("release/deps/tiny-{H3}"),
        ];
        for l in &left {
            put(t, l, 1);
        }
        // A directory under an older hash's name stays.
        std::fs::create_dir_all(t.join(format!("debug/deps/tiny-{H3}.dir"))).unwrap();
        let gone = [
            format!("debug/deps/tiny-{H3}"),
            format!("debug/deps/tiny-{H3}.d"),
            format!("debug/deps/tiny-{H3}.dwp"),
        ];
        for g in &gone {
            put(t, g, 7);
        }
        assert_eq!(
            prune(t, &keep),
            Pruned {
                files: 3,
                bytes: 21
            }
        );
        for k in keep.iter().chain(&left) {
            assert!(t.join(k).is_file(), "{k} kept");
        }
        assert!(t.join(format!("debug/deps/tiny-{H3}.dir")).is_dir());
        for g in &gone {
            assert!(!t.join(g).exists(), "{g} removed");
        }
    }

    #[test]
    fn a_product_that_did_not_arrive_removes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path();
        put(t, &format!("debug/deps/it-{H1}"), 1);
        let absent = vec![format!("debug/deps/it-{H2}")];
        assert_eq!(prune(t, &absent), Pruned::default());
        assert!(t.join(format!("debug/deps/it-{H1}")).is_file());
        assert_eq!(prune(&t.join("missing"), &absent), Pruned::default());
    }

    #[test]
    fn the_line_names_count_size_and_time() {
        let mut f = Fetched {
            count: 3,
            bytes: 12_345_678,
            elapsed: Duration::from_millis(2340),
            pruned: Pruned::default(),
        };
        assert_eq!(f.line(), "slot: fetched 3 artifacts, 12.3 MB, in 2.3 s");
        f.pruned = Pruned {
            files: 2,
            bytes: 45_600_000,
        };
        assert_eq!(
            f.line(),
            "slot: fetched 3 artifacts, 12.3 MB, in 2.3 s; removed 2 superseded files, 45.6 MB"
        );
        assert_eq!(
            bytes_received("Total bytes sent: 1\nTotal bytes received: 9,876\n"),
            Some(9876)
        );
    }
}
