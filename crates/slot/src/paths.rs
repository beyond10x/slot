//! Default locations.

use std::path::PathBuf;

/// Default daemon socket: `$XDG_RUNTIME_DIR/slot.sock`.
pub fn default_socket() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(d).join("slot.sock");
    }
    std::env::temp_dir().join(format!("slot-{}.sock", nix::unistd::getuid()))
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
