//! Adversarial cases of the remote sync's filters and tree-job registry
//! (commit "Sync: the client's filters replace the host rsync wrapper"),
//! against a fake node: a stand-in `ssh` that runs the server's command on
//! this machine in a directory of the test's own. No network.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU32, Ordering};

use slot::same_path;
use slot::sync::{self, Bounds, Remote};

/// Bounds of a test: no host-wide upload slot, no bandwidth cap.
fn bounds() -> Bounds {
    Bounds {
        bwlimit: 0,
        upload_slots: 0,
        ..Bounds::default()
    }
}

/// [`bounds`] with `SLOT_SYNC_IGNORED=1`: git-ignored files are sent.
fn bounds_ignored() -> Bounds {
    Bounds {
        send_ignored: true,
        ..bounds()
    }
}

/// A mirror id no other test or process uses (the sync lock is per id).
fn fresh_id() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "ad{:08x}{:06x}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// A fake `ssh <home> HOST WORDS…`: runs WORDS in `<root>/node`; an
/// absolute destination (a same-path sync) is moved below that directory.
/// `(home, ssh)`.
fn fake_node(root: &Path) -> (PathBuf, Vec<String>) {
    let home = root.join("node");
    std::fs::create_dir_all(&home).unwrap();
    let script = root.join("ssh.sh");
    std::fs::write(
        &script,
        "home=$1; shift 2; cd \"$home\" || exit 1\n\
         n=$#; i=0\n\
         for a do\n\
           i=$((i+1))\n\
           if [ \"$i\" -eq \"$n\" ]; then case $a in /*) a=\"$home$a\";; esac; fi\n\
           set -- \"$@\" \"$a\"\n\
         done\n\
         shift \"$n\"\n\
         exec \"$@\"\n",
    )
    .unwrap();
    let ssh = vec![
        "sh".to_string(),
        script.display().to_string(),
        home.display().to_string(),
    ];
    (home, ssh)
}

fn remote(ssh: &[String], same_path: bool) -> Remote<'_> {
    Remote {
        host: "node",
        ssh,
        ws_root: "ws",
        // Not a slot: the sibling step fails and the plain sync runs.
        remote_slot: "false",
        same_path,
    }
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

fn git_init(dir: &Path) {
    let st = Command::new("git")
        .args(["init", "-q"])
        .arg(dir)
        .status()
        .unwrap();
    assert!(st.success());
}

/// Another live remote job of `top`, named in the registry `jobs` as
/// `TreeJob::register` names one.
fn other_job(jobs: &Path, top: &Path) -> Child {
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    std::fs::create_dir_all(jobs).unwrap();
    std::fs::write(
        jobs.join(child.id().to_string()),
        format!("{}\n", top.display()),
    )
    .unwrap();
    child
}

fn end(mut c: Child) {
    let _ = c.kill();
    let _ = c.wait();
}

/// The sync must not run a job on content the caller did not have
/// (`sync.rs` module doc). A test file renamed here stays on the node
/// under its old name while another job of the tree runs, and cargo runs
/// every `tests/*.rs` as a test target there (go: every `.go` file of the
/// package, so a renamed file defines everything twice).
#[test]
fn a_file_renamed_here_leaves_the_node_while_another_job_of_the_tree_runs() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let top = root.join("tree");
    let jobs = root.join("jobs");
    write(&top.join("Cargo.toml"), "[package]\n");
    write(&top.join("tests/old.rs"), "#[test] fn t() {}\n");
    let (home, ssh) = fake_node(&root);
    let r = remote(&ssh, false);
    let id = fresh_id();
    let m = home.join("ws").join(&id);
    sync::sync(&top, &id, &["/target/"], &r, &bounds(), Some(&jobs)).unwrap();
    assert!(m.join("tests/old.rs").is_file());
    std::fs::rename(top.join("tests/old.rs"), top.join("tests/new.rs")).unwrap();
    let other = other_job(&jobs, &top);
    let res = sync::sync(&top, &id, &["/target/"], &r, &bounds(), Some(&jobs));
    end(other);
    res.unwrap();
    assert!(m.join("tests/new.rs").is_file());
    assert!(
        !m.join("tests/old.rs").exists(),
        "tests/old.rs, renamed here, is still on the node: the job runs a test the caller does not have"
    );
}

/// A directory removed here, beside another job of the tree: the files the
/// previous sync sent there leave the node; a file that job wrote in it
/// stays, and so does the directory that holds it.
#[test]
fn a_directory_removed_here_leaves_the_node_but_not_another_jobs_file_in_it() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let top = root.join("tree");
    let jobs = root.join("jobs");
    write(&top.join("keep.rs"), "\n");
    write(&top.join("gone/a/one.rs"), "\n");
    write(&top.join("gone/two.rs"), "\n");
    write(&top.join("empty/three.rs"), "\n");
    let (home, ssh) = fake_node(&root);
    let r = remote(&ssh, false);
    let id = fresh_id();
    let m = home.join("ws").join(&id);
    sync::sync(&top, &id, &["/target"], &r, &bounds(), Some(&jobs)).unwrap();
    write(&m.join("gone/a/written-by-a-job"), "out\n");
    write(&m.join("written-by-a-job"), "out\n");
    std::fs::remove_dir_all(top.join("gone")).unwrap();
    std::fs::remove_dir_all(top.join("empty")).unwrap();
    let other = other_job(&jobs, &top);
    let res = sync::sync(&top, &id, &["/target"], &r, &bounds(), Some(&jobs));
    end(other);
    res.unwrap();
    assert!(m.join("keep.rs").is_file());
    assert!(m.join("written-by-a-job").is_file());
    assert!(m.join("gone/a/written-by-a-job").is_file());
    for f in ["gone/a/one.rs", "gone/two.rs", "empty/three.rs"] {
        assert!(!m.join(f).exists(), "{f}, gone here, is still on the node");
    }
    assert!(!m.join("empty").exists(), "the emptied directory stays");
}

/// `cwd_ignored` says whether the working directory "lies under a path the
/// sync of `top` leaves out". With `SLOT_SYNC_IGNORED=1` the sync sends
/// what git ignores, so an ignored directory is not left out; without it,
/// git's word decides, as before.
#[test]
fn an_ignored_working_directory_is_not_left_out_of_the_sync() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let top = root.join("tree");
    std::fs::create_dir_all(&top).unwrap();
    git_init(&top);
    write(&top.join(".gitignore"), "/gen/\n");
    write(&top.join("gen/input.txt"), "x\n");
    write(&top.join("Cargo.toml"), "[package]\n");
    let filters = sync::tree_filters(&top);
    assert!(
        !filters.iter().any(|f| f.contains("gen")),
        "the sync leaves gen out: {filters:?}"
    );
    assert!(
        !same_path::cwd_ignored(&top, &top.join("gen"), &["/target"], true),
        "gen is sent by the sync of the tree, yet the client takes it as left out and syncs it again with --delete"
    );
    assert!(
        same_path::cwd_ignored(&top, &top.join("target/scratch"), &[], true),
        "target beside Cargo.toml is left out"
    );
    assert!(
        same_path::cwd_ignored(&top, &top.join("gen"), &["/target"], false),
        "by default git's ignore rules leave gen out"
    );
}

/// A same-path job whose working directory git ignores: the client syncs
/// the tree, then the working directory once more as an extra tree
/// (`client.rs`, `same_path_extras`, through `sync::sync_cwd`). Neither
/// sync deletes what another job of the tree wrote on the node.
#[test]
fn a_same_path_job_in_an_ignored_directory_keeps_another_jobs_files() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let top = root.join("tree");
    let jobs = root.join("jobs");
    std::fs::create_dir_all(&top).unwrap();
    git_init(&top);
    write(&top.join(".gitignore"), "/gen/\n");
    write(&top.join("gen/input.txt"), "x\n");
    let cwd = top.join("gen");
    let (home, ssh) = fake_node(&root);
    let r = remote(&ssh, true);
    let id = fresh_id();
    let node_tree = home.join(top.strip_prefix("/").unwrap());
    let both = || {
        sync::sync(&top, &id, &["/target"], &r, &bounds(), Some(&jobs))?;
        let kept_by_tree_sync =
            !node_tree.join("gen").exists() || node_tree.join("gen/written-by-a-job").is_file();
        let extra_runs = sync::sync_cwd(&top, &cwd, &["/target"], &r, &bounds(), Some(&jobs))?;
        anyhow::Ok((kept_by_tree_sync, extra_runs))
    };
    let (_, first_extra) = both().unwrap();
    assert!(
        first_extra,
        "the client does not sync the working directory"
    );
    assert!(node_tree.join("gen/input.txt").is_file());
    write(&node_tree.join("gen/written-by-a-job"), "out\n");
    let other = other_job(&jobs, &top);
    let res = both();
    end(other);
    let (kept_by_tree_sync, extra_runs) = res.unwrap();
    assert!(kept_by_tree_sync, "the tree's sync deleted it");
    assert!(
        extra_runs,
        "the client does not sync the working directory again"
    );
    assert!(
        node_tree.join("gen/written-by-a-job").is_file(),
        "the working directory's extra sync deleted a file another job of the tree wrote on the node"
    );
}

/// By default the sync applies every `.gitignore`: an ignored file such as
/// `.env` stays here. With `SLOT_SYNC_IGNORED=1` it is sent; cargo's build
/// directory stays here in both modes.
#[test]
fn ignored_files_are_sent_only_when_asked() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let top = root.join("tree");
    write(&top.join(".gitignore"), ".env\n/target/\n");
    write(&top.join(".env"), "TOKEN=x\n");
    write(&top.join("Cargo.toml"), "[package]\n");
    write(&top.join("src/lib.rs"), "\n");
    write(&top.join("target/debug/x"), "build\n");
    let (home, ssh) = fake_node(&root);
    let r = remote(&ssh, false);
    let id = fresh_id();
    let m = home.join("ws").join(&id);
    sync::sync(&top, &id, &["/target"], &r, &bounds(), None).unwrap();
    assert!(m.join("src/lib.rs").is_file());
    assert!(
        !m.join(".env").exists(),
        "an ignored file reached the node by default"
    );
    assert!(!m.join("target").exists());
    sync::sync(&top, &id, &["/target"], &r, &bounds_ignored(), None).unwrap();
    assert!(m.join(".env").is_file(), "SLOT_SYNC_IGNORED=1 sends it");
    assert!(!m.join("target").exists());
}

/// The wait for an upload slot is not part of the sync's bound: the bound
/// counts from when rsync starts.
#[test]
fn the_wait_for_an_upload_slot_does_not_spend_the_sync_bound() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("slots");
    let b = Bounds {
        upload_slots: 1,
        limit: std::time::Duration::from_secs(5),
        ..bounds()
    };
    let held = sync::upload_slot(Some(&dir), 1, std::time::Instant::now()).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(700));
        drop(held);
    });
    let t0 = std::time::Instant::now();
    let deadline = t0 + std::time::Duration::from_millis(300);
    let (_slot, moved) = sync::take_upload_slot(&b, Some(&dir), deadline).unwrap();
    release.join().unwrap();
    let waited = t0.elapsed();
    assert!(
        waited >= std::time::Duration::from_millis(600),
        "{waited:?}"
    );
    assert!(
        moved + std::time::Duration::from_millis(50) >= deadline + waited,
        "the deadline did not move by the wait"
    );
}

/// `<dir>/target/` beside each `Cargo.toml`: also beside one that is a
/// symbolic link (the walk takes only regular files).
#[test]
fn a_symlinked_cargo_toml_gets_its_build_directory_left_out() {
    let t = tempfile::tempdir().unwrap();
    let top = t.path();
    write(&top.join("shared/Cargo.toml.in"), "[package]\n");
    std::fs::create_dir_all(top.join("sub")).unwrap();
    std::os::unix::fs::symlink("../shared/Cargo.toml.in", top.join("sub/Cargo.toml")).unwrap();
    let dirs = sync::build_dirs(top);
    assert!(
        dirs.contains(&"/sub/target".to_string()),
        "sub/target is not left out: {dirs:?}"
    );
}

/// `<dir>/target/` beside each `Cargo.toml`: also below a directory named
/// `target` that cargo did not make (the walk does not enter one).
#[test]
fn a_crate_below_a_source_directory_named_target_gets_its_build_directory_left_out() {
    let t = tempfile::tempdir().unwrap();
    let top = t.path();
    write(&top.join("Cargo.toml"), "[workspace]\n");
    write(&top.join("examples/target/demo/Cargo.toml"), "[package]\n");
    write(
        &top.join("examples/target/demo/src/main.rs"),
        "fn main() {}\n",
    );
    let dirs = sync::build_dirs(top);
    assert!(
        dirs.contains(&"/examples/target/demo/target".to_string()),
        "examples/target/demo/target is not left out: {dirs:?}"
    );
}

/// A `target` beside `Cargo.toml` that is a symbolic link here (a build
/// directory on another disk) is still cargo's build directory: the sync
/// must leave the node's own build output there alone.
#[test]
fn a_symlinked_target_here_keeps_the_nodes_build_directory() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let top = root.join("tree");
    write(&top.join("Cargo.toml"), "[package]\n");
    write(&top.join("src/lib.rs"), "\n");
    let (home, ssh) = fake_node(&root);
    let r = remote(&ssh, false);
    let id = fresh_id();
    let m = home.join("ws").join(&id);
    sync::sync(&top, &id, &["/target/"], &r, &bounds(), None).unwrap();
    // The node built there.
    write(&m.join("target/debug/libx.rlib"), "build\n");
    write(&m.join("target/CACHEDIR.TAG"), "Signature\n");
    let elsewhere = root.join("fast-disk/target");
    write(&elsewhere.join("CACHEDIR.TAG"), "Signature\n");
    std::os::unix::fs::symlink(&elsewhere, top.join("target")).unwrap();
    sync::sync(&top, &id, &["/target/"], &r, &bounds(), None).unwrap();
    assert!(
        m.join("target/debug/libx.rlib").is_file(),
        "the node's build directory was replaced: target is {:?}",
        std::fs::symlink_metadata(m.join("target")).map(|x| x.file_type())
    );
}
