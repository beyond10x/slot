//! What a remote `go build -o` or `go test -c -o` wrote is on the client's
//! side after the job, at the same path, also under `target/`. The go
//! command is a stand-in that writes what `-o` names, so no Go toolchain is
//! needed.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant, SystemTime};

use super::{Daemon, SAME_PATH, forced_ssh, git, remote_command, same_path_node, script};

/// The modification time the stand-in gives a file it wrote: 2001-02-03
/// 04:05:06 UTC.
const BUILT_AT: u64 = 981_173_106;

/// A `go` that writes what `-o` names, as the go command does, and exits
/// with `code`: a file with mode 750 and the time [`BUILT_AT`]; into a
/// path that ends in `/` or is a directory, the files `a` and `b`.
fn standin_go(dir: &Path, code: i32) -> PathBuf {
    let bin = dir.join(format!("bin{code}"));
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join("go");
    script(
        &path,
        &format!(
            r#"out=
prev=
for a in "$@"; do
  case "$prev" in -o) out=$a ;; esac
  case "$a" in -o=*) out=${{a#-o=}} ;; esac
  prev=$a
done
[ -n "$out" ] || exit {code}
case "$out" in */) dir=1 ;; *) if [ -d "$out" ]; then dir=1; else dir=; fi ;; esac
if [ -n "$dir" ]; then
  mkdir -p "$out"
  for n in a b; do
    printf 'built %s\n' "$n" > "$out/$n"
    chmod 755 "$out/$n"
  done
else
  mkdir -p "$(dirname "$out")"
  printf 'built tool\n' > "$out"
  chmod 750 "$out"
  touch -d @{BUILT_AT} "$out"
fi
exit {code}"#
        ),
    );
    path
}

/// A Go module of two files in a git repository that ignores `target/`
/// and `out/`.
fn go_module(root: &Path) -> PathBuf {
    let r = root.join("mod");
    std::fs::create_dir_all(r.join("cmd/tool")).unwrap();
    std::fs::write(r.join("go.mod"), "module example.invalid/mod\n\ngo 1.22\n").unwrap();
    std::fs::write(
        r.join("cmd/tool/main.go"),
        "package main\n\nfunc main() {}\n",
    )
    .unwrap();
    std::fs::write(r.join(".gitignore"), "/target/\n/out/\n").unwrap();
    git(&r, &["init", "-q"]);
    git(&r, &["add", "."]);
    git(&r, &["commit", "-qm", "mod"]);
    r
}

/// A remote job from `cwd`, with `env` added to the caller's environment.
fn remote_job(d: &Daemon, ssh: &Path, cwd: &Path, argv: &[&str], env: &[(&str, &str)]) -> Output {
    remote_command(d, ssh, cwd, argv, &[])
        .env_remove("SLOT_NO_PRODUCTS")
        .envs(env.iter().copied())
        .output()
        .unwrap()
}

/// The module's one mirror on the stand-in node.
fn node_mirror(d: &Daemon) -> PathBuf {
    let dirs: Vec<PathBuf> = std::fs::read_dir(d.dir.path().join("state/ws"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("go.mod").is_file())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.into_iter().next().unwrap()
}

/// The rsync senders (a listing or a fetch) the fake ssh logged.
fn senders(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("ssh.log"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("rsync ") && l.contains("--sender"))
        .map(String::from)
        .collect()
}

/// The lines slot itself wrote about products, and rsync's own.
fn product_lines(err: &str) -> Vec<&str> {
    err.lines()
        .filter(|l| l.contains("fetched") || l.starts_with("rsync"))
        .collect()
}

#[test]
fn a_remote_go_build_leaves_its_output_at_the_same_path() {
    let t0 = Instant::now();
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = go_module(t.path());
    let ssh = forced_ssh(t.path(), &d, "");
    let go = standin_go(t.path(), 0);
    let go = go.to_str().unwrap();

    let out = remote_job(
        &d,
        &ssh,
        &repo,
        &[go, "build", "-o", "target/x/tool", "./cmd/tool"],
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let local = repo.join("target/x/tool");
    let node = node_mirror(&d).join("target/x/tool");
    assert_eq!(
        std::fs::read_to_string(&node).unwrap(),
        "built tool\n",
        "{err}"
    );
    assert!(
        local.is_file(),
        "target/x/tool is missing on the client's side: {err}"
    );
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "built tool\n");
    let meta = std::fs::metadata(&local).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o750, "mode kept");
    assert_eq!(
        meta.modified().unwrap(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(BUILT_AT),
        "mtime kept"
    );
    assert!(
        err.lines()
            .any(|l| l.starts_with("slot: fetched 1 artifact, ")),
        "{err}"
    );

    // A relative path is read against the job's working directory, and
    // `-o=<path>` names it too.
    let out = remote_job(
        &d,
        &ssh,
        &repo.join("cmd"),
        &[go, "build", "-o=../out/tool2", "./tool"],
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(repo.join("out/tool2").is_file(), "{err}");

    // A test binary.
    let out = remote_job(
        &d,
        &ssh,
        &repo,
        &[go, "test", "-c", "-o", "target/x/tool.test", "./cmd/tool"],
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(repo.join("target/x/tool.test").is_file(), "{err}");
    eprintln!("took {:?}", t0.elapsed());
}

#[test]
fn a_directory_brings_back_the_files_the_build_wrote_and_no_other() {
    let t0 = Instant::now();
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = go_module(t.path());
    let ssh = forced_ssh(t.path(), &d, "");
    let go = standin_go(t.path(), 0);
    let go = go.to_str().unwrap();

    // An earlier job left two files in the directory on the node: one the
    // build does not write, one it writes again.
    let out = remote_job(
        &d,
        &ssh,
        &repo,
        &[
            "sh",
            "-c",
            "mkdir -p target/bin && echo kept > target/bin/old && echo stale-content > target/bin/a",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(!repo.join("target").exists(), "nothing came back yet");

    let out = remote_job(
        &d,
        &ssh,
        &repo,
        &[go, "build", "-o", "target/bin", "./cmd/..."],
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let bin = repo.join("target/bin");
    assert_eq!(
        std::fs::read_to_string(bin.join("a")).unwrap_or_default(),
        "built a\n",
        "the file written again: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(bin.join("b")).unwrap_or_default(),
        "built b\n",
        "the new file: {err}"
    );
    assert_ne!(
        std::fs::metadata(bin.join("a"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0,
        "executable"
    );
    assert!(
        node_mirror(&d).join("target/bin/old").is_file(),
        "still on the node"
    );
    assert!(
        !bin.join("old").exists(),
        "a file the build did not write came back: {err}"
    );
    assert!(
        err.lines()
            .any(|l| l.starts_with("slot: fetched 2 artifacts, ")),
        "{err}"
    );

    // A path that ends in `/` is a directory the build makes.
    let out = remote_job(
        &d,
        &ssh,
        &repo,
        &[go, "build", "-o", "out/new/", "./cmd/..."],
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let mut names: Vec<String> = std::fs::read_dir(repo.join("out/new"))
        .unwrap_or_else(|e| panic!("out/new: {e}: {err}"))
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    assert_eq!(names, ["a", "b"], "{err}");
    eprintln!("took {:?}", t0.elapsed());
}

#[test]
fn an_output_outside_the_tree_or_a_failed_build_brings_nothing_back() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = go_module(t.path());
    let ssh = forced_ssh(t.path(), &d, "");
    let go = standin_go(t.path(), 0);
    let failing = standin_go(t.path(), 1);

    let out = remote_job(
        &d,
        &ssh,
        &repo,
        &[
            go.to_str().unwrap(),
            "build",
            "-o",
            "../outside/tool",
            "./cmd/tool",
        ],
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(
        node_mirror(&d)
            .parent()
            .unwrap()
            .join("outside/tool")
            .is_file(),
        "written on the node, beside the mirror"
    );
    assert!(!t.path().join("outside").exists(), "{err}");
    assert_eq!(product_lines(&err), Vec::<&str>::new(), "{err}");
    assert_eq!(senders(t.path()), Vec::<String>::new(), "nothing was read");

    let out = remote_job(
        &d,
        &ssh,
        &repo,
        &[
            failing.to_str().unwrap(),
            "build",
            "-o",
            "target/x/tool",
            "./cmd/tool",
        ],
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "the job's own exit: {err}");
    assert!(
        node_mirror(&d).join("target/x/tool").is_file(),
        "written on the node"
    );
    assert!(!repo.join("target").exists(), "{err}");
    assert_eq!(product_lines(&err), Vec::<&str>::new(), "{err}");
}

#[test]
fn slot_no_products_turns_the_return_of_a_go_build_off() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = go_module(t.path());
    let ssh = forced_ssh(t.path(), &d, "");
    let go = standin_go(t.path(), 0);
    let out = remote_job(
        &d,
        &ssh,
        &repo,
        &[
            go.to_str().unwrap(),
            "build",
            "-o",
            "target/x/tool",
            "./cmd/tool",
        ],
        &[("SLOT_NO_PRODUCTS", "1")],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(node_mirror(&d).join("target/x/tool").is_file(), "built");
    assert!(!repo.join("target").exists(), "{err}");
    assert_eq!(
        product_lines(&err),
        ["slot: artifacts not fetched: SLOT_NO_PRODUCTS is set"],
        "{err}"
    );
    assert_eq!(senders(t.path()), Vec::<String>::new(), "nothing was read");
}

#[test]
fn go_commands_that_name_no_build_output_are_untouched() {
    let d = Daemon::start();
    let t = tempfile::tempdir().unwrap();
    let repo = go_module(t.path());
    let ssh = forced_ssh(t.path(), &d, "");
    let go = standin_go(t.path(), 0);
    let go = go.to_str().unwrap();
    for argv in [
        vec![go, "build", "./..."],
        vec![go, "vet", "./..."],
        vec![go, "run", "./cmd/tool", "-o", "target/x/run"],
        vec![go, "test", "-o", "target/x/test", "./..."],
        vec![go, "test", "./...", "-args", "-c", "-o", "target/x/args"],
    ] {
        let out = remote_job(&d, &ssh, &repo, &argv, &[]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{argv:?}: {err}");
        assert_eq!(product_lines(&err), Vec::<&str>::new(), "{argv:?}: {err}");
    }
    assert!(!repo.join("target").exists(), "nothing came back");
    assert_eq!(senders(t.path()), Vec::<String>::new(), "nothing was read");
}

/// Same-path: the tree on the node is the client's own path, and the fetch
/// names the output below it.
#[test]
fn a_same_path_go_build_fetches_its_output_from_the_tree() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (d, ssh) = same_path_node(&root);
    let repo = go_module(&root);
    let go = standin_go(&root, 0);
    let abs = repo.join("out/abs");
    for o in ["target/x/tool", abs.to_str().unwrap()] {
        let out = remote_job(
            &d,
            &ssh,
            &repo,
            &[go.to_str().unwrap(), "build", "-o", o, "./cmd/tool"],
            SAME_PATH,
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{err}");
        assert!(
            err.lines()
                .any(|l| l.starts_with("slot: fetched 1 artifact, ")),
            "{o}: {err}"
        );
    }
    let got = senders(&root);
    for rel in ["target/x/tool", "out/abs"] {
        let want = format!(" {}/./{rel}", repo.display());
        assert!(
            got.iter().any(|l| l.ends_with(&want)),
            "no fetch of{want}: {got:?}"
        );
    }
}
