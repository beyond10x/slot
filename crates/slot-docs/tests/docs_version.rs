//! The install snippet in the docs names the version being released.

#[test]
fn getting_started_installs_this_version() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../website/docs/getting-started.md"
    );
    let page = std::fs::read_to_string(path).expect("read getting-started.md");
    let want = format!("version={}", env!("CARGO_PKG_VERSION"));
    assert!(
        page.lines().any(|l| l.trim() == want),
        "website/docs/getting-started.md must contain the line `{want}`: update it with the release"
    );
}
