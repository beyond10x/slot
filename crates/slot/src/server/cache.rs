//! Target directories of synced workspaces on a build server.
//!
//! Every remote workspace id gets `<cache-root>/target/<id>` as its
//! `CARGO_TARGET_DIR`, and `<cache-root>/target/<id>.json` records which
//! repository it belongs to, when a job last used it and how large it was
//! after that job. Two things use the record:
//!
//! - **Seeding.** A workspace id with no target directory yet (a new
//!   worktree) starts from a reflink copy of the most recently used target
//!   directory of the same repository, so dependencies are not compiled
//!   again. Only a reflink: on ext4 a plain copy of a 4 GB target directory
//!   took longer than the compile time it saved (`docs/design.md` § 16).
//! - **Eviction.** Before a remote job, target directories and their source
//!   mirrors are deleted, least recently used first, while the cache is over
//!   `--cache-budget` or the filesystem has less than `--min-free-disk` free.
//!   A workspace with a queued or running job is never deleted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// What is recorded per workspace id.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    /// Repository name (the same for every worktree of a repository).
    #[serde(default)]
    pub repo: String,
    /// When a job last started in this workspace, ms since the epoch.
    #[serde(default)]
    pub last_used_ms: i64,
    /// Bytes on disk of the target directory after its last job.
    #[serde(default)]
    pub bytes: u64,
}

/// The cache root's `target` directory and the workspace root.
#[derive(Debug, Clone)]
pub struct Cache {
    /// `<cache-root>/target`.
    pub targets: PathBuf,
    /// The workspace root (source mirrors).
    pub ws_root: PathBuf,
}

/// One workspace id with its record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Workspace id.
    pub id: String,
    /// Its record.
    pub meta: Meta,
}

impl Cache {
    /// The cache under `cache_root` for mirrors under `ws_root`.
    pub fn new(cache_root: &Path, ws_root: &Path) -> Self {
        Self {
            targets: cache_root.join("target"),
            ws_root: ws_root.to_path_buf(),
        }
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.targets.join(format!("{id}.json"))
    }

    /// The target directory of `id`.
    pub fn dir(&self, id: &str) -> PathBuf {
        self.targets.join(id)
    }

    /// The record of `id`; the default when there is none.
    pub fn meta(&self, id: &str) -> Meta {
        std::fs::read(self.meta_path(id))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write_meta(&self, id: &str, m: &Meta) {
        let _ = std::fs::create_dir_all(&self.targets);
        let tmp = self.targets.join(format!(".{id}.json.tmp"));
        if let Ok(b) = serde_json::to_vec(m)
            && std::fs::write(&tmp, b).is_ok()
        {
            let _ = std::fs::rename(&tmp, self.meta_path(id));
        }
    }

    /// Records that a job of `repo` starts in `id` now.
    pub fn touch(&self, id: &str, repo: &str, now_ms: i64) {
        let mut m = self.meta(id);
        if !repo.is_empty() {
            m.repo = repo.to_string();
        }
        m.last_used_ms = now_ms;
        self.write_meta(id, &m);
    }

    /// Measures `id`'s target directory and records its size.
    pub fn record_size(&self, id: &str) -> u64 {
        let bytes = disk_usage(&self.dir(id));
        let mut m = self.meta(id);
        m.bytes = bytes;
        self.write_meta(id, &m);
        bytes
    }

    /// Every workspace id with a record or a target directory.
    pub fn entries(&self) -> Vec<Entry> {
        let Ok(rd) = std::fs::read_dir(&self.targets) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    return None;
                }
                Some(
                    name.strip_suffix(".json")
                        .map_or(name.clone(), str::to_string),
                )
            })
            .collect();
        ids.sort();
        ids.dedup();
        ids.into_iter()
            .map(|id| Entry {
                meta: self.meta(&id),
                id,
            })
            .collect()
    }

    /// Seeds `id`'s target directory from the most recently used target
    /// directory of the same repository that no job uses (`busy`), by a
    /// reflink copy. Returns the source id and how long it took, or the
    /// reason nothing was seeded. Does nothing when `id` already has one.
    pub fn seed(
        &self,
        id: &str,
        repo: &str,
        busy: &HashSet<String>,
    ) -> Result<(String, Duration), String> {
        if repo.is_empty() {
            return Err("no repository name".into());
        }
        if self.dir(id).exists() {
            return Err("target directory exists".into());
        }
        let Some(src) = self
            .entries()
            .into_iter()
            .filter(|e| {
                e.id != id
                    && e.meta.repo == repo
                    && !busy.contains(&e.id)
                    && self.dir(&e.id).is_dir()
            })
            .max_by_key(|e| e.meta.last_used_ms)
        else {
            return Err(format!("no other target directory of {repo}"));
        };
        let tmp = self.targets.join(format!(".{id}.seed"));
        let _ = std::fs::remove_dir_all(&tmp);
        let t0 = Instant::now();
        let out = Command::new("cp")
            .args(["-a", "--reflink=always"])
            .arg(self.dir(&src.id))
            .arg(&tmp)
            .output()
            .map_err(|e| format!("cp: {e}"))?;
        if !out.status.success() {
            let _ = std::fs::remove_dir_all(&tmp);
            let msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "no reflink copy ({})",
                msg.lines().next().unwrap_or("cp failed").trim()
            ));
        }
        std::fs::rename(&tmp, self.dir(id)).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp);
            format!("rename: {e}")
        })?;
        let mut mine = self.meta(id);
        mine.repo = repo.to_string();
        mine.bytes = src.meta.bytes;
        self.write_meta(id, &mine);
        Ok((src.id, t0.elapsed()))
    }

    /// Deletes target directories and source mirrors, least recently used
    /// first and never one in `keep`, until the recorded total is at most
    /// `budget` (0: no budget) and the filesystem has `min_free` bytes free
    /// (0: no floor). Returns the ids deleted with their recorded bytes.
    pub fn evict(&self, budget: u64, min_free: u64, keep: &HashSet<String>) -> Vec<(String, u64)> {
        self.evict_with(budget, min_free, keep, || {
            crate::admission::disk_free(&self.targets)
        })
    }

    fn evict_with(
        &self,
        budget: u64,
        min_free: u64,
        keep: &HashSet<String>,
        mut free: impl FnMut() -> Option<u64>,
    ) -> Vec<(String, u64)> {
        let mut entries = self.entries();
        entries.sort_by_key(|e| e.meta.last_used_ms);
        let mut total: u64 = entries.iter().map(|e| e.meta.bytes).sum();
        let mut gone = Vec::new();
        for e in entries {
            let over_budget = budget > 0 && total > budget;
            let short = min_free > 0 && free().is_some_and(|f| f < min_free);
            if !over_budget && !short {
                break;
            }
            if keep.contains(&e.id) {
                continue;
            }
            let _ = std::fs::remove_dir_all(self.dir(&e.id));
            let _ = std::fs::remove_file(self.meta_path(&e.id));
            let _ = std::fs::remove_dir_all(self.ws_root.join(&e.id));
            total = total.saturating_sub(e.meta.bytes);
            gone.push((e.id, e.meta.bytes));
        }
        gone
    }
}

/// Bytes on disk under `dir` (allocated blocks, symlinks not followed).
pub fn disk_usage(dir: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            total += m.blocks() * 512;
            if m.is_dir() {
                stack.push(e.path());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (tempfile::TempDir, Cache) {
        let d = tempfile::tempdir().unwrap();
        let c = Cache::new(&d.path().join("cache"), &d.path().join("ws"));
        (d, c)
    }

    fn make(c: &Cache, id: &str, repo: &str, used: i64, bytes: u64) {
        std::fs::create_dir_all(c.dir(id).join("debug")).unwrap();
        std::fs::write(c.dir(id).join("debug/x"), b"x").unwrap();
        std::fs::create_dir_all(c.ws_root.join(id)).unwrap();
        c.write_meta(
            id,
            &Meta {
                repo: repo.into(),
                last_used_ms: used,
                bytes,
            },
        );
    }

    #[test]
    fn evicts_least_recently_used_first_and_keeps_busy_ones() {
        let (_d, c) = cache();
        make(&c, "a", "r", 1, 10);
        make(&c, "b", "r", 2, 10);
        make(&c, "c", "r", 3, 10);
        let keep: HashSet<String> = ["a".to_string()].into();
        let gone = c.evict_with(20, 0, &keep, || None);
        assert_eq!(gone, vec![("b".to_string(), 10)]);
        assert!(c.dir("a").exists());
        assert!(!c.dir("b").exists());
        assert!(!c.ws_root.join("b").exists());
        assert!(c.dir("c").exists());
    }

    #[test]
    fn evicts_for_the_disk_floor_until_enough_is_free() {
        let (_d, c) = cache();
        make(&c, "a", "r", 1, 10);
        make(&c, "b", "r", 2, 10);
        let mut free = 5u64;
        let gone = c.evict_with(0, 12, &HashSet::new(), || {
            let f = free;
            free += 10;
            Some(f)
        });
        assert_eq!(gone, vec![("a".to_string(), 10)]);
        assert!(c.dir("b").exists());
    }

    #[test]
    fn nothing_is_evicted_under_budget_and_floor() {
        let (_d, c) = cache();
        make(&c, "a", "r", 1, 10);
        assert_eq!(
            c.evict_with(100, 1, &HashSet::new(), || Some(50)),
            Vec::new()
        );
        assert_eq!(c.evict_with(0, 0, &HashSet::new(), || Some(0)), Vec::new());
    }

    #[test]
    fn seed_picks_the_latest_idle_dir_of_the_same_repo() {
        let (_d, c) = cache();
        make(&c, "old", "r", 1, 10);
        make(&c, "new", "r", 5, 10);
        make(&c, "busy", "r", 9, 10);
        make(&c, "other", "s", 10, 10);
        let busy: HashSet<String> = ["busy".to_string()].into();
        match c.seed("fresh", "r", &busy) {
            Ok((src, _)) => {
                assert_eq!(src, "new");
                assert!(c.dir("fresh").join("debug/x").exists());
                assert_eq!(c.meta("fresh").repo, "r");
            }
            // The test filesystem may not do reflinks (ext4, tmpfs): then
            // nothing may be left behind.
            Err(e) => {
                assert!(e.contains("reflink"), "{e}");
                assert!(!c.dir("fresh").exists());
                assert!(!c.targets.join(".fresh.seed").exists());
            }
        }
        assert!(
            c.seed("new", "r", &busy).is_err(),
            "existing dir is never replaced"
        );
        assert!(c.seed("x", "nope", &busy).is_err());
    }

    #[test]
    fn touch_and_size_are_recorded() {
        let (_d, c) = cache();
        make(&c, "a", "r", 1, 0);
        c.touch("a", "r", 77);
        assert_eq!(c.meta("a").last_used_ms, 77);
        assert!(c.record_size("a") > 0);
        assert_eq!(c.entries().len(), 1);
    }
}
