//! The run-time limit: how long a job may run in the pool before the daemon
//! ends it.
//!
//! The limit counts from the job's start; the wait in the queue is not part
//! of it. A job's limit is the time it names itself (`slot --timeout`,
//! `SLOT_TIMEOUT`), else the daemon's limit. What a job names is capped at
//! the daemon's maximum. With no daemon limit and no `--timeout` a job has no
//! limit. A `timeout` command in front of the job's command extends nothing:
//! the daemon reads its time only to tell the client so.

use serde::{Deserialize, Serialize};

/// The most a job may name when the daemon has no `run_limit_max_s`, seconds.
pub const DEFAULT_MAX_S: u64 = 3600;

/// Exit code of a job ended at its limit, as `timeout` uses it.
pub const EXIT: i32 = 124;

/// The daemon's limit and the most a job may name, as kept in
/// `limits.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// Seconds a job may run; `None` = no limit.
    pub run_limit_s: Option<u64>,
    /// The most a job may name, seconds; `None` = [`DEFAULT_MAX_S`].
    pub run_limit_max_s: Option<u64>,
}

impl Limits {
    /// The most a job may name, seconds.
    pub fn max_s(&self) -> u64 {
        self.run_limit_max_s.unwrap_or(DEFAULT_MAX_S)
    }

    /// What `GET /api/run-limit` answers.
    pub fn view(&self) -> View {
        View {
            run_limit_s: self.run_limit_s,
            run_limit_max_s: self.max_s(),
        }
    }

    /// The limits after `req`, or why it is refused. A field left out keeps
    /// its value.
    pub fn changed(&self, req: &Request) -> Result<Self, String> {
        let mut to = *self;
        match req.run_limit_s {
            Some(0) => to.run_limit_s = None,
            Some(s) => to.run_limit_s = Some(s),
            None => {}
        }
        match req.run_limit_max_s {
            Some(0) => return Err("slot: run_limit_max_s must be at least 1".into()),
            Some(s) => to.run_limit_max_s = Some(s),
            None => {}
        }
        if req.run_limit_s.is_none() && req.run_limit_max_s.is_none() {
            return Err("slot: name run_limit_s, run_limit_max_s or both".into());
        }
        if let Some(limit) = to.run_limit_s
            && limit > to.max_s()
        {
            return Err(format!(
                "slot: the limit of {} is longer than {}, the most a job may name; raise that too \
                 (`slot run-limit {} --max {}`)",
                text(limit),
                text(to.max_s()),
                text(limit),
                text(limit),
            ));
        }
        Ok(to)
    }
}

/// Body of `PUT /api/run-limit`; a field left out is not changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Seconds a job may run; 0 = no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_limit_s: Option<u64>,
    /// The most a job may name, seconds; at least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_limit_max_s: Option<u64>,
}

/// Answer of `GET` and `PUT /api/run-limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// Seconds a job may run; `None` = no limit.
    #[serde(default)]
    pub run_limit_s: Option<u64>,
    /// The most a job may name, seconds.
    pub run_limit_max_s: u64,
}

/// The limit argument of `slot run-limit`: a duration, or `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// No limit.
    Off,
    /// Seconds a job may run.
    Seconds(u64),
}

impl std::str::FromStr for Setting {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        if s == "off" {
            Ok(Self::Off)
        } else {
            parse_duration(s)
                .map(Self::Seconds)
                .map_err(|e| format!("{e}, or `off`"))
        }
    }
}

/// Seconds of a duration as the command line takes it: plain seconds, or a
/// number followed by `s`, `m` or `h` (`90s`, `10m`, `1h`). At least 1 s.
pub fn parse_duration(s: &str) -> Result<u64, String> {
    let bad = || {
        format!("`{s}` is not a duration: give seconds, or a number with s, m or h (90s, 10m, 1h)")
    };
    let (digits, unit) = match s.as_bytes().last() {
        Some(b's') => (&s[..s.len() - 1], 1),
        Some(b'm') => (&s[..s.len() - 1], 60),
        Some(b'h') => (&s[..s.len() - 1], 3600),
        _ => (s, 1),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let secs = digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(unit))
        .ok_or_else(bad)?;
    if secs == 0 {
        return Err(format!("`{s}`: a duration is at least 1 s"));
    }
    Ok(secs)
}

/// `secs` in words: `45s`, `10m`, `1m30s`, `1h`, `1h30m`.
pub fn text(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    match (h, m, s) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m{s:02}s"),
        (h, 0, 0) => format!("{h}h"),
        (h, m, 0) => format!("{h}h{m:02}m"),
        (h, m, s) => format!("{h}h{m:02}m{s:02}s"),
    }
}

/// The time of the coreutils `timeout` command `argv` starts with, seconds,
/// rounded up: `timeout` is the first word or the second (`nice timeout …`),
/// and its options are skipped. `None` when `argv` starts with no `timeout`
/// or its duration does not read.
pub fn wrapper_timeout(argv: &[String]) -> Option<u64> {
    let is_timeout = |a: &String| a.rsplit('/').next() == Some("timeout");
    let at = argv.iter().take(2).position(is_timeout)?;
    let mut args = argv[at + 1..].iter();
    let duration = loop {
        let a = args.next()?;
        match a.as_str() {
            "--" => break args.next()?,
            // The option's value is the next word.
            "-s" | "-k" | "--signal" | "--kill-after" => {
                args.next()?;
            }
            // `--signal=INT`, `-k5`, `--foreground`, `-v` and the like.
            o if o.starts_with('-') => {}
            _ => break a,
        }
    };
    let (number, unit) = match duration.as_bytes().last()? {
        b's' => (&duration[..duration.len() - 1], 1.0),
        b'm' => (&duration[..duration.len() - 1], 60.0),
        b'h' => (&duration[..duration.len() - 1], 3600.0),
        b'd' => (&duration[..duration.len() - 1], 86400.0),
        _ => (duration.as_str(), 1.0),
    };
    // `f64::from_str` also reads `inf` and `nan`, which name no time.
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    let secs = number.parse::<f64>().ok()? * unit;
    (secs.is_finite() && secs < 1e15).then(|| secs.ceil() as u64)
}

/// The limit of a job that named `own`, under the daemon's `default` and
/// `max`, seconds; `None` = the job has no limit.
pub fn job_limit(own: Option<u64>, default: Option<u64>, max: u64) -> Option<u64> {
    own.map(|own| own.min(max)).or(default)
}

/// `secs` as `slot --timeout` takes it: `45s`, `30m`, `1h`, `5400s`.
fn arg(secs: u64) -> String {
    match secs {
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// What the client of a job with the limit `limit` is told when its command
/// starts with a `timeout` that names more ([`wrapper_timeout`]): that time
/// extends nothing, so a caller who relies on it learns at submission that
/// the job ends earlier, and how to name the time. `None` for a job with no
/// limit or a `timeout` inside it.
pub fn wrapped_message(limit: Option<u64>, argv: &[String], max: u64) -> Option<String> {
    let limit = limit?;
    let wrapped = wrapper_timeout(argv).filter(|&w| w > limit)?;
    let ends = text(limit);
    Some(if limit < max {
        format!(
            "slot: this job ends after {ends}; a wrapped `timeout` does not extend it, name the \
             time with `slot --timeout {}` (at most {})",
            arg(wrapped.min(max)),
            text(max)
        )
    } else {
        format!(
            "slot: this job ends after {ends}, the most a job may name; a wrapped `timeout` does \
             not extend it"
        )
    })
}

/// What the client of a job with a `--timeout` over the maximum is told.
pub fn capped_message(asked: u64, max: u64) -> String {
    format!(
        "slot: --timeout {} capped at {}, the most a job may name (`slot run-limit`)",
        text(asked),
        text(max)
    )
}

/// What the client of job `id` is told when the job is ended at its limit
/// of `limit` seconds after running `ran_ms`.
pub fn ended_message(id: i64, ran_ms: i64, limit: u64, max: u64) -> String {
    let ran = text(u64::try_from(ran_ms / 1000).unwrap_or(0));
    let longer = if limit < max {
        format!(
            "name a longer one with `slot --timeout <DURATION>`, at most {}",
            text(max)
        )
    } else {
        "that is the most a job may name (`slot run-limit`)".to_string()
    };
    format!(
        "slot: job {id} ended after {ran}: over its run-time limit of {}; {longer}",
        text(limit)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn durations_are_seconds_or_a_number_with_s_m_or_h() {
        for (s, secs) in [
            ("90", 90),
            ("90s", 90),
            ("10m", 600),
            ("1h", 3600),
            ("1", 1),
            ("120m", 7200),
        ] {
            assert_eq!(parse_duration(s), Ok(secs), "{s}");
        }
        for bad in [
            "",
            "0",
            "0m",
            "soon",
            "-5",
            "1.5h",
            "10 m",
            "1d",
            "m",
            "1h30m",
            "+5",
            "99999999999999999999",
            "9999999999999999999h",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
        assert_eq!("off".parse(), Ok(Setting::Off));
        assert_eq!("10m".parse(), Ok(Setting::Seconds(600)));
        assert!("never".parse::<Setting>().is_err());
        assert!("0".parse::<Setting>().is_err(), "off is spelled out");
    }

    #[test]
    fn a_duration_in_words() {
        for (secs, words) in [
            (0, "0s"),
            (45, "45s"),
            (90, "1m30s"),
            (600, "10m"),
            (3600, "1h"),
            (5400, "1h30m"),
            (3661, "1h01m01s"),
            (7200, "2h"),
        ] {
            assert_eq!(text(secs), words);
        }
    }

    #[test]
    fn the_time_of_a_timeout_command_the_job_starts_with() {
        for (cmd, secs) in [
            ("timeout 1800 cargo test", Some(1800)),
            ("timeout -s INT 2700 x", Some(2700)),
            ("timeout -k 5 1h x", Some(3600)),
            ("timeout 290 x", Some(290)),
            ("timeout --signal TERM --preserve-status 1.5m x", Some(90)),
            (
                "/usr/bin/timeout --signal=KILL --kill-after 10 30m x",
                Some(1800),
            ),
            ("timeout -k5 -v --foreground 45s x", Some(45)),
            ("timeout -- 20 x", Some(20)),
            ("timeout 1d x", Some(86400)),
            ("timeout 0.2 x", Some(1)),
            ("timeout 0 x", Some(0)),
            ("nice timeout 1800 x", Some(1800)),
            ("env /usr/bin/timeout 2h x", Some(7200)),
            ("cargo test", None),
            ("cargo run timeout 1800", None),
            ("sh -c timeout 1800 x", None),
            ("timeout", None),
            ("timeout -s", None),
            ("timeout -k 5", None),
            ("timeout soon x", None),
            ("timeout inf x", None),
            ("timeout -5 x", None),
            ("my-timeout 1800 x", None),
        ] {
            assert_eq!(wrapper_timeout(&argv(cmd)), secs, "{cmd}");
        }
    }

    #[test]
    fn a_jobs_limit_is_its_own_time_else_the_daemons() {
        // own --timeout, the daemon's limit → the job's limit; the most a job
        // may name is 1 h.
        for (own, default, limit) in [
            (None, None, None),
            (None, Some(600), Some(600)),
            (Some(1800), Some(600), Some(1800)),
            (Some(1800), None, Some(1800)),
            (Some(60), Some(600), Some(60)),
            (Some(3600), Some(600), Some(3600)),
            (Some(7200), Some(600), Some(3600)),
            (Some(7200), None, Some(3600)),
        ] {
            assert_eq!(
                job_limit(own, default, 3600),
                limit,
                "--timeout {own:?}, daemon {default:?}"
            );
        }
    }

    #[test]
    fn a_wrapped_timeout_over_the_jobs_limit_gets_one_line() {
        // the job's limit, command → the time the line tells the caller to
        // name; the most a job may name is 1 h.
        for (limit, cmd, named) in [
            (Some(600), "timeout 1800 cargo test", Some("30m")),
            (Some(600), "timeout -s INT 2700 x", Some("45m")),
            (Some(600), "timeout -k 5 1h x", Some("1h")),
            (Some(600), "timeout 2h x", Some("1h")),
            (Some(600), "timeout 601 x", Some("601s")),
            (Some(600), "nice timeout 1800 x", Some("30m")),
            (Some(900), "timeout 1800 x", Some("30m")),
            (Some(600), "timeout 290 x", None),
            (Some(600), "timeout 600 x", None),
            (Some(600), "timeout 0 x", None),
            (Some(600), "timeout soon x", None),
            (Some(600), "cargo test", None),
            (Some(600), "cargo run timeout 1800", None),
            (Some(600), "sh -c timeout 1800 x", None),
            (None, "timeout 1800 x", None),
        ] {
            let line = wrapped_message(limit, &argv(cmd), 3600);
            let want = named.map(|n| {
                format!(
                    "slot: this job ends after {}; a wrapped `timeout` does not extend it, name \
                     the time with `slot --timeout {n}` (at most 1h)",
                    text(limit.unwrap())
                )
            });
            assert_eq!(line, want, "limit {limit:?}, `{cmd}`");
            if let Some(n) = named {
                assert!(parse_duration(n).is_ok(), "`--timeout {n}` must parse");
            }
        }
        assert_eq!(
            wrapped_message(Some(3600), &argv("timeout 2h x"), 3600).as_deref(),
            Some(
                "slot: this job ends after 1h, the most a job may name; a wrapped `timeout` does \
                 not extend it"
            )
        );
    }

    #[test]
    fn a_change_keeps_what_it_does_not_name_and_refuses_a_limit_over_the_maximum() {
        let none = Limits::default();
        assert_eq!(
            none.view(),
            View {
                run_limit_s: None,
                run_limit_max_s: 3600
            }
        );
        let req = |limit, max| Request {
            run_limit_s: limit,
            run_limit_max_s: max,
        };
        let ten = none.changed(&req(Some(600), None)).unwrap();
        assert_eq!((ten.run_limit_s, ten.run_limit_max_s), (Some(600), None));
        let wide = ten.changed(&req(None, Some(7200))).unwrap();
        assert_eq!((wide.run_limit_s, wide.max_s()), (Some(600), 7200));
        let off = wide.changed(&req(Some(0), None)).unwrap();
        assert_eq!(
            (off.run_limit_s, off.max_s()),
            (None, 7200),
            "0 turns it off"
        );
        let both = none.changed(&req(Some(7200), Some(7200))).unwrap();
        assert_eq!((both.run_limit_s, both.max_s()), (Some(7200), 7200));
        for (from, bad) in [
            (none, req(None, None)),
            (none, req(Some(3601), None)),
            (none, req(Some(600), Some(0))),
            (none, req(Some(600), Some(300))),
            (ten, req(None, Some(300))),
        ] {
            let why = from.changed(&bad).unwrap_err();
            assert!(why.starts_with("slot: "), "{bad:?}: {why}");
        }
    }

    #[test]
    fn what_the_client_is_told() {
        assert_eq!(
            ended_message(7, 602_400, 600, 3600),
            "slot: job 7 ended after 10m02s: over its run-time limit of 10m; name a longer one \
             with `slot --timeout <DURATION>`, at most 1h"
        );
        let at_max = ended_message(7, 3_600_900, 3600, 3600);
        assert!(
            at_max.contains("limit of 1h; that is the most a job may name"),
            "{at_max}"
        );
        assert!(!at_max.contains("--timeout"), "{at_max}");
        assert_eq!(
            capped_message(7200, 3600),
            "slot: --timeout 2h capped at 1h, the most a job may name (`slot run-limit`)"
        );
    }
}
