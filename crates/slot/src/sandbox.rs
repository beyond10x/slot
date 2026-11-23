//! Jobs in a sandbox (`slot serve --sandbox`).
//!
//! Every job runs under bubblewrap (`bwrap`): the whole filesystem is bound
//! read-only, in a pid and IPC namespace of its own, and only what the job
//! builds is writable: its checkout, its target directory, a private `/tmp`
//! and the download caches a build fills. Other synced workspaces, other
//! jobs' target directories, the user's runtime directory (the user bus) and
//! the daemon's socket are hidden. The network is shared: the compile cache
//! server answers on TCP and cargo may fetch.
//!
//! A job cannot delete, replace or install a program another build needs
//! (`~/.cargo/bin`, `~/.rustup`, `/usr`), nor change another build's tree.
//! `docs/design.md` § 27 names each writable path and its reason.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::toolchain;

/// Set in a sandboxed job's environment, so a build can tell.
pub const ENV: &str = "SLOT_SANDBOX";

/// One path a sandboxed job may write, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Writable {
    /// The path, the same inside the sandbox as outside.
    pub path: PathBuf,
    /// Why the job may write it.
    pub why: &'static str,
    /// Whether the daemon creates it before the job starts. A path not
    /// created is bound only when it exists.
    pub create: Create,
}

/// What the daemon creates for a writable path that does not exist yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Create {
    /// Nothing: bound only when it exists.
    No,
    /// A directory.
    Dir,
    /// An empty file (cargo's lock files).
    File,
}

/// What the daemon knows about a job as it starts it.
#[derive(Debug, Clone)]
pub struct Job<'a> {
    /// The job's working directory.
    pub cwd: &'a Path,
    /// The root of the job's synced workspace (`<ws-root>/<id>`); `None`
    /// for a local job.
    pub workspace: Option<&'a Path>,
    /// The job's Cargo target directory, when the daemon knows it.
    pub target_dir: Option<PathBuf>,
    /// The job's environment as it starts.
    pub env: &'a [String],
    /// Where synced workspaces live: hidden but for the job's own.
    pub ws_root: &'a Path,
    /// Target directories of synced workspaces live under its `target/`:
    /// hidden but for the job's own.
    pub cache_root: &'a Path,
    /// The daemon's socket: hidden.
    pub socket: &'a Path,
    /// The pool's jobserver FIFO: bound again after the hidden paths, in
    /// case one of them holds it.
    pub fifo: &'a Path,
}

/// The sandbox every job of a daemon started with `--sandbox` runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    bwrap: PathBuf,
    extra: Vec<PathBuf>,
}

impl Sandbox {
    /// The sandbox with `bwrap` from `PATH`, after a probe that it can
    /// create the namespaces a job needs. `extra` are more writable paths
    /// (`--sandbox-write`). An error says why no job could run sandboxed:
    /// the daemon then refuses to start, it never runs a job without it.
    pub fn find(extra: Vec<PathBuf>) -> Result<Self> {
        let path = std::env::var("PATH").unwrap_or_default();
        let bwrap = std::env::split_paths(&path)
            .chain([PathBuf::from("/usr/bin")])
            .map(|d| d.join("bwrap"))
            .find(|p| p.is_file())
            .context(
                "--sandbox needs bubblewrap, and `bwrap` is not on PATH: install it \
                 (`dnf install bubblewrap`, `apt install bubblewrap`, `pacman -S bubblewrap`) \
                 or start without --sandbox",
            )?;
        let s = Self { bwrap, extra };
        s.probe()?;
        Ok(s)
    }

    /// Runs `true` in the namespaces and mounts every job gets.
    fn probe(&self) -> Result<()> {
        let out = Command::new(&self.bwrap)
            .args(BASE)
            .args(["--tmpfs", "/tmp", "--", "true"])
            .output()
            .with_context(|| format!("--sandbox: run {}", self.bwrap.display()))?;
        if !out.status.success() {
            bail!(
                "--sandbox: {} cannot create a sandbox here ({}): {}; unprivileged user \
                 namespaces may be off (`sysctl user.max_user_namespaces`)",
                self.bwrap.display(),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// The program the daemon starts in place of the job's own.
    pub fn bwrap(&self) -> &Path {
        &self.bwrap
    }

    /// The paths `job` may write, in the order they are bound.
    pub fn writable(&self, job: &Job<'_>) -> Vec<Writable> {
        let home = toolchain::get(job.env, "HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        // The home directories, the job's and the daemon's: never writable,
        // nor a directory that holds one, so a job run from there does not
        // get the toolchains back.
        let homes: Vec<PathBuf> = home
            .iter()
            .cloned()
            .chain(std::env::var_os("HOME").map(PathBuf::from))
            .filter(|h| h.is_absolute())
            .collect();
        let mut w = Vec::new();
        let tree = job
            .workspace
            .map_or_else(|| checkout(job.cwd, &homes), Path::to_path_buf);
        w.push(Writable {
            path: tree,
            why: "the job's checkout: a build writes generated files and Cargo.lock in its tree",
            create: Create::No,
        });
        if let Some(t) = &job.target_dir {
            w.push(Writable {
                path: t.clone(),
                why: "the job's target directory: what cargo builds",
                create: Create::Dir,
            });
        }
        let cargo_home = toolchain::get(job.env, "CARGO_HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".cargo")));
        if let Some(ch) = cargo_home {
            let exists = ch.is_dir();
            let create = |c| if exists { c } else { Create::No };
            for (name, kind) in [("registry", Create::Dir), ("git", Create::Dir)] {
                w.push(Writable {
                    path: ch.join(name),
                    why: "cargo's download cache of crates and git dependencies",
                    create: create(kind),
                });
            }
            for name in [".package-cache", ".package-cache-mutate"] {
                w.push(Writable {
                    path: ch.join(name),
                    why: "cargo's lock on its download cache, shared with every other cargo",
                    create: create(Create::File),
                });
            }
        }
        // Named in the environment (the daemon names it for a synced
        // workspace), it is created; Go's default only when it exists.
        let gocache = toolchain::get(job.env, "GOCACHE")
            .filter(|v| !v.is_empty() && *v != "off")
            .map(|v| (PathBuf::from(v), Create::Dir))
            .or_else(|| {
                xdg_cache(job.env, home.as_deref()).map(|c| (c.join("go-build"), Create::No))
            });
        if let Some((path, create)) = gocache {
            w.push(Writable {
                path,
                why: "Go's build cache, content-addressed",
                create,
            });
        }
        if let Some(modcache) = go_mod_cache(job.env, home.as_deref()) {
            w.push(Writable {
                path: modcache,
                why: "Go's module download cache",
                create: Create::No,
            });
        }
        for p in &self.extra {
            w.push(Writable {
                path: p.clone(),
                why: "named with --sandbox-write",
                create: Create::No,
            });
        }
        w.retain(|x| {
            x.path.is_absolute()
                && x.path.parent().is_some()
                && !homes.iter().any(|h| h.starts_with(&x.path))
        });
        w
    }

    /// Paths `job` does not see: other workspaces, other jobs' target
    /// directories, the user's runtime directory (the user bus, the
    /// daemon's socket). The job's own are bound again on top.
    fn hidden(job: &Job<'_>) -> Vec<PathBuf> {
        let runtime = toolchain::get(job.env, "XDG_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(crate::paths::runtime_dir);
        [
            Some(job.ws_root.to_path_buf()),
            Some(job.cache_root.join("target")),
            runtime,
        ]
        .into_iter()
        .flatten()
        .filter(|p| p.is_absolute() && p.parent().is_some() && p.is_dir())
        .collect()
    }

    /// Creates what [`Sandbox::writable`] says the daemon creates.
    pub fn prepare(&self, job: &Job<'_>) {
        for w in self.writable(job) {
            if w.path.exists() {
                continue;
            }
            let _ = match w.create {
                Create::No => Ok(()),
                Create::Dir => std::fs::create_dir_all(&w.path),
                Create::File => std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&w.path)
                    .map(drop),
            };
        }
    }

    /// `argv` run in the sandbox: `bwrap`, its options, `--`, `argv`.
    pub fn wrap(&self, job: &Job<'_>, argv: Vec<String>) -> Vec<String> {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let mut a: Vec<String> = std::iter::once(s(&self.bwrap))
            .chain(BASE.iter().map(|x| (*x).to_string()))
            .collect();
        a.extend(["--tmpfs".into(), "/tmp".into()]);
        // Bound, then hidden, then the job's own bound again on top: a
        // writable checkout that holds the workspace root does not show the
        // other workspaces, and the job's own workspace stays writable.
        let writable = self.writable(job);
        let hidden = Self::hidden(job);
        for w in &writable {
            a.extend(["--bind-try".into(), s(&w.path), s(&w.path)]);
        }
        for h in &hidden {
            a.extend(["--tmpfs".into(), s(h)]);
        }
        for w in &writable {
            if hidden.iter().any(|h| w.path.starts_with(h)) {
                a.extend(["--bind-try".into(), s(&w.path), s(&w.path)]);
            }
        }
        // A socket path bound to /dev/null refuses every connection.
        if job.socket.is_absolute() {
            a.extend(["--ro-bind".into(), "/dev/null".into(), s(job.socket)]);
        }
        // A FIFO stays writable on a read-only mount: bound again only so a
        // hidden directory or the private /tmp does not take it away.
        if job.fifo.is_absolute() {
            a.extend(["--ro-bind-try".into(), s(job.fifo), s(job.fifo)]);
        }
        a.extend(["--chdir".into(), s(job.cwd), "--".into()]);
        a.extend(argv);
        a
    }

    /// `env` with what a sandboxed job's environment says differently: its
    /// `TMPDIR` is the private `/tmp`, and [`ENV`] is set.
    pub fn env(env: Vec<String>) -> Vec<String> {
        let env = toolchain::set(env, "TMPDIR", "/tmp");
        toolchain::set(env, ENV, "bwrap")
    }
}

/// What every sandbox starts from: the root read-only, a fresh `/dev` and
/// `/proc`, its own pid and IPC namespace.
const BASE: &[&str] = &[
    "--ro-bind",
    "/",
    "/",
    "--dev",
    "/dev",
    "--proc",
    "/proc",
    "--unshare-pid",
    "--unshare-ipc",
];

/// The git checkout that holds `cwd`: the nearest ancestor with a `.git`
/// entry below every home directory in `homes` (a home kept in git is not
/// a checkout to build in), else `cwd` itself.
fn checkout(cwd: &Path, homes: &[PathBuf]) -> PathBuf {
    let cwd = normalize(cwd);
    cwd.ancestors()
        .take_while(|d| !homes.iter().any(|h| h.starts_with(d)))
        .find(|d| d.join(".git").exists())
        .map_or_else(|| cwd.clone(), Path::to_path_buf)
}

fn xdg_cache(env: &[String], home: Option<&Path>) -> Option<PathBuf> {
    toolchain::get(env, "XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".cache")))
}

fn go_mod_cache(env: &[String], home: Option<&Path>) -> Option<PathBuf> {
    if let Some(m) = toolchain::get(env, "GOMODCACHE").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(m));
    }
    let gopath = toolchain::get(env, "GOPATH")
        .and_then(|v| std::env::split_paths(v).next())
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| home.map(|h| h.join("go")))?;
    Some(gopath.join("pkg").join("mod"))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(extra: &[&str]) -> Sandbox {
        Sandbox {
            bwrap: "/usr/bin/bwrap".into(),
            extra: extra.iter().map(PathBuf::from).collect(),
        }
    }

    fn env(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    fn paths(w: &[Writable]) -> Vec<String> {
        w.iter()
            .map(|w| w.path.to_string_lossy().into_owned())
            .collect()
    }

    /// A synced workspace's job writes its tree, its target directory,
    /// cargo's download cache and locks, the Go caches and the extra paths;
    /// never the home directory or a directory above it.
    #[test]
    fn a_workspace_job_writes_its_tree_target_and_download_caches() {
        let e = env(&[
            "HOME=/h",
            "CARGO_HOME=/h/.cargo",
            "GOCACHE=/c/go-build",
            "GOPATH=/g:/g2",
        ]);
        let job = Job {
            cwd: Path::new("/ws/abc/sub"),
            workspace: Some(Path::new("/ws/abc")),
            target_dir: Some("/c/target/abc".into()),
            env: &e,
            ws_root: Path::new("/ws"),
            cache_root: Path::new("/c"),
            socket: Path::new("/run/s.sock"),
            fifo: Path::new("/state/jobserver.fifo"),
        };
        let w = sandbox(&["/srv/pnpm", "/h", "/"]).writable(&job);
        assert_eq!(
            paths(&w),
            [
                "/ws/abc",
                "/c/target/abc",
                "/h/.cargo/registry",
                "/h/.cargo/git",
                "/h/.cargo/.package-cache",
                "/h/.cargo/.package-cache-mutate",
                "/c/go-build",
                "/g/pkg/mod",
                "/srv/pnpm",
            ]
        );
        assert!(w.iter().all(|w| !w.why.is_empty()));
    }

    /// The bwrap command line: the root read-only, the private /tmp, the
    /// hidden paths before the job's own writable paths, the socket bound
    /// to /dev/null, the FIFO bound again, then the job's command.
    #[test]
    fn the_command_line_hides_before_it_binds() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("ws");
        let cache = root.path().join("cache");
        std::fs::create_dir_all(ws.join("abc")).unwrap();
        std::fs::create_dir_all(cache.join("target")).unwrap();
        let e = env(&["HOME=/h"]);
        let job = Job {
            cwd: &ws.join("abc"),
            workspace: Some(&ws.join("abc")),
            target_dir: Some(cache.join("target/abc")),
            env: &e,
            ws_root: &ws,
            cache_root: &cache,
            socket: Path::new("/run/s.sock"),
            fifo: Path::new("/state/jobserver.fifo"),
        };
        let a = sandbox(&[]).wrap(&job, env(&["cargo", "build"]));
        let line = a.join(" ");
        let at = |s: &str| line.find(s).unwrap_or_else(|| panic!("{s} in {line}"));
        assert!(line.starts_with("/usr/bin/bwrap --ro-bind / / --dev /dev --proc /proc"));
        assert!(at("--unshare-pid") < at("--tmpfs /tmp"));
        let hide_ws = at(&format!("--tmpfs {}", ws.display()));
        let bind_ws = line
            .rfind(&format!("--bind-try {0} {0}", ws.join("abc").display()))
            .unwrap();
        assert!(hide_ws < bind_ws, "{line}");
        let bind_target = line
            .rfind(&format!(
                "--bind-try {0} {0}",
                cache.join("target/abc").display()
            ))
            .unwrap();
        assert!(at(&format!("--tmpfs {}", cache.join("target").display())) < bind_target);
        assert!(line.contains("--ro-bind /dev/null /run/s.sock"));
        assert!(line.contains("--ro-bind-try /state/jobserver.fifo /state/jobserver.fifo"));
        assert!(line.ends_with(&format!(
            "--chdir {} -- cargo build",
            ws.join("abc").display()
        )));
    }

    /// A local job whose checkout holds the workspace root: its checkout is
    /// bound before the workspace root is hidden, so the other workspaces
    /// stay out of sight.
    #[test]
    fn a_checkout_that_holds_the_workspace_root_does_not_show_it() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("ws");
        std::fs::create_dir_all(ws.join("other")).unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let e = env(&["HOME=/h"]);
        let job = Job {
            cwd: root.path(),
            workspace: None,
            target_dir: None,
            env: &e,
            ws_root: &ws,
            cache_root: &root.path().join("cache"),
            socket: Path::new("/s"),
            fifo: Path::new("/f"),
        };
        let line = sandbox(&[]).wrap(&job, env(&["true"])).join(" ");
        let bind = line
            .find(&format!("--bind-try {0} {0}", root.path().display()))
            .unwrap();
        let hide = line.find(&format!("--tmpfs {}", ws.display())).unwrap();
        assert!(bind < hide, "{line}");
    }

    #[test]
    fn a_local_job_writes_the_checkout_that_holds_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(repo.join("crates/a")).unwrap();
        std::fs::write(repo.join(".git"), "gitdir: elsewhere\n").unwrap();
        assert_eq!(
            checkout(&repo.join("crates/a"), &[root.path().to_path_buf()]),
            repo
        );
        let bare = root.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(checkout(&bare, &[root.path().to_path_buf()]), bare);
        // A home directory kept in git: a directory in it is its own checkout.
        std::fs::create_dir(root.path().join(".git")).unwrap();
        assert_eq!(checkout(&bare, &[root.path().to_path_buf()]), bare);
        assert_eq!(checkout(&bare, &[]), root.path());
    }

    #[test]
    fn a_job_run_from_the_home_directory_does_not_get_it_writable() {
        let e = env(&["HOME=/h"]);
        let job = Job {
            cwd: Path::new("/h"),
            workspace: None,
            target_dir: None,
            env: &e,
            ws_root: Path::new("/ws"),
            cache_root: Path::new("/c"),
            socket: Path::new("/s"),
            fifo: Path::new("/f"),
        };
        let w = paths(&sandbox(&[]).writable(&job));
        assert!(!w.contains(&"/h".to_string()), "{w:?}");
    }

    #[test]
    fn the_job_environment_points_tmpdir_at_the_private_tmp() {
        let e = Sandbox::env(env(&["TMPDIR=/srv/scratch", "A=1"]));
        assert_eq!(toolchain::get(&e, "TMPDIR"), Some("/tmp"));
        assert_eq!(toolchain::get(&e, ENV), Some("bwrap"));
        assert_eq!(toolchain::get(&e, "A"), Some("1"));
    }
}
