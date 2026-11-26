//! Copies the built web UI (`ui/dist`) into `OUT_DIR/ui` for embedding, or
//! writes a one-page placeholder when the UI has not been built.

use std::fs;
use std::path::{Path, PathBuf};

fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy(&e.path(), &target)?;
        } else {
            fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}

#[path = "src/build_version.rs"]
#[allow(dead_code)]
mod build_version;

/// Runs git in the crate's directory; `None` when git fails or is missing.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Sets `SLOT_VERSION`: the package version, with `+g<commit>` for any
/// build that is not of its release tag (`build_version::name`).
fn version(manifest: &Path) {
    let head = git(manifest, &["rev-parse", "--short", "HEAD"]).filter(|s| !s.is_empty());
    if head.is_some() {
        for which in ["--git-dir", "--git-common-dir"] {
            if let Some(d) = git(manifest, &["rev-parse", "--path-format=absolute", which]) {
                let d = PathBuf::from(d);
                for f in ["HEAD", "index", "packed-refs", "refs/tags"] {
                    println!("cargo:rerun-if-changed={}", d.join(f).display());
                }
            }
        }
        println!("cargo:rerun-if-changed={}", manifest.join("src").display());
    }
    let c = build_version::Checkout {
        tags: git(manifest, &["tag", "--points-at", "HEAD"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default(),
        dirty: head.is_some()
            && git(manifest, &["status", "--porcelain", "--untracked-files=no"])
                .is_none_or(|s| !s.is_empty()),
        ci_tag: (std::env::var("GITHUB_REF_TYPE").as_deref() == Ok("tag"))
            .then(|| std::env::var("GITHUB_REF_NAME").ok())
            .flatten(),
        head,
    };
    println!("cargo:rerun-if-env-changed=GITHUB_REF_TYPE");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
    let pkg = std::env::var("CARGO_PKG_VERSION").expect("package version");
    println!(
        "cargo:rustc-env=SLOT_VERSION={}",
        build_version::name(&pkg, &c)
    );
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    version(&manifest);
    let dist = manifest.join("../../ui/dist");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("ui");
    let _ = fs::remove_dir_all(&out);
    println!("cargo:rerun-if-changed={}", dist.display());
    if dist.join("index.html").is_file() {
        copy(&dist, &out).expect("copy ui/dist");
    } else {
        fs::create_dir_all(&out).expect("create ui placeholder dir");
        fs::write(
            out.join("index.html"),
            "<!doctype html><title>slot</title><p>The web UI was not built into this binary. \
             Build it with <code>task ui</code>, then rebuild slot.</p>",
        )
        .expect("write ui placeholder");
    }
}
