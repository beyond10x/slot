//! Call mode: while a call holds the microphone or the camera, the daemon
//! raises the headroom, and puts it back a while after the call ends.
//!
//! A call is seen two ways:
//!
//! - PipeWire (`pw-dump`): an active link from a microphone or camera node
//!   (`Audio/Source*`, `Video/Source*`, not a sink monitor) to a capture
//!   stream (`Stream/Input/Audio`, `Stream/Input/Video`) in state `running`.
//!   A browser tab or a call app that keeps its stream open but corked is
//!   `idle` or `suspended`, so an open but unused microphone does not count.
//! - A process that holds `/dev/video*` open: browsers read the camera
//!   through V4L2 directly, not through PipeWire.
//!
//! Level meters and mixers capture too; their names are in
//! [`DEFAULT_IGNORE`] and the list can be replaced (`--call-ignore`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Applications that capture without being a call: level meters, mixers,
/// patchbays and filters.
pub const DEFAULT_IGNORE: &[&str] = &[
    "pavucontrol",
    "pwvucontrol",
    "easyeffects",
    "helvum",
    "qpwgraph",
    "coppwr",
    "noisetorch",
];

/// Processes that open cameras to list them or to serve them to PipeWire
/// clients; a PipeWire camera client is seen through its link instead.
const CAMERA_SERVICES: &[&str] = &["pipewire", "wireplumber", "pipewire-media-session"];

fn prop<'a>(props: &'a Value, key: &str) -> Option<&'a str> {
    props.get(key).and_then(Value::as_str)
}

fn flag(props: &Value, key: &str) -> bool {
    match props.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    }
}

fn ignored(name: &str, binary: &str, ignore: &[String]) -> bool {
    ignore.iter().any(|i| {
        let i = i.to_ascii_lowercase();
        name.to_ascii_lowercase() == i || binary.to_ascii_lowercase() == i
    })
}

/// Names of the applications that capture from a microphone or a camera in
/// a `pw-dump` document, sorted, without duplicates.
pub fn capturing_apps(dump: &Value, ignore: &[String]) -> Vec<String> {
    let Some(objects) = dump.as_array() else {
        return Vec::new();
    };
    let mut sources = HashSet::new();
    let mut streams: HashMap<i64, String> = HashMap::new();
    for o in objects {
        if o.get("type").and_then(Value::as_str) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let Some(id) = o.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let info = &o["info"];
        let props = &info["props"];
        let class = prop(props, "media.class").unwrap_or("");
        let name = prop(props, "node.name").unwrap_or("");
        if (class.starts_with("Audio/Source") || class.starts_with("Video/Source"))
            && !name.ends_with(".monitor")
        {
            sources.insert(id);
        }
        let capture =
            class.starts_with("Stream/Input/Audio") || class.starts_with("Stream/Input/Video");
        let running = info.get("state").and_then(Value::as_str) == Some("running");
        if capture
            && running
            && !flag(props, "stream.capture.sink")
            && !flag(props, "stream.monitor")
        {
            let app = prop(props, "application.name")
                .or_else(|| prop(props, "application.process.binary"))
                .or_else(|| prop(props, "node.name"))
                .unwrap_or("unknown");
            let binary = prop(props, "application.process.binary").unwrap_or("");
            if !ignored(app, binary, ignore) {
                streams.insert(id, app.to_string());
            }
        }
    }
    let mut apps = BTreeSet::new();
    for o in objects {
        if o.get("type").and_then(Value::as_str) != Some("PipeWire:Interface:Link") {
            continue;
        }
        let info = &o["info"];
        let out = info.get("output-node-id").and_then(Value::as_i64);
        let inp = info.get("input-node-id").and_then(Value::as_i64);
        let active = info.get("state").and_then(Value::as_str) == Some("active");
        if let (Some(out), Some(inp)) = (out, inp)
            && active
            && sources.contains(&out)
            && let Some(app) = streams.get(&inp)
        {
            apps.insert(app.clone());
        }
    }
    apps.into_iter().collect()
}

/// Command names of this user's processes that hold a `/dev/video*` device
/// open, except the PipeWire services.
pub fn camera_users(ignore: &[String]) -> Vec<String> {
    let mut out = BTreeSet::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for e in dir.flatten() {
        let name = e.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        let holds = fds.flatten().any(|f| {
            std::fs::read_link(f.path())
                .is_ok_and(|l| l.to_string_lossy().starts_with("/dev/video"))
        });
        if !holds {
            continue;
        }
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        let comm = comm.trim();
        if comm.is_empty() || CAMERA_SERVICES.contains(&comm) || ignored(comm, comm, ignore) {
            continue;
        }
        out.insert(comm.to_string());
    }
    out.into_iter().collect()
}

/// Runs `pw-dump` with a deadline; `None` when it is missing, fails or hangs.
pub fn pw_dump(deadline: Duration) -> Option<Value> {
    let mut child = Command::new("pw-dump")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if start.elapsed() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    serde_json::from_slice(&reader.join().ok()?).ok()
}

/// Every application in a call now: PipeWire capture streams and camera
/// holders, sorted, without duplicates.
pub fn probe(ignore: &[String]) -> Vec<String> {
    let mut apps: BTreeSet<String> = pw_dump(Duration::from_secs(3))
        .map(|d| capturing_apps(&d, ignore))
        .unwrap_or_default()
        .into_iter()
        .collect();
    apps.extend(camera_users(ignore));
    apps.into_iter().collect()
}

/// What the daemon does with the headroom after an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Leave it.
    Keep,
    /// A call started: raise the headroom to this, without saving it.
    Raise(u8),
    /// The call ended: put back the headroom from before the call.
    Restore(u8),
}

/// Call mode's state: whether a call is on, which headroom it replaced, and
/// whether the operator changed the headroom during it.
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    active: bool,
    raised: bool,
    manual: bool,
    base: u8,
    last_seen: Option<Instant>,
    apps: Vec<String>,
}

impl Tracker {
    /// Takes one probe result. `current` is the headroom in force,
    /// `call_percent` the headroom a call gets (0 = call mode off), `off_after`
    /// how long no capture must be seen before the call counts as ended.
    pub fn observe(
        &mut self,
        now: Instant,
        apps: Vec<String>,
        current: u8,
        call_percent: u8,
        off_after: Duration,
    ) -> Action {
        if !apps.is_empty() {
            self.last_seen = Some(now);
            self.apps = apps;
            if !self.active {
                self.active = true;
                self.manual = false;
                self.base = current;
                self.raised = call_percent > current;
                if self.raised {
                    return Action::Raise(call_percent);
                }
            }
            return Action::Keep;
        }
        let ended = self
            .last_seen
            .is_none_or(|t| now.duration_since(t) >= off_after);
        if self.active && ended {
            let restore = self.raised && !self.manual;
            self.active = false;
            self.raised = false;
            self.manual = false;
            self.apps.clear();
            if restore {
                return Action::Restore(self.base);
            }
        }
        Action::Keep
    }

    /// The operator set the headroom: during a call that value stays, and
    /// the end of the call does not change it.
    pub fn manual(&mut self) {
        if self.active {
            self.manual = true;
        }
    }

    /// The call headroom changed while a call is on; what to set now.
    pub fn retarget(&mut self, call_percent: u8) -> Action {
        if !self.active || self.manual {
            return Action::Keep;
        }
        if call_percent > self.base {
            self.raised = true;
            Action::Raise(call_percent)
        } else if self.raised {
            self.raised = false;
            Action::Restore(self.base)
        } else {
            Action::Keep
        }
    }

    /// Whether a call is on.
    pub fn active(&self) -> bool {
        self.active
    }

    /// Whether the operator changed the headroom during this call.
    pub fn is_manual(&self) -> bool {
        self.manual
    }

    /// The headroom from before the call.
    pub fn base(&self) -> u8 {
        self.base
    }

    /// Applications seen in the call.
    pub fn apps(&self) -> &[String] {
        &self.apps
    }

    /// Seconds until the headroom goes back, while no capture is seen.
    pub fn ends_in(&self, now: Instant, off_after: Duration) -> Option<u64> {
        let t = self.last_seen?;
        if !self.active || now.duration_since(t) < Duration::from_secs(6) {
            return None;
        }
        Some(off_after.saturating_sub(now.duration_since(t)).as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn defaults() -> Vec<String> {
        DEFAULT_IGNORE.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn an_idle_desktop_is_no_call() {
        assert_eq!(
            capturing_apps(&fixture("pw-idle.json"), &defaults()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_running_capture_from_the_microphone_is_a_call() {
        assert_eq!(
            capturing_apps(&fixture("pw-call.json"), &defaults()),
            vec!["Chromium".to_string()]
        );
    }

    #[test]
    fn a_corked_stream_a_level_meter_and_a_sink_monitor_are_no_call() {
        assert_eq!(
            capturing_apps(&fixture("pw-not-a-call.json"), &defaults()),
            Vec::<String>::new()
        );
        let none: Vec<String> = Vec::new();
        assert_eq!(
            capturing_apps(&fixture("pw-not-a-call.json"), &none),
            vec!["pavucontrol".to_string()],
            "only the ignore list keeps the level meter out"
        );
    }

    #[test]
    fn a_call_raises_and_its_end_restores_after_the_delay() {
        let t0 = Instant::now();
        let off = Duration::from_secs(120);
        let call = || vec!["Chromium".to_string()];
        let mut t = Tracker::default();
        assert_eq!(t.observe(t0, vec![], 20, 40, off), Action::Keep);
        assert_eq!(t.observe(t0, call(), 20, 40, off), Action::Raise(40));
        assert!(t.active());
        assert_eq!(
            t.observe(t0 + Duration::from_secs(5), call(), 40, 40, off),
            Action::Keep
        );
        let quiet = t0 + Duration::from_secs(5);
        assert_eq!(
            t.observe(quiet + Duration::from_secs(60), vec![], 40, 40, off),
            Action::Keep,
            "a pause shorter than the delay keeps the call"
        );
        assert_eq!(t.ends_in(quiet + Duration::from_secs(60), off), Some(60));
        assert_eq!(
            t.observe(quiet + Duration::from_secs(120), vec![], 40, 40, off),
            Action::Restore(20)
        );
        assert!(!t.active());
    }

    #[test]
    fn a_headroom_already_above_the_call_value_stays() {
        let t0 = Instant::now();
        let off = Duration::from_secs(120);
        let mut t = Tracker::default();
        assert_eq!(t.observe(t0, vec!["a".into()], 50, 40, off), Action::Keep);
        assert_eq!(
            t.observe(t0 + off, vec![], 50, 40, off),
            Action::Keep,
            "nothing was raised, so nothing is put back"
        );
    }

    #[test]
    fn a_manual_change_during_a_call_wins() {
        let t0 = Instant::now();
        let off = Duration::from_secs(120);
        let mut t = Tracker::default();
        assert_eq!(
            t.observe(t0, vec!["a".into()], 20, 40, off),
            Action::Raise(40)
        );
        t.manual();
        assert_eq!(t.retarget(45), Action::Keep);
        assert_eq!(t.observe(t0 + off, vec![], 10, 40, off), Action::Keep);
        assert!(!t.active());
        assert_eq!(
            t.observe(t0 + off * 2, vec!["a".into()], 10, 40, off),
            Action::Raise(40),
            "the next call raises again"
        );
    }

    #[test]
    fn call_mode_off_raises_nothing_and_turning_it_off_restores() {
        let t0 = Instant::now();
        let off = Duration::from_secs(120);
        let mut t = Tracker::default();
        assert_eq!(t.observe(t0, vec!["a".into()], 20, 0, off), Action::Keep);
        assert_eq!(t.retarget(40), Action::Raise(40));
        assert_eq!(t.retarget(0), Action::Restore(20));
    }
}
