//! Disk-budget eviction of local Cargo target directories.
//!
//! When the filesystem holding target directories has less free space than
//! a floor, the daemon deletes the target directories it saw jobs build
//! into, least recently built first, until free space is above the floor
//! plus a margin. On a build node these are the directories the jobs that
//! arrived with a workspace built into (`Job::node_target_dir`), but not
//! one under the daemon's cache root: that one is the cache LRU's
//! (`server/cache.rs`). It never deletes a directory a queued or running job
//! builds into (its own or the one its `CARGO_TARGET_DIR` names, a directory
//! nested in it or holding it), one in the tree a queued or running job
//! works in (git's top level of its working directory, else the working
//! directory) or in the tree of a job that ran in the last hour, one built
//! more recently than the idle keep (2 h, or the one `slot evict-below
//! --idle` set; never less than an hour), one a process or a service uses
//! or whose `.cargo-lock` a process holds locked, one whose removal failed
//! in the last hour, or a path that no job of the log built into. The mark is
//! `--evict-below`, or the one `slot evict-below` set at run time. A path
//! must carry Cargo's `CACHEDIR.TAG` to count as a target directory. A dry
//! run (`--evict-dry-run`, or [`FILE`]) only says what it would delete.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// First line of the `CACHEDIR.TAG` Cargo writes into a target directory.
pub const CACHEDIR_SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

/// Settings; a floor of 0 turns eviction off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Free bytes below which eviction starts.
    pub floor: u64,
    /// Free bytes above the floor eviction stops at.
    pub margin: u64,
    /// A target directory built more recently than this is kept.
    pub min_idle: Duration,
    /// Only report what would be deleted.
    pub dry_run: bool,
}

/// The idle keep until `slot evict-below --idle` sets one: a target
/// directory built in the last 2 h is kept.
pub const DEFAULT_MIN_IDLE: Duration = Duration::from_secs(2 * 3600);

/// The least idle keep `slot evict-below --idle` accepts, seconds.
pub const MIN_IDLE_S: u64 = 5 * 60;

/// The most idle keep `slot evict-below --idle` accepts, seconds: 7 days.
pub const MAX_IDLE_S: u64 = 7 * 24 * 3600;

/// How long a target directory whose removal failed is left out of
/// eviction, so the next pass plans the next candidates.
pub const FAILED_KEEP: Duration = Duration::from_secs(3600);

/// The idle keep in force: the one set with `slot evict-below --idle`
/// (`set_s`, kept in `limits.json`), else `option`.
pub fn min_idle(set_s: Option<u64>, option: Duration) -> Duration {
    set_s.map_or(option, Duration::from_secs)
}

/// Why an idle keep for `slot evict-below --idle` is refused; `None` when
/// it is from [`MIN_IDLE_S`] to [`MAX_IDLE_S`].
pub fn idle_refused(s: u64) -> Option<String> {
    (!(MIN_IDLE_S..=MAX_IDLE_S).contains(&s)).then(|| {
        format!(
            "slot: the idle keep must be from {} to {} (7 days), not {}",
            crate::run_limit::text(MIN_IDLE_S),
            crate::run_limit::text(MAX_IDLE_S),
            crate::run_limit::text(s)
        )
    })
}

/// `--idle` of `slot evict-below`: a duration from 5 min to 7 days.
pub fn parse_idle(s: &str) -> Result<u64, String> {
    let secs = crate::run_limit::parse_duration(s)?;
    match idle_refused(secs) {
        Some(why) => Err(why.trim_start_matches("slot: ").to_string()),
        None => Ok(secs),
    }
}

/// File in the state directory that overrides `--evict-dry-run`.
pub const FILE: &str = "evict.json";

/// What [`FILE`] holds. The daemon reads it and never writes it, so a
/// provisioning tool or an operator owns it.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct FileSettings {
    /// Only report what would be deleted; `None` leaves `--evict-dry-run`.
    #[serde(default)]
    dry_run: Option<bool>,
}

/// Whether eviction only reports: `dry_run` of [`FILE`] in `state_dir`
/// when it sets one, else `flag` (`--evict-dry-run`). Read at every pass, so
/// a change applies without a reload.
pub fn dry_run(state_dir: &Path, flag: bool) -> bool {
    std::fs::read_to_string(state_dir.join(FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<FileSettings>(&s).ok())
        .and_then(|f| f.dry_run)
        .unwrap_or(flag)
}

/// The free space below which eviction starts: the larger of
/// `--evict-below` and `--min-free-disk`, so eviction runs whenever the disk
/// hold stops builds from starting. `--evict-below 0` keeps it off.
pub fn mark(evict_below: u64, min_free_disk: u64) -> u64 {
    if evict_below == 0 {
        0
    } else {
        evict_below.max(min_free_disk)
    }
}

/// The largest eviction mark `slot evict-below` accepts, GB.
pub const MAX_BELOW_GB: f64 = 100_000.0;

/// GB as bytes; 0 for 0, a negative or a non-finite value.
pub fn gb_bytes(gb: f64) -> u64 {
    if gb.is_finite() && gb > 0.0 {
        (gb * crate::admission::GB as f64) as u64
    } else {
        0
    }
}

/// The eviction mark before the disk hold raises it: the one set with
/// `slot evict-below` (`set_gb`, kept in `limits.json`), else
/// `--evict-below` (`option`, bytes). 0 is off.
pub fn below(set_gb: Option<f64>, option: u64) -> u64 {
    set_gb.map_or(option, gb_bytes)
}

/// Why a mark for `slot evict-below` is refused; `None` when it is a
/// number from 0 to [`MAX_BELOW_GB`].
pub fn below_refused(gb: f64) -> Option<String> {
    if !gb.is_finite() || gb < 0.0 {
        return Some(format!(
            "slot: the eviction mark must be a number of GB from 0 (off) to {MAX_BELOW_GB}, not {gb}"
        ));
    }
    if gb > MAX_BELOW_GB {
        return Some(format!(
            "slot: the eviction mark {gb} GB is over the most, {MAX_BELOW_GB} GB"
        ));
    }
    None
}

/// Body of `PUT /api/evict-below`. A field that is absent leaves its
/// setting as it is; at least one must be named, so an empty body changes
/// nothing.
#[allow(
    clippy::option_option,
    reason = "absent leaves a setting, `null` forgets it, a value sets it"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct BelowRequest {
    /// The mark, GB, 0 = eviction off; `null` forgets it and
    /// `--evict-below` applies again.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub evict_below_gb: Option<Option<f64>>,
    /// The idle keep, seconds: a target directory built more recently is
    /// kept; `null` forgets it and the 2 h default applies again.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub evict_idle_s: Option<Option<u64>>,
}

/// A field that is present: `Some(None)` for `null`, so it differs from an
/// absent field (`None`, by `default`).
#[allow(clippy::option_option, reason = "the type of a field of BelowRequest")]
fn named<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::deserialize(d).map(Some)
}

impl BelowRequest {
    /// The request `slot evict-below [GB|default] [--idle DURATION]` makes;
    /// `None` without arguments (it only reads). `default` forgets the mark
    /// and, unless `--idle` names one, the idle keep.
    pub fn of(mark: Option<BelowSetting>, idle_s: Option<u64>) -> Option<Self> {
        let idle = match (mark, idle_s) {
            (_, Some(s)) => Some(Some(s)),
            (Some(BelowSetting::Default), None) => Some(None),
            _ => None,
        };
        let r = Self {
            evict_below_gb: mark.map(BelowSetting::value),
            evict_idle_s: idle,
        };
        (r != Self::default()).then_some(r)
    }

    /// The request when its values are accepted, or why it is refused.
    pub fn checked(self) -> Result<Self, String> {
        if self.evict_below_gb.is_none() && self.evict_idle_s.is_none() {
            return Err("slot: name `evict_below_gb`, `evict_idle_s` or both".into());
        }
        if let Some(why) = self.evict_below_gb.flatten().and_then(below_refused) {
            return Err(why);
        }
        if let Some(why) = self.evict_idle_s.flatten().and_then(idle_refused) {
            return Err(why);
        }
        Ok(self)
    }
}

/// Answer of `GET` and `PUT /api/evict-below`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct BelowView {
    /// The mark set with `slot evict-below`, GB; `None` = `--evict-below`.
    #[serde(default)]
    pub evict_below_gb: Option<f64>,
    /// `--evict-below`, bytes.
    #[serde(default)]
    pub option: u64,
    /// `--min-free-disk`, bytes.
    #[serde(default)]
    pub min_free_disk: u64,
    /// The mark in force, bytes, after the disk hold raised it; 0 = off.
    #[serde(default)]
    pub floor: u64,
    /// The idle keep set with `slot evict-below --idle`, seconds; `None` =
    /// the 2 h default.
    #[serde(default)]
    pub evict_idle_s: Option<u64>,
    /// The idle keep in force, seconds; 0 from a daemon older than the
    /// setting.
    #[serde(default)]
    pub idle_s: u64,
}

impl BelowView {
    /// The view of a mark `set_gb` over the options `option` and
    /// `min_free_disk`.
    pub fn of(set_gb: Option<f64>, option: u64, min_free_disk: u64) -> Self {
        Self {
            evict_below_gb: set_gb,
            option,
            min_free_disk,
            floor: mark(below(set_gb, option), min_free_disk),
            ..Self::default()
        }
    }

    /// The view with the idle keep `set_s` over the default `option`.
    #[must_use]
    pub fn with_idle(self, set_s: Option<u64>, option: Duration) -> Self {
        Self {
            evict_idle_s: set_s,
            idle_s: min_idle(set_s, option).as_secs(),
            ..self
        }
    }

    /// `slot evict-below`: the mark in force and where it comes from, and
    /// the idle keep.
    pub fn render(&self) -> String {
        let mut out = self.render_mark();
        if self.idle_s > 0 {
            let from = if self.evict_idle_s.is_some() {
                "set with `slot evict-below --idle`"
            } else {
                "the default"
            };
            let _ = writeln!(
                out,
                "eviction keeps target dirs built in the last {} ({from})",
                crate::run_limit::text(self.idle_s)
            );
        }
        out
    }

    fn render_mark(&self) -> String {
        let below = below(self.evict_below_gb, self.option);
        let from = match self.evict_below_gb {
            Some(_) => format!(
                "set with `slot evict-below`; `--evict-below` is {}",
                crate::admission::gb(self.option)
            ),
            None => "`--evict-below`".to_string(),
        };
        if self.floor == 0 {
            return format!("eviction is off ({from})\n");
        }
        let raised = if self.floor > below {
            format!(
                ", raised from {} to the disk hold's mark (`--min-free-disk`)",
                crate::admission::gb(below)
            )
        } else {
            String::new()
        };
        format!(
            "eviction starts below {} free{raised} ({from})\n",
            crate::admission::gb(self.floor)
        )
    }
}

/// The argument of `slot evict-below`: a number of GB, or `default`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BelowSetting {
    /// Forget the mark: `--evict-below` applies again.
    Default,
    /// The mark, GB; 0 turns eviction off.
    At(f64),
}

impl BelowSetting {
    /// The value `limits.json` and the API carry.
    pub fn value(self) -> Option<f64> {
        match self {
            Self::Default => None,
            Self::At(gb) => Some(gb),
        }
    }
}

impl std::str::FromStr for BelowSetting {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        if s == "default" {
            return Ok(Self::Default);
        }
        let Ok(gb) = s.parse::<f64>() else {
            return Err(format!(
                "`{s}` is not a number of GB: give one from 0 (off) to {MAX_BELOW_GB}, or `default`"
            ));
        };
        match below_refused(gb) {
            Some(why) => Err(why.trim_start_matches("slot: ").to_string()),
            None => Ok(Self::At(gb)),
        }
    }
}

/// Whether a queued or running job that builds into `target_dir` and runs
/// in `cwd` keeps target directory `dir`: it builds into it, below it or
/// above it ([`holds_target`]), or it runs in the checkout that holds it
/// ([`runs_in_checkout`]).
pub fn job_keeps(dir: &Path, target_dir: &Path, cwd: &Path) -> bool {
    holds_target(dir, target_dir) || runs_in_checkout(dir, cwd, &tree_of(cwd))
}

/// Whether target directory `dir` equals, holds or lies inside `target`, a
/// target directory a job builds into (`target/ignite-test/rt` keeps
/// `target`, and `target` keeps the ones nested in it).
pub fn holds_target(dir: &Path, target: &Path) -> bool {
    !target.as_os_str().is_empty() && (target.starts_with(dir) || dir.starts_with(target))
}

/// Whether a job working in `cwd`, whose tree is `tree` ([`tree_of`]), uses
/// target directory `dir`: `dir` lies anywhere in the tree (nested ones
/// such as `target/ignite-test/rt` or `driver/target` too), or `cwd` lies
/// below the directory that holds `dir`.
fn runs_in_checkout(dir: &Path, cwd: &Path, tree: &Path) -> bool {
    !cwd.as_os_str().is_empty()
        && (dir.parent().is_some_and(|c| cwd.starts_with(c))
            || (!tree.as_os_str().is_empty() && dir.starts_with(tree)))
}

/// The tree a job working in `cwd` uses: git's top level (the nearest
/// directory at or above `cwd` that holds `.git`), else `cwd` itself.
/// A `.git` in the home directory (`$HOME`) or above it does not count: a
/// repository of the whole home would make every target directory on the
/// host one tree. Empty for an empty `cwd`.
pub fn tree_of(cwd: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    tree_under(cwd, home.as_deref())
}

/// [`tree_of`] with the home directory `home`.
fn tree_under(cwd: &Path, home: Option<&Path>) -> PathBuf {
    if cwd.as_os_str().is_empty() {
        return PathBuf::new();
    }
    cwd.ancestors()
        .take_while(|d| home.is_none_or(|h| !h.starts_with(d)))
        .find(|d| std::fs::symlink_metadata(d.join(".git")).is_ok())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// How long the tree of a job keeps every target directory in it after the
/// job ran, whatever the idle keep: the steps of one long task run as
/// several jobs in a row and use the same target directories between them.
pub const TREE_KEEP: Duration = Duration::from_secs(3600);

/// The working directories of `ran` (a directory and when the last job
/// ran in it, Unix ms) a job ran in within [`TREE_KEEP`] of `now_ms`: their
/// trees keep their target directories like a queued job's
/// ([`candidates`]).
#[allow(clippy::implicit_hasher)]
pub fn ran_recently(ran: &HashMap<PathBuf, i64>, now_ms: i64) -> HashSet<PathBuf> {
    let keep = i64::try_from(TREE_KEEP.as_millis()).unwrap_or(i64::MAX);
    ran.iter()
        .filter(|(_, at)| now_ms - **at < keep)
        .map(|(p, _)| p.clone())
        .collect()
}

/// The target directory a job's environment names (`CARGO_TARGET_DIR`,
/// else `CARGO_BUILD_TARGET_DIR`; a later entry wins), relative to `cwd`.
pub fn env_target(env: &[String], cwd: &Path) -> Option<PathBuf> {
    ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"]
        .iter()
        .find_map(|k| {
            env.iter()
                .rev()
                .find_map(|e| e.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
                .filter(|v| !v.is_empty())
        })
        .map(|t| normalize(&cwd.join(t)))
}

/// [`env_target`] of process `pid`, read from `/proc/<pid>/environ`: the
/// target of a running job whose request a reload did not keep.
pub fn pid_env_target(pid: i32, cwd: &Path) -> Option<PathBuf> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let env: Vec<String> = raw
        .split(|b| *b == 0)
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .collect();
    env_target(&env, cwd)
}

/// The mark as `slot status` and the journal name it: `40.0 GB`, or
/// `50.0 GB, raised from 40.0 GB to the disk hold's mark` when
/// `--min-free-disk` raised it from `--evict-below` (`raised_from`, 0 when
/// it did not).
pub fn mark_text(floor: u64, raised_from: u64) -> String {
    if raised_from == 0 {
        return crate::admission::gb(floor);
    }
    format!(
        "{}, raised from {} to the disk hold's mark",
        crate::admission::gb(floor),
        crate::admission::gb(raised_from)
    )
}

/// `disk: 32.0 GB free, under 40.0 GB: eviction is off (dry run), would
/// free 31.0 GB in 2 dir(s)`: what a dry run under the floor says in the
/// journal and in `slot status`. `raised_from` as in [`mark_text`].
pub fn off_summary(free: u64, floor: u64, raised_from: u64, done: &[Eviction]) -> String {
    let bytes: u64 = done.iter().map(|e| e.bytes).sum();
    format!(
        "disk: {} free, under {}: eviction is off (dry run), would free {} in {} dir(s)",
        crate::admission::gb(free),
        mark_text(floor, raised_from),
        crate::admission::gb(bytes),
        done.len()
    )
}

/// What `slot status` and the web UI show about eviction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct Report {
    /// Free bytes below which eviction starts (the mark); 0 = off.
    #[ts(type = "number")]
    #[serde(default)]
    pub floor: u64,
    /// `--evict-below`, bytes, when `--min-free-disk` is larger and so
    /// raised the mark to the disk hold's value; 0 when it did not.
    #[ts(type = "number")]
    #[serde(default)]
    pub raised_from: u64,
    /// Only reporting, not deleting.
    #[serde(default)]
    pub dry_run: bool,
    /// Target directories seen in the job log.
    #[serde(default)]
    pub known: u32,
    /// Lowest free space of their filesystems at the last look, bytes.
    #[ts(type = "number")]
    #[serde(default)]
    pub free: u64,
    /// The last look measured nothing: no known target directory exists,
    /// so no filesystem was asked and `free` is 0. False from a daemon
    /// older than the field, whose `free` of 0 may be this case.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unmeasured: bool,
    /// Time of the last look, Unix milliseconds.
    #[ts(type = "number")]
    #[serde(default)]
    pub checked_ms: i64,
    /// What the last pass deleted, or would have.
    #[serde(default)]
    pub last: Vec<Eviction>,
    /// Bytes deleted since the daemon started.
    #[ts(type = "number")]
    #[serde(default)]
    pub deleted_bytes: u64,
}

/// One target directory deleted, or that would be in a dry run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct Eviction {
    /// The target directory.
    pub path: String,
    /// Its size, bytes.
    #[ts(type = "number")]
    pub bytes: u64,
    /// When a job last built into it, Unix milliseconds.
    #[ts(type = "number")]
    pub last_ms: i64,
    /// `deleted`, `would delete`, `discard-cache` (a managed worktree) or
    /// why it failed.
    pub action: String,
}

/// The target directory a Rust job builds into: `--target-dir`,
/// `CARGO_TARGET_DIR` or `CARGO_BUILD_TARGET_DIR`, else `target` beside the
/// workspace root found from `--manifest-path` or the working directory.
pub fn target_dir(argv: &[String], env: &[String], cwd: &Path) -> Option<PathBuf> {
    let get = |k: &str| {
        env.iter()
            .rev()
            .find_map(|e| e.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
            .filter(|v| !v.is_empty())
    };
    let mut flag = None;
    let mut manifest = None;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            break;
        }
        if let Some(v) = a.strip_prefix("--target-dir=") {
            flag = Some(v.to_string());
        } else if a == "--target-dir" {
            flag = it.next().cloned();
        } else if let Some(v) = a.strip_prefix("--manifest-path=") {
            manifest = Some(v.to_string());
        } else if a == "--manifest-path" {
            manifest = it.next().cloned();
        }
    }
    if let Some(t) = flag
        .or_else(|| get("CARGO_TARGET_DIR").map(String::from))
        .or_else(|| get("CARGO_BUILD_TARGET_DIR").map(String::from))
    {
        return Some(normalize(&cwd.join(t)));
    }
    let start = manifest.map_or_else(
        || cwd.to_path_buf(),
        |m| {
            let m = cwd.join(m);
            m.parent()
                .map_or_else(|| cwd.to_path_buf(), Path::to_path_buf)
        },
    );
    workspace_root(&start).map(|r| normalize(&r.join("target")))
}

/// The directory of the workspace `Cargo.toml` above `dir`: the highest
/// ancestor whose `Cargo.toml` has a `[workspace]` table, else the nearest
/// `Cargo.toml`.
fn workspace_root(dir: &Path) -> Option<PathBuf> {
    let mut nearest = None;
    let mut workspace = None;
    let mut d = Some(dir);
    while let Some(p) = d {
        if let Ok(s) = std::fs::read_to_string(p.join("Cargo.toml")) {
            nearest.get_or_insert_with(|| p.to_path_buf());
            if s.lines().any(|l| l.trim() == "[workspace]") {
                workspace = Some(p.to_path_buf());
            }
        }
        d = p.parent();
    }
    workspace.or(nearest)
}

/// `a/./b/../c` → `a/c`, without touching the filesystem.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// Whether `p` may be deleted as a target directory: absolute, at least
/// three levels deep, a real directory (not a symlink) and tagged by Cargo.
pub fn is_target_dir(p: &Path) -> bool {
    if !p.is_absolute() || p.components().count() < 4 {
        return false;
    }
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return false;
    };
    if !m.is_dir() {
        return false;
    }
    std::fs::read_to_string(p.join("CACHEDIR.TAG")).is_ok_and(|s| s.starts_with(CACHEDIR_SIGNATURE))
}

/// Bytes under `p`, not following symlinks or leaving its filesystem.
pub fn dir_size(p: &Path) -> u64 {
    let Ok(root) = std::fs::symlink_metadata(p) else {
        return 0;
    };
    let dev = root.dev();
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.dev() != dev {
                continue;
            }
            if m.is_dir() {
                stack.push(e.path());
            } else {
                total += m.blocks() * 512;
            }
        }
    }
    total
}

/// A target directory that may go, oldest build first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The directory.
    pub path: PathBuf,
    /// Repository name of its last job.
    pub repo: String,
    /// When a job last built into it, Unix milliseconds.
    pub last_ms: i64,
}

/// Target directories eligible now, least recently built first: not
/// built within `min_idle` (and never within [`TREE_KEEP`]) of `now_ms`,
/// not equal to, inside or holding one of `busy_dirs` (the target
/// directories of queued and running jobs, [`holds_target`]) and not in
/// the tree of one of `busy_cwds` (the working directories of queued and
/// running jobs and of jobs that ran within [`TREE_KEEP`],
/// [`job_keeps`]). Another directory of the same repository may go.
#[allow(clippy::implicit_hasher)]
pub fn candidates(
    seen: &HashMap<PathBuf, (String, i64)>,
    busy_dirs: &HashSet<PathBuf>,
    busy_cwds: &HashSet<PathBuf>,
    now_ms: i64,
    min_idle: Duration,
) -> Vec<Candidate> {
    let idle_ms = i64::try_from(min_idle.max(TREE_KEEP).as_millis()).unwrap_or(i64::MAX);
    let trees: Vec<(&PathBuf, PathBuf)> = busy_cwds.iter().map(|c| (c, tree_of(c))).collect();
    let mut v: Vec<Candidate> = seen
        .iter()
        .filter(|(p, (_, last))| {
            now_ms - *last >= idle_ms
                && !busy_dirs.iter().any(|t| holds_target(p, t))
                && !trees.iter().any(|(c, t)| runs_in_checkout(p, c, t))
        })
        .map(|(p, (repo, last))| Candidate {
            path: p.clone(),
            repo: repo.clone(),
            last_ms: *last,
        })
        .collect();
    v.sort_by(|a, b| a.last_ms.cmp(&b.last_ms).then_with(|| a.path.cmp(&b.path)));
    v
}

/// Which candidates to delete on a filesystem with `free` bytes: the
/// oldest first until free space would reach `floor + margin`. `sizes`
/// gives each candidate's bytes. Nothing when `free` is at or above the
/// floor.
pub fn plan(free: u64, s: Settings, cands: &[(Candidate, u64)]) -> Vec<&(Candidate, u64)> {
    if s.floor == 0 || free >= s.floor {
        return Vec::new();
    }
    let goal = s.floor.saturating_add(s.margin);
    let mut have = free;
    let mut out = Vec::new();
    for c in cands {
        if have >= goal {
            break;
        }
        if c.1 == 0 {
            continue;
        }
        have = have.saturating_add(c.1);
        out.push(c);
    }
    out
}

/// Paths that processes and services use, read once per eviction pass: a
/// target directory that holds one of them is kept.
#[derive(Debug, Default)]
pub struct InUse {
    /// A used path and who uses it (`process 123 runs it`).
    paths: Vec<(PathBuf, String)>,
    /// Inode numbers of the files a process holds a lock on
    /// (`<proc>/locks`). Matched without the device: `/proc/locks` names
    /// the filesystem's device, which on btrfs differs from the one `stat`
    /// gives, and a false match only keeps a directory.
    locked: HashSet<u64>,
}

/// The inode numbers `/proc/locks` text `text` names (`MAJ:MIN:INODE` on
/// each line, blocked waiters too).
pub fn locked_inodes(text: &str) -> HashSet<u64> {
    text.lines()
        .filter_map(|l| {
            l.split_whitespace().find_map(|w| {
                let mut p = w.split(':');
                let (maj, min, ino) = (p.next()?, p.next()?, p.next()?);
                (p.next().is_none()
                    && u32::from_str_radix(maj, 16).is_ok()
                    && u32::from_str_radix(min, 16).is_ok())
                .then(|| ino.parse::<u64>().ok())
                .flatten()
            })
        })
        .collect()
}

/// Whether a `.cargo-lock` anywhere under `dir` has an inode in `locked`:
/// a cargo holds the directory's build lock.
fn holds_held_lock(dir: &Path, locked: &HashSet<u64>) -> bool {
    if locked.is_empty() {
        return false;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                stack.push(e.path());
            } else if e.file_name() == ".cargo-lock"
                && e.metadata().is_ok_and(|m| locked.contains(&m.ino()))
            {
                return true;
            }
        }
    }
    false
}

impl InUse {
    /// Reads `/proc` and the user's systemd unit directories.
    pub fn scan() -> Self {
        let mut unit_dirs = Vec::new();
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        if let Some(c) = config {
            unit_dirs.push(c.join("systemd/user"));
        }
        if let Some(r) = crate::paths::runtime_dir() {
            unit_dirs.push(r.join("systemd/transient"));
            unit_dirs.push(r.join("systemd/user"));
        }
        Self::scan_from(Path::new("/proc"), &unit_dirs)
    }

    /// [`InUse::scan`] over `proc_root` and `unit_dirs`.
    pub fn scan_from(proc_root: &Path, unit_dirs: &[PathBuf]) -> Self {
        let mut paths = Vec::new();
        for e in std::fs::read_dir(proc_root).into_iter().flatten().flatten() {
            let name = e.file_name();
            let Some(pid) = name
                .to_str()
                .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            else {
                continue;
            };
            let dir = e.path();
            if let Ok(p) = std::fs::read_link(dir.join("exe")) {
                paths.push((p, format!("process {pid} runs it")));
            }
            if let Ok(p) = std::fs::read_link(dir.join("cwd")) {
                paths.push((p, format!("process {pid} works in it")));
            }
            for fd in std::fs::read_dir(dir.join("fd"))
                .into_iter()
                .flatten()
                .flatten()
            {
                if let Ok(p) = std::fs::read_link(fd.path())
                    && p.is_absolute()
                {
                    paths.push((p, format!("process {pid} has a file of it open")));
                }
            }
            let maps = std::fs::read_to_string(dir.join("maps")).unwrap_or_default();
            for line in maps.lines() {
                if let Some(i) = line.find('/') {
                    paths.push((
                        PathBuf::from(&line[i..]),
                        format!("process {pid} has a file of it mapped"),
                    ));
                }
            }
        }
        for d in unit_dirs {
            for (file, line) in unit_lines(d) {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                if !key.trim().starts_with("ExecStart") {
                    continue;
                }
                for word in value.split_whitespace() {
                    let word = word.trim_start_matches(['-', '@', ':', '+', '!']);
                    if word.starts_with('/') {
                        paths.push((
                            PathBuf::from(word),
                            format!("the unit {} starts from it", file.display()),
                        ));
                    }
                }
            }
        }
        let locked =
            locked_inodes(&std::fs::read_to_string(proc_root.join("locks")).unwrap_or_default());
        Self { paths, locked }
    }

    /// Why target directory `dir` is in use, if it is.
    pub fn why(&self, dir: &Path) -> Option<&str> {
        self.paths
            .iter()
            .find(|(p, _)| p.starts_with(dir))
            .map(|(_, why)| why.as_str())
            .or_else(|| {
                holds_held_lock(dir, &self.locked).then_some("a cargo holds a .cargo-lock in it")
            })
    }
}

/// The lines of the unit files in `dir` and in its drop-in directories
/// (`<unit>.d/*.conf`), with the file each came from.
fn unit_lines(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            files.extend(
                std::fs::read_dir(&p)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path()),
            );
        } else {
            files.push(p);
        }
    }
    for f in files {
        if let Ok(text) = std::fs::read_to_string(&f) {
            out.extend(text.lines().map(|l| (f.clone(), l.to_string())));
        }
    }
    out
}

/// Groups candidates by filesystem (device id).
pub fn by_device(cands: Vec<Candidate>) -> BTreeMap<u64, Vec<Candidate>> {
    let mut m: BTreeMap<u64, Vec<Candidate>> = BTreeMap::new();
    for c in cands {
        if let Ok(md) = std::fs::symlink_metadata(&c.path) {
            m.entry(md.dev()).or_default().push(c);
        }
    }
    m
}

/// Deletes target directory `p`. When its parent is a managed worktree
/// (`worktree discard-cache --dry-run` accepts it) the worktree tool
/// deletes the tree's caches instead, which refuses a tree in use.
/// Returns the action taken.
pub fn remove(p: &Path) -> Result<&'static str, String> {
    if !is_target_dir(p) {
        return Err("not a Cargo target directory".into());
    }
    if let Some(tree) = p.parent()
        && managed_tree(tree)
    {
        let out = Command::new("worktree")
            .arg("discard-cache")
            .arg(tree)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("worktree discard-cache: {e}"))?;
        if out.status.success() {
            return Ok("discard-cache");
        }
        return Err(format!(
            "worktree discard-cache: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    std::fs::remove_dir_all(p).map_err(|e| e.to_string())?;
    Ok("deleted")
}

fn managed_tree(tree: &Path) -> bool {
    Command::new("worktree")
        .args(["discard-cache", "--dry-run", "--json"])
        .arg(tree)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    /// A process started from a binary inside a target directory keeps it
    /// until the process ends.
    #[test]
    fn a_target_a_process_runs_from_is_in_use_until_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("debug")).unwrap();
        // A multi-call coreutils picks its tool by name: keep `sleep`.
        let bin = target.join("debug/sleep");
        std::fs::copy("/bin/sleep", &bin).unwrap();
        // Another test of this process may fork while the copy's write
        // descriptor is open; the child holds it until its own exec, and an
        // exec of the copy meanwhile fails with ETXTBSY. Try again.
        let started = std::time::Instant::now();
        let mut child = loop {
            match Command::new(&bin).arg("30").spawn() {
                Err(e)
                    if e.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && started.elapsed() < Duration::from_secs(10) =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                r => break r.unwrap(),
            }
        };
        // Until the child has called exec, its executable is this test's.
        let exe = PathBuf::from(format!("/proc/{}/exe", child.id()));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::fs::read_link(&exe).ok().as_deref() != Some(bin.as_path()) {
            assert!(
                std::time::Instant::now() < deadline,
                "the child never ran {bin:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut used = InUse::scan_from(Path::new("/proc"), &[]);
        while used.why(&target).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            used = InUse::scan_from(Path::new("/proc"), &[]);
        }
        let why = used.why(&target).map(String::from);
        let pid = format!("process {} ", child.id());
        let seen: Vec<_> = used
            .paths
            .iter()
            .filter(|(_, w)| w.starts_with(&pid))
            .collect();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            why,
            Some(format!("process {} runs it", child.id())),
            "the scan saw for the child: {seen:?}; exe now {:?}",
            std::fs::read_link(&exe)
        );
        assert_eq!(InUse::scan_from(Path::new("/proc"), &[]).why(&target), None);
    }

    /// A process working in a target directory, or holding a file of it
    /// open, keeps it.
    #[test]
    fn a_target_a_process_works_in_or_holds_open_is_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let cwd_target = dir.path().join("a/target");
        let open_target = dir.path().join("b/target");
        std::fs::create_dir_all(&cwd_target).unwrap();
        std::fs::create_dir_all(&open_target).unwrap();
        std::fs::write(open_target.join("f"), "x").unwrap();
        let mut child = Command::new("sleep")
            .arg("30")
            .current_dir(&cwd_target)
            .spawn()
            .unwrap();
        // Until the child has changed directory, its directory is this test's.
        let cwd = PathBuf::from(format!("/proc/{}/cwd", child.id()));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::fs::read_link(&cwd).ok().as_deref() != Some(cwd_target.as_path()) {
            assert!(
                std::time::Instant::now() < deadline,
                "the child never worked in {cwd_target:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let file = std::fs::File::open(open_target.join("f")).unwrap();
        let used = InUse::scan_from(Path::new("/proc"), &[]);
        child.kill().unwrap();
        child.wait().unwrap();
        drop(file);
        assert_eq!(
            used.why(&cwd_target),
            Some(format!("process {} works in it", child.id()).as_str())
        );
        assert_eq!(
            used.why(&open_target),
            Some(format!("process {} has a file of it open", std::process::id()).as_str())
        );
    }

    /// A user unit whose `ExecStart` names a binary inside a target
    /// directory keeps it, from the unit file or from a drop-in, whether or
    /// not the unit runs.
    #[test]
    fn a_target_a_units_exec_start_names_is_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let units = dir.path().join("units");
        let a = dir.path().join("a/target");
        let b = dir.path().join("b/target");
        let c = dir.path().join("c/target");
        std::fs::create_dir_all(units.join("svc-b.service.d")).unwrap();
        std::fs::write(
            units.join("svc-a.service"),
            format!(
                "[Service]\nExecStart=-{}/release/svcd --serve\nWorkingDirectory=/\n",
                a.display()
            ),
        )
        .unwrap();
        std::fs::write(
            units.join("svc-b.service.d/override.conf"),
            format!("[Service]\nExecStartPre={}/release/prep\n", b.display()),
        )
        .unwrap();
        let used = InUse::scan_from(
            dir.path().join("no-proc").as_path(),
            std::slice::from_ref(&units),
        );
        assert_eq!(
            used.why(&a),
            Some(
                format!(
                    "the unit {} starts from it",
                    units.join("svc-a.service").display()
                )
                .as_str()
            )
        );
        assert!(
            used.why(&b).is_some_and(|w| w.contains("override.conf")),
            "{:?}",
            used.why(&b)
        );
        assert_eq!(used.why(&c), None);
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    fn tagged(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("CACHEDIR.TAG"), format!("{CACHEDIR_SIGNATURE}\n")).unwrap();
    }

    #[test]
    fn the_dry_run_follows_the_state_dir_file_over_the_flag() {
        let d = tempfile::tempdir().unwrap();
        assert!(!dry_run(d.path(), false), "no file: the flag");
        assert!(dry_run(d.path(), true), "no file: the flag");
        std::fs::write(d.path().join(FILE), r#"{"dry_run": true}"#).unwrap();
        assert!(dry_run(d.path(), false));
        std::fs::write(d.path().join(FILE), r#"{"dry_run": false}"#).unwrap();
        assert!(!dry_run(d.path(), true));
        std::fs::write(d.path().join(FILE), "{}").unwrap();
        assert!(dry_run(d.path(), true), "field left out: the flag");
        std::fs::write(d.path().join(FILE), "not json").unwrap();
        assert!(!dry_run(d.path(), false), "unreadable: the flag");
    }

    #[test]
    fn a_dry_run_under_the_floor_says_eviction_is_off_and_what_it_would_free() {
        let e = |path: &str, bytes| Eviction {
            path: path.into(),
            bytes,
            last_ms: 0,
            action: "would delete".into(),
        };
        let done = [e("/w/a/target", 20 * GB), e("/w/b/target", 11 * GB)];
        assert_eq!(
            off_summary(32 * GB, 40 * GB, 0, &done),
            "disk: 32.0 GB free, under 40.0 GB: eviction is off (dry run), \
             would free 31.0 GB in 2 dir(s)"
        );
        assert_eq!(
            off_summary(39 * GB, 40 * GB, 0, &[]),
            "disk: 39.0 GB free, under 40.0 GB: eviction is off (dry run), \
             would free 0.0 GB in 0 dir(s)"
        );
        assert_eq!(
            off_summary(45 * GB, 50 * GB, 40 * GB, &done),
            "disk: 45.0 GB free, under 50.0 GB, raised from 40.0 GB to the disk hold's mark: \
             eviction is off (dry run), would free 31.0 GB in 2 dir(s)"
        );
    }

    #[test]
    fn the_mark_is_the_larger_of_evict_below_and_the_disk_hold_and_0_stays_off() {
        assert_eq!(mark(40 * GB, 50 * GB), 50 * GB, "the disk hold raises it");
        assert_eq!(mark(40 * GB, 25 * GB), 40 * GB);
        assert_eq!(mark(40 * GB, 0), 40 * GB, "no disk hold");
        assert_eq!(mark(0, 50 * GB), 0, "--evict-below 0 is off");
        assert_eq!(mark(0, 0), 0);
        assert_eq!(mark_text(40 * GB, 0), "40.0 GB");
        assert_eq!(
            mark_text(50 * GB, 40 * GB),
            "50.0 GB, raised from 40.0 GB to the disk hold's mark"
        );
    }

    #[test]
    fn the_target_dir_follows_flags_env_and_the_workspace_root() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"a\"]\n").unwrap();
        std::fs::create_dir_all(root.join("a/src")).unwrap();
        std::fs::write(root.join("a/Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        let a = root.join("a");
        assert_eq!(
            target_dir(&s(&["cargo", "build"]), &[], &a),
            Some(root.join("target"))
        );
        assert_eq!(
            target_dir(&s(&["cargo", "build", "--target-dir", "../t"]), &[], &a),
            Some(root.join("t"))
        );
        assert_eq!(
            target_dir(&s(&["cargo", "test"]), &s(&["CARGO_TARGET_DIR=/x/y/z"]), &a),
            Some(PathBuf::from("/x/y/z"))
        );
        assert_eq!(
            target_dir(
                &s(&["cargo", "check", "--manifest-path", "a/Cargo.toml"]),
                &s(&["CARGO_TARGET_DIR="]),
                root
            ),
            Some(root.join("target"))
        );
        let lone = tempfile::tempdir().unwrap();
        assert_eq!(target_dir(&s(&["cargo", "build"]), &[], lone.path()), None);
    }

    #[test]
    fn only_tagged_deep_directories_count() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("repo/target");
        std::fs::create_dir_all(&t).unwrap();
        assert!(!is_target_dir(&t), "no CACHEDIR.TAG");
        tagged(&t);
        assert!(is_target_dir(&t));
        assert!(!is_target_dir(Path::new("/tmp")));
        assert!(!is_target_dir(Path::new("relative/target/x")));
        let link = d.path().join("repo/link");
        std::os::unix::fs::symlink(&t, &link).unwrap();
        assert!(!is_target_dir(&link), "a symlink is not deleted through");
    }

    #[test]
    fn candidates_skip_busy_and_recent_and_sort_oldest_first() {
        let h = 3_600_000;
        let now = 100 * h;
        let mut seen = HashMap::new();
        seen.insert(
            PathBuf::from("/w/old/target"),
            ("old".to_string(), now - 50 * h),
        );
        seen.insert(
            PathBuf::from("/w/mid/target"),
            ("mid".to_string(), now - 10 * h),
        );
        seen.insert(PathBuf::from("/w/new/target"), ("new".to_string(), now - h));
        seen.insert(
            PathBuf::from("/w/busy/target"),
            ("busy".to_string(), now - 90 * h),
        );
        seen.insert(
            PathBuf::from("/w/dir/target"),
            ("dir".to_string(), now - 80 * h),
        );
        let cwds: HashSet<PathBuf> = [PathBuf::from("/w/busy/crates/a")].into();
        let dirs: HashSet<PathBuf> = [PathBuf::from("/w/dir/target")].into();
        let c = candidates(&seen, &dirs, &cwds, now, Duration::from_secs(7200));
        let names: Vec<&str> = c.iter().map(|c| c.repo.as_str()).collect();
        assert_eq!(names, ["old", "mid"]);
    }

    /// A job keeps the target directory it builds into and the one of the
    /// checkout it runs in; another checkout of the same repository, idle,
    /// may go.
    #[test]
    fn a_job_keeps_its_own_checkout_and_target_dir_not_its_whole_repository() {
        let main = PathBuf::from("/w/repo/target");
        let tree = PathBuf::from("/w/trees/repo-a/target");
        let idle = PathBuf::from("/w/trees/repo-b/target");
        let shared = PathBuf::from("/w/shared/repo-c");
        let seen = HashMap::from([
            (main.clone(), ("repo".to_string(), 1)),
            (tree.clone(), ("repo".to_string(), 1)),
            (idle.clone(), ("repo".to_string(), 1)),
            (shared.clone(), ("repo".to_string(), 1)),
        ]);
        let dirs = HashSet::from([shared.clone()]);
        let cwds = HashSet::from([PathBuf::from("/w/trees/repo-a"), PathBuf::from("/w/repo")]);
        let left: Vec<PathBuf> = candidates(&seen, &dirs, &cwds, 2 * 3_600_000, Duration::ZERO)
            .into_iter()
            .map(|c| c.path)
            .collect();
        assert_eq!(
            left,
            std::slice::from_ref(&idle),
            "only the idle checkout's goes"
        );
        assert!(job_keeps(&main, Path::new(""), Path::new("/w/repo/src")));
        assert!(job_keeps(&shared, &shared, Path::new("/elsewhere")));
        assert!(!job_keeps(&idle, &tree, Path::new("/w/trees/repo-a")));
        assert!(
            !job_keeps(&idle, Path::new(""), Path::new("/w/trees/repo-bb")),
            "a sibling whose name starts the same is another checkout"
        );
        assert!(!job_keeps(&idle, Path::new(""), Path::new("")));
    }

    /// A queued or running job keeps every target directory in its tree
    /// (git's top level, else its working directory), nested ones too, and
    /// the one its environment names, with what lies above and below it.
    #[test]
    fn a_job_keeps_every_target_dir_in_its_tree_and_the_one_its_env_names() {
        let d = tempfile::tempdir().unwrap();
        let u = d.path().join("srv/u");
        let tree = u.join("tree");
        std::fs::create_dir_all(tree.join(".git")).unwrap();
        std::fs::create_dir_all(tree.join("crates/a")).unwrap();
        let plain = u.join("plain");
        std::fs::create_dir_all(plain.join("sub")).unwrap();
        let nested = tree.join("target/ignite-test/rt");
        let top = tree.join("target");
        let driver = tree.join("driver/target");
        let in_plain = plain.join("sub/target");
        let shared = u.join("shared/target");
        let shared_rt = shared.join("ignite-test/rt");
        let other = u.join("other/target");
        let seen: HashMap<PathBuf, (String, i64)> = [
            &nested, &top, &driver, &in_plain, &shared, &shared_rt, &other,
        ]
        .map(|p| (p.clone(), ("r".to_string(), 1)))
        .into();
        let cwd = u.join("elsewhere");
        let env_t = env_target(
            &s(&["CARGO_TARGET_DIR=../shared/target/ignite-test/rt"]),
            &cwd,
        )
        .expect("the environment names a target");
        assert_eq!(env_t, shared_rt);
        let dirs = HashSet::from([env_t.clone()]);
        let cwds = HashSet::from([tree.join("crates/a"), plain.clone()]);
        let left: Vec<PathBuf> = candidates(&seen, &dirs, &cwds, 100 * 3_600_000, Duration::ZERO)
            .into_iter()
            .map(|c| c.path)
            .collect();
        assert_eq!(left, std::slice::from_ref(&other));
        let a = tree.join("crates/a");
        for kept in [&nested, &top, &driver] {
            assert!(job_keeps(kept, Path::new(""), &a), "{}", kept.display());
        }
        assert!(job_keeps(&in_plain, Path::new(""), &plain));
        assert!(
            job_keeps(&shared, &env_t, &cwd),
            "it holds the job's target"
        );
        assert!(
            job_keeps(&shared_rt, &shared, &cwd),
            "it lies in the job's target"
        );
        assert!(!job_keeps(&other, &env_t, &a));
        assert_eq!(env_target(&s(&["CARGO_TARGET_DIR="]), &cwd), None);

        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(h.path().join(".git")).unwrap();
        let sub = h.path().join("r/sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(tree_under(&sub, None), h.path());
        assert_eq!(
            tree_under(&sub, Some(h.path())),
            sub,
            "a repository of the home directory is not a tree"
        );
    }

    /// A target directory with a `.cargo-lock` a process holds locked,
    /// anywhere in it, is in use; one whose lock nobody holds is not.
    #[test]
    fn a_target_whose_cargo_lock_is_held_is_in_use() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("srv/u/tree/target");
        let lock = t.join("ignite-test/rt/debug/.cargo-lock");
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(&lock, b"").unwrap();
        let idle = d.path().join("srv/u/idle/target");
        std::fs::create_dir_all(idle.join("debug")).unwrap();
        std::fs::write(idle.join("debug/.cargo-lock"), b"").unwrap();
        let ino = std::fs::metadata(&lock).unwrap().ino();

        let f = std::fs::File::open(&lock).unwrap();
        f.lock().unwrap();
        let held = locked_inodes(&std::fs::read_to_string("/proc/locks").unwrap());
        f.unlock().unwrap();
        drop(f);
        assert!(held.contains(&ino), "/proc/locks names the held lock");

        let proc_root = d.path().join("proc");
        std::fs::create_dir_all(&proc_root).unwrap();
        std::fs::write(
            proc_root.join("locks"),
            format!(
                "1: FLOCK  ADVISORY  WRITE 4242 00:2a:{ino} 0 EOF\n\
                 1: -> FLOCK  ADVISORY  WRITE 4343 00:2a:{ino} 0 EOF\n\
                 2: POSIX  ADVISORY  WRITE 99 fd:01:7 0 EOF\n"
            ),
        )
        .unwrap();
        let used = InUse::scan_from(&proc_root, &[]);
        assert_eq!(used.why(&t), Some("a cargo holds a .cargo-lock in it"));
        assert_eq!(used.why(&idle), None);
        std::fs::write(proc_root.join("locks"), "").unwrap();
        assert_eq!(InUse::scan_from(&proc_root, &[]).why(&t), None);
    }

    /// A tree any job ran in within the hour keeps all its target
    /// directories, whatever the idle keep; an hour later they may go.
    #[test]
    fn a_tree_a_job_ran_in_within_the_hour_keeps_its_target_dirs() {
        let d = tempfile::tempdir().unwrap();
        let tree = d.path().join("srv/u/tree");
        std::fs::create_dir_all(tree.join(".git")).unwrap();
        std::fs::create_dir_all(tree.join("crates/a")).unwrap();
        let h = 3_600_000;
        let now = 100 * h;
        let nested = tree.join("target/ignite-test/rt");
        let top = tree.join("target");
        let other = d.path().join("srv/u/other/target");
        let seen: HashMap<PathBuf, (String, i64)> = [&nested, &top, &other]
            .map(|p| (p.clone(), ("r".to_string(), now - 10 * h)))
            .into();
        let left = |ran_ago: i64| -> Vec<PathBuf> {
            let ran = HashMap::from([(tree.join("crates/a"), now - ran_ago)]);
            let cwds = ran_recently(&ran, now);
            candidates(&seen, &HashSet::new(), &cwds, now, Duration::ZERO)
                .into_iter()
                .map(|c| c.path)
                .collect()
        };
        assert_eq!(left(59 * 60_000), std::slice::from_ref(&other));
        let mut all = left(61 * 60_000);
        all.sort();
        let mut want = vec![nested.clone(), top.clone(), other.clone()];
        want.sort();
        assert_eq!(all, want);
        let built: HashMap<PathBuf, (String, i64)> =
            HashMap::from([(other.clone(), ("r".to_string(), now - 59 * 60_000))]);
        assert!(
            candidates(
                &built,
                &HashSet::new(),
                &HashSet::new(),
                now,
                Duration::ZERO
            )
            .is_empty(),
            "a target built into within the hour stays whatever the idle keep"
        );
    }

    #[test]
    fn unnamed_repositories_are_protected_by_their_active_target_directory() {
        let active = PathBuf::from("/w/active/target");
        let idle = PathBuf::from("/w/idle/target");
        let seen = HashMap::from([
            (active.clone(), (String::new(), 1)),
            (idle.clone(), (String::new(), 1)),
        ]);
        let busy_dirs = HashSet::from([active]);

        let candidates = candidates(
            &seen,
            &busy_dirs,
            &HashSet::new(),
            2 * 3_600_000,
            Duration::ZERO,
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].path, idle);
    }

    /// The mark set at run time wins over `--evict-below`, 0 turns eviction
    /// off, `default` forgets it, and the disk hold still raises it.
    #[test]
    fn the_run_time_mark_replaces_the_option_and_the_disk_hold_still_raises_it() {
        assert_eq!(below(None, 40 * GB), 40 * GB, "unset: the option");
        assert_eq!(below(Some(100.0), 40 * GB), 100 * GB);
        assert_eq!(below(Some(0.0), 40 * GB), 0, "0 is off");
        assert_eq!(below(Some(10.0), 0), 10 * GB, "on over an option of 0");
        assert_eq!(mark(below(Some(30.0), 40 * GB), 50 * GB), 50 * GB);
        assert_eq!(mark(below(Some(0.0), 40 * GB), 50 * GB), 0);
        let v = BelowView::of(Some(100.0), 40 * GB, 25 * GB);
        assert_eq!(v.floor, 100 * GB);
        assert_eq!(
            v.render(),
            "eviction starts below 100.0 GB free (set with `slot evict-below`; \
             `--evict-below` is 40.0 GB)\n"
        );
        assert_eq!(
            BelowView::of(None, 40 * GB, 50 * GB).render(),
            "eviction starts below 50.0 GB free, raised from 40.0 GB to the disk hold's mark \
             (`--min-free-disk`) (`--evict-below`)\n"
        );
        assert_eq!(
            BelowView::of(Some(0.0), 40 * GB, 25 * GB).render(),
            "eviction is off (set with `slot evict-below`; `--evict-below` is 40.0 GB)\n"
        );
    }

    #[test]
    fn the_argument_of_evict_below_is_gb_or_default() {
        assert_eq!("100".parse(), Ok(BelowSetting::At(100.0)));
        assert_eq!("2.5".parse(), Ok(BelowSetting::At(2.5)));
        assert_eq!("0".parse(), Ok(BelowSetting::At(0.0)));
        assert_eq!("default".parse(), Ok(BelowSetting::Default));
        assert_eq!(BelowSetting::Default.value(), None);
        assert_eq!(BelowSetting::At(7.0).value(), Some(7.0));
        for bad in ["-1", "100001", "NaN", "inf", "ten", "", "off"] {
            assert!(bad.parse::<BelowSetting>().is_err(), "{bad:?}");
        }
        assert!(
            "100001"
                .parse::<BelowSetting>()
                .unwrap_err()
                .contains("over the most")
        );
        let setting = |body: &str| {
            serde_json::from_str::<BelowRequest>(body)
                .map_err(|e| e.to_string())
                .and_then(BelowRequest::checked)
                .map(|r| (r.evict_below_gb, r.evict_idle_s))
        };
        assert_eq!(
            setting(r#"{"evict_below_gb":100}"#),
            Ok((Some(Some(100.0)), None)),
            "the idle keep is left as it is"
        );
        assert_eq!(
            setting(r#"{"evict_below_gb":null}"#),
            Ok((Some(None), None))
        );
        assert_eq!(
            setting(r#"{"evict_idle_s":1800}"#),
            Ok((None, Some(Some(1800))))
        );
        assert_eq!(
            setting(r#"{"evict_below_gb":null,"evict_idle_s":null}"#),
            Ok((Some(None), Some(None)))
        );
        for bad in [
            "{}",
            r#"{"evict_below_gb":-1}"#,
            r#"{"evict_below_gb":"9"}"#,
            r#"{"evict_idle_s":299}"#,
            r#"{"evict_idle_s":604801}"#,
            r#"{"evict_below_gb":50,"evict_idle_s":-1}"#,
        ] {
            assert!(setting(bad).is_err(), "{bad}");
        }
        let body = |r: BelowRequest| serde_json::to_string(&r).unwrap();
        assert_eq!(
            body(BelowRequest {
                evict_below_gb: Some(Some(100.0)),
                evict_idle_s: None,
            }),
            r#"{"evict_below_gb":100.0}"#,
            "an older daemon reads it"
        );
        assert_eq!(
            body(BelowRequest {
                evict_below_gb: Some(None),
                evict_idle_s: Some(None),
            }),
            r#"{"evict_below_gb":null,"evict_idle_s":null}"#
        );
        let of = |m, i| BelowRequest::of(m, i).map(|r| (r.evict_below_gb, r.evict_idle_s));
        assert_eq!(of(None, None), None, "no arguments: only read");
        assert_eq!(
            of(Some(BelowSetting::At(100.0)), None),
            Some((Some(Some(100.0)), None))
        );
        assert_eq!(
            of(Some(BelowSetting::At(100.0)), Some(1800)),
            Some((Some(Some(100.0)), Some(Some(1800))))
        );
        assert_eq!(of(None, Some(1800)), Some((None, Some(Some(1800)))));
        assert_eq!(
            of(Some(BelowSetting::Default), None),
            Some((Some(None), Some(None))),
            "`default` forgets both"
        );
    }

    /// The idle keep set at run time replaces the 2 h default, from 5 min
    /// to 7 days, and `slot evict-below` names it.
    #[test]
    fn the_idle_keep_set_at_run_time_replaces_the_default() {
        assert_eq!(min_idle(None, DEFAULT_MIN_IDLE), Duration::from_secs(7200));
        assert_eq!(
            min_idle(Some(1800), DEFAULT_MIN_IDLE),
            Duration::from_secs(1800)
        );
        assert_eq!(parse_idle("30m"), Ok(1800));
        assert_eq!(parse_idle("5m"), Ok(300));
        assert_eq!(parse_idle("168h"), Ok(MAX_IDLE_S));
        for bad in ["4m", "169h", "0", "soon", "default"] {
            assert!(parse_idle(bad).is_err(), "{bad}");
        }
        let h = 3_600_000;
        let now = 10 * h;
        let seen: HashMap<PathBuf, (String, i64)> = [
            (PathBuf::from("/w/a/target"), ("a".to_string(), now - h)),
            (PathBuf::from("/w/b/target"), ("b".to_string(), now - h / 4)),
        ]
        .into();
        let none = HashSet::new();
        let names = |idle| -> Vec<String> {
            candidates(&seen, &none, &none, now, idle)
                .into_iter()
                .map(|c| c.repo)
                .collect()
        };
        assert!(names(DEFAULT_MIN_IDLE).is_empty(), "both built in 2 h");
        assert_eq!(names(min_idle(Some(1800), DEFAULT_MIN_IDLE)), ["a"]);
        assert_eq!(
            names(min_idle(Some(300), DEFAULT_MIN_IDLE)),
            ["a"],
            "an idle keep under an hour still keeps what was built in the last hour"
        );
        let v = BelowView::of(Some(100.0), 40 * GB, 25 * GB);
        assert_eq!(
            v.with_idle(None, DEFAULT_MIN_IDLE).render(),
            "eviction starts below 100.0 GB free (set with `slot evict-below`; \
             `--evict-below` is 40.0 GB)\n\
             eviction keeps target dirs built in the last 2h (the default)\n"
        );
        assert_eq!(
            v.with_idle(Some(1800), DEFAULT_MIN_IDLE).render(),
            "eviction starts below 100.0 GB free (set with `slot evict-below`; \
             `--evict-below` is 40.0 GB)\n\
             eviction keeps target dirs built in the last 30m (set with \
             `slot evict-below --idle`)\n"
        );
    }

    #[test]
    fn the_plan_deletes_oldest_until_floor_plus_margin() {
        let st = Settings {
            floor: 40 * GB,
            margin: 10 * GB,
            min_idle: Duration::from_secs(7200),
            dry_run: true,
        };
        let c = |n: &str, ms| Candidate {
            path: PathBuf::from(n),
            repo: n.into(),
            last_ms: ms,
        };
        let cands = vec![
            (c("a", 1), 8 * GB),
            (c("b", 2), 0),
            (c("c", 3), 6 * GB),
            (c("d", 4), 30 * GB),
        ];
        assert!(plan(41 * GB, st, &cands).is_empty(), "above the floor");
        let p: Vec<&str> = plan(35 * GB, st, &cands)
            .iter()
            .map(|x| x.0.repo.as_str())
            .collect();
        assert_eq!(
            p,
            ["a", "c", "d"],
            "35 + 8 + 6 = 49 < 50, so d too; b is empty"
        );
        let p: Vec<&str> = plan(39 * GB, st, &cands)
            .iter()
            .map(|x| x.0.repo.as_str())
            .collect();
        assert_eq!(p, ["a", "c"]);
        assert!(
            plan(1, Settings { floor: 0, ..st }, &cands).is_empty(),
            "off"
        );
    }

    #[test]
    fn remove_refuses_an_untagged_directory_and_deletes_a_tagged_one() {
        let d = tempfile::tempdir().unwrap();
        let plain = d.path().join("repo/src");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(remove(&plain).is_err());
        assert!(plain.exists());
        let t = d.path().join("repo/target");
        tagged(&t);
        std::fs::write(t.join("big"), vec![0u8; 100_000]).unwrap();
        assert!(dir_size(&t) >= 100_000);
        assert_eq!(remove(&t), Ok("deleted"));
        assert!(!t.exists());
        assert!(plain.exists());
    }
}
