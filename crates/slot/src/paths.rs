//! Default locations.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The user's runtime directory: `$XDG_RUNTIME_DIR`, else `/run/user/<uid>`
/// when it exists, so a program started with a cleared environment still
/// finds the daemon's socket.
pub fn runtime_dir() -> Option<PathBuf> {
    runtime_dir_from(
        std::env::var_os("XDG_RUNTIME_DIR"),
        Path::new("/run/user"),
        nix::unistd::getuid().as_raw(),
    )
}

fn runtime_dir_from(xdg: Option<OsString>, run_user: &Path, uid: u32) -> Option<PathBuf> {
    if let Some(d) = xdg.filter(|d| !d.is_empty()) {
        return Some(d.into());
    }
    let d = run_user.join(uid.to_string());
    d.is_dir().then_some(d)
}

/// Default daemon socket: `slot.sock` in the runtime directory, else
/// `slot-<uid>.sock` in the temporary directory.
pub fn default_socket() -> PathBuf {
    runtime_dir().map_or_else(
        || std::env::temp_dir().join(format!("slot-{}.sock", nix::unistd::getuid())),
        |d| d.join("slot.sock"),
    )
}

/// Default state directory: `$XDG_STATE_HOME/slot` or `~/.local/state/slot`.
pub fn default_state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(d).join("slot");
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".local").join("state").join("slot")
}

/// Default remote workspace root, relative to the remote user's home.
pub const DEFAULT_REMOTE_WSROOT: &str = ".local/state/slot/ws";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_xdg_runtime_dir_the_runtime_dir_is_run_user_uid() {
        let root = tempfile::tempdir().unwrap();
        let run_user = root.path();
        assert_eq!(runtime_dir_from(None, run_user, 1000), None);
        std::fs::create_dir(run_user.join("1000")).unwrap();
        assert_eq!(
            runtime_dir_from(None, run_user, 1000),
            Some(run_user.join("1000"))
        );
        assert_eq!(
            runtime_dir_from(Some("".into()), run_user, 1000),
            Some(run_user.join("1000")),
            "an empty variable counts as unset"
        );
        assert_eq!(
            runtime_dir_from(Some("/x".into()), run_user, 1000),
            Some(PathBuf::from("/x"))
        );
    }
}
