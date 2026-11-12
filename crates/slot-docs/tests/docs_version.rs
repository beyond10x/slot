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

#[test]
fn plugin_manifests_carry_this_version() {
    let want = format!("\"version\": \"{}\"", env!("CARGO_PKG_VERSION"));
    for rel in [
        "/../../plugin/.claude-plugin/plugin.json",
        "/../../plugin/.codex-plugin/plugin.json",
    ] {
        let path = format!("{}{rel}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).expect("read plugin manifest");
        assert!(
            text.contains(&want),
            "{rel} must contain `{want}`: update it with the release"
        );
    }
}
