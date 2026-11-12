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

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
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
