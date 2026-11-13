//! Disk-budget eviction of local Cargo target directories.
//!
//! When the filesystem holding target directories has less free space than
//! a floor, the daemon deletes the target directories it saw jobs build
//! into, least recently built first, until free space is above the floor
//! plus a margin. It never deletes a directory whose repository has a job
//! queued or running, one built in the last two hours, or a path that no job
//! of the log built into. A path must carry Cargo's `CACHEDIR.TAG` to count
//! as a target directory. The first release only says what it would delete.

use std::collections::{BTreeMap, HashMap, HashSet};
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

/// What `slot status` and the web UI show about eviction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct Report {
    /// Free bytes below which eviction starts; 0 = off.
    #[ts(type = "number")]
    #[serde(default)]
    pub floor: u64,
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
/// built within `min_idle` of `now_ms`, not of a repository in `busy_repos`
/// and not in `busy_dirs`.
#[allow(clippy::implicit_hasher)]
pub fn candidates(
    seen: &HashMap<PathBuf, (String, i64)>,
    busy_repos: &HashSet<String>,
    busy_dirs: &HashSet<PathBuf>,
    now_ms: i64,
    min_idle: Duration,
) -> Vec<Candidate> {
    let idle_ms = i64::try_from(min_idle.as_millis()).unwrap_or(i64::MAX);
    let mut v: Vec<Candidate> = seen
        .iter()
        .filter(|(p, (repo, last))| {
            now_ms - *last >= idle_ms && !busy_repos.contains(repo) && !busy_dirs.contains(*p)
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

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    fn tagged(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("CACHEDIR.TAG"), format!("{CACHEDIR_SIGNATURE}\n")).unwrap();
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
        let busy: HashSet<String> = ["busy".to_string()].into();
        let dirs: HashSet<PathBuf> = [PathBuf::from("/w/dir/target")].into();
        let c = candidates(&seen, &busy, &dirs, now, Duration::from_secs(7200));
        let names: Vec<&str> = c.iter().map(|c| c.repo.as_str()).collect();
        assert_eq!(names, ["old", "mid"]);
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
