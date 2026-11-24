//! cargo's lock on a build directory, seen from outside.
//!
//! While cargo compiles it holds an exclusive `flock` on a lock file in the
//! profile directory it builds into (`target/debug`, `target/<triple>/release`):
//! `.cargo-build-lock`, or `.cargo-lock` in older releases. A second cargo
//! that builds into the same directory prints "Blocking waiting for file
//! lock on build directory" and sleeps until the first has compiled. cargo
//! gives the lock back when compiling ends, before `cargo test` runs the
//! tests.
//!
//! The daemon reads the lock to keep a job out of a directory it could only
//! wait in: it asks for a shared lock without waiting and gives it back at
//! once. That fails exactly while a process holds the file exclusively, and
//! it sees the lock of a build in a sandbox too, which the job's process
//! tree does not show.

use std::fs::{self, File, TryLockError};
use std::path::Path;

/// The lock files of a profile directory that cargo holds exclusively while
/// it compiles.
const LOCK_FILES: &[&str] = &[".cargo-build-lock", ".cargo-lock"];

/// cargo subcommands that compile, and so take the lock.
const COMPILING: &[&str] = &[
    "build", "b", "check", "c", "clippy", "test", "t", "bench", "run", "r", "doc", "d", "rustc",
    "rustdoc", "fix", "install", "nextest", "llvm-cov", "miri",
];

/// Whether `argv` is a cargo command that compiles: `cargo test`,
/// `cargo +nightly clippy`, `cargo-nextest`. `cargo fmt`, `cargo metadata`
/// and a bare `rustc` take no lock on a build directory.
pub fn compiles(argv: &[String]) -> bool {
    let name = argv
        .first()
        .and_then(|a| Path::new(a).file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let sub = if name == "cargo" {
        crate::hook::cargo_sub(&argv[1..])
    } else {
        name.strip_prefix("cargo-")
    };
    sub.is_some_and(|s| COMPILING.contains(&s))
}

/// Whether a process holds the lock of a profile directory of the target
/// directory `target` now: one of its directories (`debug`, `release`, a
/// custom profile), or one of those below a target triple. The command line
/// of a queued job does not say which profile directory it will build into
/// (flags, variables and cargo's configuration decide), so every one counts.
/// False when `target` does not exist or cannot be read.
pub fn held(target: &Path) -> bool {
    let dirs = |p: &Path| {
        fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
    };
    dirs(target).any(|d| {
        // No lock file: a target triple, with profile directories below.
        profile_held(&d).unwrap_or_else(|| dirs(&d).any(|p| profile_held(&p) == Some(true)))
    })
}

/// Whether a lock file of the directory `dir` is held; `None` when it has
/// no lock file, so it is not a profile directory.
fn profile_held(dir: &Path) -> Option<bool> {
    let mut found = None;
    for name in LOCK_FILES {
        let Ok(f) = File::open(dir.join(name)) else {
            continue;
        };
        if locked(&f) {
            return Some(true);
        }
        found = Some(false);
    }
    found
}

/// Whether another open of `f`'s file holds it exclusively. An error other
/// than "would block" (a filesystem without `flock`) reads as not locked.
fn locked(f: &File) -> bool {
    match f.try_lock_shared() {
        Ok(()) => {
            // Given back explicitly: a child forked at this moment keeps a
            // copy of the descriptor until it execs.
            let _ = f.unlock();
            false
        }
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Error(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_file(dir: &Path, name: &str) -> File {
        fs::create_dir_all(dir).unwrap();
        File::create(dir.join(name)).unwrap()
    }

    #[test]
    fn an_exclusive_lock_in_a_profile_directory_is_seen_until_it_is_given_back() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("target");
        assert!(!held(&target), "a target directory that does not exist");
        let build = lock_file(&target.join("debug"), ".cargo-build-lock");
        fs::create_dir_all(target.join("debug/deps")).unwrap();
        fs::create_dir_all(target.join("tmp")).unwrap();
        assert!(!held(&target), "a lock file nobody holds");
        build.lock().unwrap();
        assert!(held(&target));
        build.unlock().unwrap();
        assert!(!held(&target));
        // What the look takes it gives back: the file can be locked again.
        build.try_lock().unwrap();
    }

    #[test]
    fn the_lock_of_an_older_cargo_and_one_below_a_target_triple_count() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("target");
        let old = lock_file(&target.join("release"), ".cargo-lock");
        old.lock().unwrap();
        assert!(held(&target));
        // A child another test forks shares this open file until its exec;
        // unlock releases the lock for every holder, a drop only for this one.
        old.unlock().unwrap();
        drop(old);
        assert!(!held(&target));
        let cross = lock_file(
            &target.join("aarch64-unknown-linux-gnu/debug"),
            ".cargo-build-lock",
        );
        cross.lock().unwrap();
        assert!(held(&target));
    }

    #[test]
    fn a_shared_lock_and_a_lock_file_elsewhere_do_not_count() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("target");
        let shared = lock_file(&target.join("debug"), ".cargo-lock");
        shared.lock_shared().unwrap();
        assert!(!held(&target), "readers do not block a reader");
        let deep = lock_file(&target.join("a/b/debug"), ".cargo-build-lock");
        deep.lock().unwrap();
        let top = lock_file(&target, ".cargo-build-lock");
        top.lock().unwrap();
        assert!(!held(&target), "only profile directories are read");
    }

    #[test]
    fn only_cargo_commands_that_compile_take_the_lock() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        for yes in [
            "cargo test",
            "cargo +nightly clippy --all-targets",
            "cargo --locked -q check",
            "/usr/bin/cargo build --release",
            "cargo nextest run",
            "cargo-nextest nextest run",
            "cargo-clippy",
            "cargo run -p x",
        ] {
            assert!(compiles(&a(yes)), "{yes}");
        }
        for no in [
            "cargo fmt --check",
            "cargo metadata",
            "cargo",
            "cargo --version",
            "rustc --crate-name x",
            "cargo-fmt",
            "sh -c true",
        ] {
            assert!(!compiles(&a(no)), "{no}");
        }
        assert!(!compiles(&[]));
    }
}
