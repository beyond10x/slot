//! `slot-docs ui-pages`: the pages generated from the UI spec are the committed ones, and the
//! generator refuses what it does not draw instead of dropping it.

use std::path::Path;
use std::process::{Command, Output};

/// Runs `slot-docs ui-pages` in `dir` with `args`.
fn ui_pages(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_slot-docs"))
        .arg("ui-pages")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run slot-docs")
}

fn repo() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A scratch tree holding the repository's spec with `ui` as the UI document.
fn tree(ui: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("spec/ui")).expect("mkdir");
    std::fs::write(dir.path().join("spec/ui/slot.ui.yaml"), ui).expect("write ui");
    std::fs::copy(
        repo().join("spec/http.yaml"),
        dir.path().join("spec/http.yaml"),
    )
    .expect("copy http");
    dir
}

fn spec() -> String {
    std::fs::read_to_string(repo().join("spec/ui/slot.ui.yaml")).expect("read the UI spec")
}

#[test]
fn the_committed_pages_are_what_the_spec_generates() {
    let out = ui_pages(&repo(), &["--check"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_changed_page_and_a_page_the_spec_lacks_fail_the_check() {
    let dir = tree(&spec());
    let generated = dir.path().join("gen");
    let out = ui_pages(dir.path(), &["--out", "gen"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        ui_pages(dir.path(), &["--out", "gen", "--check"])
            .status
            .success()
    );
    let list = generated.join("pages/JobsList.vue");
    let text = std::fs::read_to_string(&list).expect("read page");
    std::fs::write(&list, text.replace("Running", "Busy")).expect("edit page");
    std::fs::write(generated.join("pages/Old.vue"), "").expect("stale page");
    let out = ui_pages(dir.path(), &["--out", "gen", "--check"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("pages/JobsList.vue") && err.contains("Old.vue"),
        "{err}"
    );
}

#[test]
fn a_key_the_generator_does_not_draw_is_refused_by_name() {
    let ui = spec().replace(
        "        title: Running\n",
        "        title: Running\n        load: on_visible\n",
    );
    let out = ui_pages(tree(&ui).path(), &["--out", "gen"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains("unknown field `load`"), "{err}");
}

#[test]
fn a_filter_compiles_to_the_operators_of_the_runtime() {
    let dir = tree(&spec());
    assert!(ui_pages(dir.path(), &["--out", "gen"]).status.success());
    let page =
        std::fs::read_to_string(dir.path().join("gen/pages/JobsList.vue")).expect("read page");
    assert!(
        page.contains(r#"among(row["state"], ["ok", "failed", "cancelled"]) && contains(row["state"], state["state"]) && contains(row["argv"], state["search"])"#),
        "{page}"
    );
}

#[test]
fn a_filter_on_state_the_page_lacks_is_refused() {
    let ui = spec().replace("contains state.search", "contains state.nothing");
    let out = ui_pages(tree(&ui).path(), &["--out", "gen"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("`state.nothing` names no state of the page"),
        "{err}"
    );
}
