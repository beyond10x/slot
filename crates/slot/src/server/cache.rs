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
//!   again. Only a reflink by default: on ext4 a plain copy of a 4 GB target
//!   directory took longer than the compile time it saved (`docs/design.md`
//!   § 16); `slot serve --seed-copy copy` takes a byte copy where the
//!   filesystem has no reflinks. Never hard links: the copy and the sibling
//!   share no file. cargo decides freshness by modification time, so after
//!   the copy every source file of the new mirror that is byte for byte the
//!   sibling's takes the sibling's time, and every other one a time after
//!   every file of the copy: cargo then rebuilds exactly the crates whose
//!   sources differ from the sibling's, and nothing it would not rebuild in
//!   the sibling's tree.
//! - **Eviction.** Before a remote job, target directories and their source
//!   mirrors are deleted, least recently used first, while the cache is over
//!   `--cache-budget` or the filesystem has less than `--min-free-disk` free.
//!   A workspace with a queued or running job is never deleted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

/// How a new workspace's target directory is copied from its sibling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum SeedCopy {
    /// A reflink copy only; no seed where the filesystem has no reflinks.
    #[default]
    Reflink,
    /// A reflink copy where the filesystem has reflinks, else a byte copy.
    Copy,
}

/// What [`Cache::seed`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seeded {
    /// The workspace id the target directory was copied from.
    pub from: String,
    /// Wall time of the copy and of setting the source times.
    pub took: Duration,
    /// Source files of the new mirror whose content differs from the
    /// sibling's (or that it lacks): each is now newer than every file of
    /// the copy.
    pub differ: usize,
}

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

    /// The target directory [`Cache::seed`] copies for a new workspace `id`
    /// of `repo`: the most recently used one of the repository that no job
    /// uses (`busy`).
    pub fn seed_source(&self, id: &str, repo: &str, busy: &HashSet<String>) -> Option<Entry> {
        self.entries()
            .into_iter()
            .filter(|e| {
                e.id != id
                    && e.meta.repo == repo
                    && !busy.contains(&e.id)
                    && self.dir(&e.id).is_dir()
            })
            .max_by_key(|e| e.meta.last_used_ms)
    }

    /// Seeds `id`'s target directory from the most recently used target
    /// directory of the same repository that no job uses (`busy`), by a
    /// copy of kind `how`, then sets the times of `id`'s source mirror
    /// ([`align_sources`]) so cargo rebuilds every crate whose sources
    /// differ from the sibling's. Returns what it did, or the reason nothing
    /// was seeded. Does nothing when `id` already has one.
    pub fn seed(
        &self,
        id: &str,
        repo: &str,
        busy: &HashSet<String>,
        how: SeedCopy,
    ) -> Result<Seeded, String> {
        if repo.is_empty() {
            return Err("no repository name".into());
        }
        if self.dir(id).exists() {
            return Err("target directory exists".into());
        }
        let Some(src) = self.seed_source(id, repo, busy) else {
            return Err(format!("no other target directory of {repo}"));
        };
        let tmp = self.targets.join(format!(".{id}.seed"));
        let _ = std::fs::remove_dir_all(&tmp);
        let t0 = Instant::now();
        // `-a` keeps hard links among the copied files (rustc links a
        // crate's `.rmeta` into its incremental directory) inside the copy;
        // no file of the copy is a link to a file of the sibling.
        let out = Command::new("cp")
            .arg("-a")
            .arg(match how {
                SeedCopy::Reflink => "--reflink=always",
                SeedCopy::Copy => "--reflink=auto",
            })
            .arg(self.dir(&src.id))
            .arg(&tmp)
            .output()
            .map_err(|e| format!("cp: {e}"))?;
        if !out.status.success() {
            let _ = std::fs::remove_dir_all(&tmp);
            let msg = String::from_utf8_lossy(&out.stderr);
            let first = msg.lines().next().unwrap_or("cp failed").trim();
            return Err(match how {
                SeedCopy::Reflink => format!("no reflink copy ({first})"),
                SeedCopy::Copy => format!("no copy ({first})"),
            });
        }
        let differ = align_sources(&self.ws_root.join(id), &self.ws_root.join(&src.id), &tmp);
        std::fs::rename(&tmp, self.dir(id)).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp);
            format!("rename: {e}")
        })?;
        let mut mine = self.meta(id);
        mine.repo = repo.to_string();
        mine.bytes = src.meta.bytes;
        self.write_meta(id, &mine);
        Ok(Seeded {
            from: src.id,
            took: t0.elapsed(),
            differ,
        })
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
            crate::mirror::forget(&self.ws_root, &e.id);
            crate::cargo_fetch::forget(&self.ws_root, &e.id);
            total = total.saturating_sub(e.meta.bytes);
            gone.push((e.id, e.meta.bytes));
        }
        gone
    }
}

/// Sets the modification times of source mirror `mirror` after its target
/// directory was seeded into `copy` from the sibling whose mirror is
/// `sibling`. cargo takes a crate as built when no source file is newer
/// than its last build, so:
///
/// - a regular file that the sibling's mirror holds with the same bytes
///   takes the sibling's file's time: cargo decides on it here as it would
///   in the sibling's tree, against the same build times;
/// - every other regular file takes a time after every entry of `copy`, so
///   each crate that holds one is built again.
///
/// Symbolic links are left alone. Returns the count of the second kind.
pub fn align_sources(mirror: &Path, sibling: &Path, copy: &Path) -> usize {
    let now = SystemTime::now();
    let after = match newest(copy) {
        Some(n) if n >= now => n + Duration::from_millis(1),
        _ => now,
    };
    let mut differ = 0;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(mirror.join(&rel)) else {
            continue;
        };
        for e in rd.flatten() {
            let rel = rel.join(e.file_name());
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                stack.push(rel);
            } else if kind.is_file() {
                let path = mirror.join(&rel);
                let t = same_bytes(&path, &sibling.join(&rel)).unwrap_or_else(|| {
                    differ += 1;
                    after
                });
                let _ = std::fs::File::open(&path).and_then(|f| f.set_modified(t));
            }
        }
    }
    differ
}

/// The newest modification time of `dir` and everything under it,
/// symbolic links not followed.
fn newest(dir: &Path) -> Option<SystemTime> {
    let mut max = std::fs::symlink_metadata(dir).ok()?.modified().ok();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            }
            if let Ok(t) = m.modified() {
                max = max.max(Some(t));
            }
        }
    }
    max
}

/// The modification time of `theirs` when it is a regular file with the
/// bytes of `mine`.
fn same_bytes(mine: &Path, theirs: &Path) -> Option<SystemTime> {
    use std::io::Read as _;
    let meta = std::fs::symlink_metadata(theirs)
        .ok()
        .filter(std::fs::Metadata::is_file)?;
    if std::fs::metadata(mine).ok()?.len() != meta.len() {
        return None;
    }
    let mut ours = std::fs::File::open(mine).ok()?;
    let mut sibling = std::fs::File::open(theirs).ok()?;
    let mut ours_buf = vec![0u8; 64 << 10];
    let mut sibling_buf = vec![0u8; 64 << 10];
    loop {
        let got = ours.read(&mut ours_buf).ok()?;
        if got == 0 {
            return (sibling.read(&mut sibling_buf[..1]).ok()? == 0)
                .then(|| meta.modified().ok())
                .flatten();
        }
        sibling.read_exact(&mut sibling_buf[..got]).ok()?;
        if ours_buf[..got] != sibling_buf[..got] {
            return None;
        }
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
        match c.seed("fresh", "r", &busy, SeedCopy::Reflink) {
            Ok(s) => {
                assert_eq!(s.from, "new");
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
            c.seed("new", "r", &busy, SeedCopy::Copy).is_err(),
            "existing dir is never replaced"
        );
        assert!(c.seed("x", "nope", &busy, SeedCopy::Copy).is_err());
    }

    fn set_mtime(p: &Path, t: SystemTime) {
        std::fs::File::open(p).unwrap().set_modified(t).unwrap();
    }

    fn write_at(p: &Path, body: &str, t: SystemTime) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
        set_mtime(p, t);
    }

    fn mtime(p: &Path) -> SystemTime {
        std::fs::metadata(p).unwrap().modified().unwrap()
    }

    /// (device, inode) and modification time of every regular file under
    /// `dir`.
    fn files(dir: &Path) -> Vec<((u64, u64), SystemTime)> {
        use std::os::unix::fs::MetadataExt;
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let m = e.path().symlink_metadata().unwrap();
                if m.is_dir() {
                    stack.push(e.path());
                } else if m.is_file() {
                    out.push(((m.dev(), m.ino()), m.modified().unwrap()));
                }
            }
        }
        out
    }

    #[test]
    fn a_seeded_target_is_a_copy_that_shares_no_file_with_its_sibling() {
        let (_d, c) = cache();
        make(&c, "old", "r", 1, 10);
        // rustc links a crate's `.rmeta` into its incremental directory.
        std::fs::hard_link(
            c.dir("old").join("debug/x"),
            c.dir("old").join("debug/x.rmeta"),
        )
        .unwrap();
        let s = c.seed("new", "r", &HashSet::new(), SeedCopy::Copy).unwrap();
        assert_eq!(s.from, "old");
        let theirs: Vec<(u64, u64)> = files(&c.dir("old")).into_iter().map(|f| f.0).collect();
        let mine = files(&c.dir("new"));
        assert_eq!(mine.len(), 2, "{mine:?}");
        assert!(
            mine.iter().all(|(i, _)| !theirs.contains(i)),
            "a file of the copy is a hard link into the sibling's"
        );
        assert_eq!(
            std::fs::read(c.dir("new").join("debug/x.rmeta")).unwrap(),
            b"x"
        );
    }

    #[test]
    fn after_a_seed_only_sources_that_differ_from_the_siblings_are_newer_than_the_copy() {
        let (_d, c) = cache();
        make(&c, "old", "r", 1, 10);
        let now = SystemTime::now();
        let h = Duration::from_secs(3600);
        let (theirs, mine) = (c.ws_root.join("old/src"), c.ws_root.join("new/src"));
        // The sibling's sources, then its build.
        write_at(&theirs.join("same.rs"), "same", now - 3 * h);
        write_at(&theirs.join("changed.rs"), "v1", now - 3 * h);
        write_at(&theirs.join("sub/deep.rs"), "deep", now - 3 * h);
        set_mtime(&c.dir("old").join("debug/x"), now - 2 * h);
        // The new mirror, with older times than the sibling's build.
        write_at(&mine.join("same.rs"), "same", now - 4 * h);
        write_at(&mine.join("changed.rs"), "v2", now - 4 * h);
        write_at(&mine.join("sub/deep.rs"), "deep", now - 4 * h);
        write_at(&mine.join("added.rs"), "new", now - 4 * h);
        std::os::unix::fs::symlink("same.rs", mine.join("link.rs")).unwrap();

        let s = c.seed("new", "r", &HashSet::new(), SeedCopy::Copy).unwrap();
        assert_eq!(s.differ, 2, "{s:?}");
        for f in ["same.rs", "sub/deep.rs"] {
            assert_eq!(mtime(&mine.join(f)), mtime(&theirs.join(f)), "{f}");
        }
        let copy = files(&c.dir("new"));
        for f in ["changed.rs", "added.rs"] {
            let t = mtime(&mine.join(f));
            assert!(
                copy.iter().all(|(_, m)| *m < t),
                "{f} at {t:?} is not newer than the copy: {copy:?}"
            );
        }
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
