//! Headroom: the share of the CPU kept free for interactive work.
//!
//! A headroom of `h` percent lowers the number of circulating jobserver
//! tokens to `tokens * (100 - h) / 100` and, when the daemon runs in a systemd
//! user unit, sets `CPUQuota` on that unit to `(100 - h) * cores` percent. Every
//! job runs inside the unit's cgroup, so the quota is a hard cap on all of them
//! together, test binaries and compile servers started by a job included.

use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

/// Largest headroom accepted, percent.
pub const MAX_HEADROOM: u8 = 50;

/// File in the state directory that keeps the headroom across restarts.
pub const FILE: &str = "limits.json";

/// What is saved in [`FILE`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// Headroom, percent.
    pub headroom_percent: u8,
    /// Headroom during a call, percent, 0 = call mode off; `None` = the
    /// daemon's `--call-headroom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_headroom_percent: Option<u8>,
}

/// Reads the saved headroom; 0 when the file is missing or unreadable.
pub fn load(dir: &Path) -> Saved {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<Saved>(&s).ok())
        .map(|s| Saved {
            headroom_percent: s.headroom_percent.min(MAX_HEADROOM),
            call_headroom_percent: s.call_headroom_percent.map(|c| c.min(MAX_HEADROOM)),
        })
        .unwrap_or_default()
}

/// Writes the headroom.
pub fn save(dir: &Path, saved: Saved) -> std::io::Result<()> {
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(&saved)?)?;
    std::fs::rename(tmp, dir.join(FILE))
}

/// Tokens that circulate with `headroom` percent kept free; at least 1.
pub fn effective_tokens(tokens: usize, headroom: u8) -> usize {
    let keep = 100 - usize::from(headroom.min(MAX_HEADROOM));
    ((tokens * keep + 50) / 100).max(1)
}

/// The `CPUQuota` percent for `headroom` on `cores` CPUs; `None` for no cap.
pub fn cpu_quota_percent(cores: u32, headroom: u8) -> Option<u32> {
    let h = u32::from(headroom.min(MAX_HEADROOM));
    (h > 0).then(|| (100 - h) * cores)
}

/// Sets or clears `CPUQuota` on `unit`. `Err` carries the reason.
pub fn apply_quota(unit: Option<&str>, quota: Option<u32>) -> Result<(), String> {
    let Some(unit) = unit else {
        return Err("not running in a systemd user unit".into());
    };
    let value = quota.map_or_else(|| "CPUQuota=".to_string(), |q| format!("CPUQuota={q}%"));
    let ok = Command::new("systemctl")
        .args(["--user", "set-property", "--runtime", unit, &value])
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok(())
    } else {
        Err(format!("systemctl set-property {unit} {value} failed"))
    }
}

/// Body of `PUT /api/limits`; a field left out is not changed.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Request {
    /// Headroom, percent, 0 to [`MAX_HEADROOM`].
    #[serde(default)]
    pub headroom_percent: Option<u8>,
    /// Headroom during a call, percent, 0 to [`MAX_HEADROOM`]; 0 turns call
    /// mode off.
    #[serde(default)]
    pub call_headroom_percent: Option<u8>,
}

/// Call mode as `GET /api/limits` shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CallView {
    /// Whether a call holds the microphone or the camera now.
    pub active: bool,
    /// Applications seen capturing.
    pub apps: Vec<String>,
    /// Headroom before the call, which comes back after it.
    pub base_percent: u8,
    /// Whether the headroom was set by hand during this call, so it stays.
    pub manual: bool,
    /// Seconds until the headroom goes back, once no capture is seen.
    pub ends_in_s: Option<u64>,
    /// Why call detection does not run; empty when it does.
    pub note: String,
}

/// Answer of `GET` and `PUT /api/limits`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// Headroom, percent.
    pub headroom_percent: u8,
    /// `CPUQuota` set on the daemon's unit, percent; `None` when not capped.
    pub cpu_quota_percent: Option<u32>,
    /// Tokens that circulate.
    pub effective_tokens: usize,
    /// Pool size configured at start.
    pub tokens: usize,
    /// Whether the daemon runs in a systemd user unit, so the quota applies.
    pub systemd: bool,
    /// Why the quota is not applied; empty when it is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Headroom during a call, percent; 0 = call mode off. 0 from a daemon
    /// older than 0.5.0.
    #[serde(default)]
    pub call_headroom_percent: u8,
    /// Call mode now.
    #[serde(default)]
    pub call: CallView,
}

/// Whether a browser request from `origin` may change settings. A request
/// without `Origin` comes from a program such as curl or `slot headroom`.
/// A page served by the daemon itself sends its own origin.
pub fn origin_allowed(origin: Option<&str>, port: u16) -> bool {
    match origin {
        None => true,
        Some(o) => {
            o == format!("http://127.0.0.1:{port}") || o == format!("http://localhost:{port}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_quota_follow_headroom() {
        assert_eq!(effective_tokens(20, 0), 20);
        assert_eq!(effective_tokens(20, 25), 15);
        assert_eq!(effective_tokens(20, 30), 14);
        assert_eq!(effective_tokens(20, 50), 10);
        assert_eq!(effective_tokens(20, 90), 10, "capped at 50 %");
        assert_eq!(effective_tokens(1, 50), 1);
        assert_eq!(effective_tokens(3, 50), 2);
        assert_eq!(cpu_quota_percent(20, 0), None);
        assert_eq!(cpu_quota_percent(20, 30), Some(1400));
        assert_eq!(cpu_quota_percent(8, 50), Some(400));
    }

    #[test]
    fn headroom_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).headroom_percent, 0);
        save(
            dir.path(),
            Saved {
                headroom_percent: 35,
                call_headroom_percent: None,
            },
        )
        .unwrap();
        assert_eq!(load(dir.path()).headroom_percent, 35);
        assert_eq!(load(dir.path()).call_headroom_percent, None);
        save(
            dir.path(),
            Saved {
                headroom_percent: 20,
                call_headroom_percent: Some(0),
            },
        )
        .unwrap();
        assert_eq!(load(dir.path()).call_headroom_percent, Some(0));
        std::fs::write(dir.path().join(FILE), r#"{"headroom_percent":99}"#).unwrap();
        assert_eq!(load(dir.path()).headroom_percent, MAX_HEADROOM);
        std::fs::write(dir.path().join(FILE), "garbage").unwrap();
        assert_eq!(load(dir.path()).headroom_percent, 0);
    }

    #[test]
    fn only_the_daemons_own_page_may_change_settings() {
        assert!(origin_allowed(None, 7411));
        assert!(origin_allowed(Some("http://127.0.0.1:7411"), 7411));
        assert!(origin_allowed(Some("http://localhost:7411"), 7411));
        assert!(!origin_allowed(Some("http://127.0.0.1:8080"), 7411));
        assert!(!origin_allowed(Some("https://example.com"), 7411));
        assert!(!origin_allowed(Some("null"), 7411));
    }
}
