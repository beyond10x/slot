//! The limit on build processes per job (`slot job-parallel`).
//!
//! Without it a job's `cargo` and `make` read the pool's jobserver FIFO: a
//! job holds one slot and takes one more token for every compiler it starts,
//! so the compilers of running jobs compete with waiting jobs for each freed
//! token. With the limit `N` a job gets no pool jobserver. Its environment
//! names a bound of its own instead ([`crate::toolchain::own_jobserver`]):
//! `cargo` makes a private jobserver of that many tokens and `make` runs
//! that many recipes at once. The job still waits for and holds its slots;
//! its compilers take none. The pool then bounds jobs, not compilers.
//!
//! The bound of one job is the larger of `N` and the slots it holds
//! ([`for_job`]): a job that named 8 slots (`slot --tokens 8`) holds 8 of
//! its own and may use them.

use serde::{Deserialize, Deserializer, Serialize};

/// Body of `PUT /api/job-parallel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Build processes one job may run at once, at least 1; `null` for no
    /// limit. The field must be named.
    #[serde(deserialize_with = "named")]
    pub job_parallel: Option<usize>,
}

/// An `Option` that must be present: `null` is `None`, a missing field is an
/// error, so an empty body changes nothing instead of switching the limit
/// off.
fn named<'de, D: Deserializer<'de>>(d: D) -> Result<Option<usize>, D::Error> {
    Option::deserialize(d)
}

impl Request {
    /// The setting the request names, or why it is refused.
    pub fn setting(&self) -> Result<Option<usize>, String> {
        match self.job_parallel {
            Some(0) => {
                Err("slot: job_parallel must be at least 1; null turns the limit off".into())
            }
            n => Ok(n),
        }
    }
}

/// Answer of `GET` and `PUT /api/job-parallel`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// Build processes one job may run at once; `None` = no limit, each
    /// build process takes a slot of the pool.
    #[serde(default)]
    pub job_parallel: Option<usize>,
}

/// The argument of `slot job-parallel`: a count, or `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// No limit: jobs get the pool's jobserver.
    Off,
    /// Build processes one job may run at once.
    At(usize),
}

impl Setting {
    /// The value `limits.json` and the API carry.
    pub fn value(self) -> Option<usize> {
        match self {
            Self::Off => None,
            Self::At(n) => Some(n),
        }
    }
}

impl std::str::FromStr for Setting {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        if s == "off" {
            return Ok(Self::Off);
        }
        match s.parse::<usize>() {
            Ok(0) => Err("a job runs at least 1 build process; `off` for no limit".into()),
            Ok(n) => Ok(Self::At(n)),
            Err(_) => Err(format!(
                "`{s}` is not a count: give a number from 1, or `off`"
            )),
        }
    }
}

/// The bound of a job that holds `held` slots while the setting is
/// `setting`: the larger of the two; `None` with no setting.
pub fn for_job(setting: Option<usize>, held: usize) -> Option<usize> {
    setting.map(|n| n.max(held).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jobs_bound_is_the_setting_or_the_slots_it_holds_if_more() {
        assert_eq!(for_job(None, 1), None);
        assert_eq!(for_job(None, 8), None, "off: nothing changes for any job");
        assert_eq!(for_job(Some(4), 1), Some(4));
        assert_eq!(for_job(Some(4), 4), Some(4));
        assert_eq!(for_job(Some(4), 8), Some(8), "a job that holds 8 slots");
        assert_eq!(for_job(Some(1), 0), Some(1));
    }

    #[test]
    fn the_argument_is_a_count_from_one_or_off() {
        assert_eq!("4".parse(), Ok(Setting::At(4)));
        assert_eq!("1".parse(), Ok(Setting::At(1)));
        assert_eq!("off".parse(), Ok(Setting::Off));
        assert_eq!(Setting::At(4).value(), Some(4));
        assert_eq!(Setting::Off.value(), None);
        for bad in ["0", "-1", "four", "4.5", "", "OFF", "4 "] {
            assert!(bad.parse::<Setting>().is_err(), "{bad:?}");
        }
        assert!(
            "0".parse::<Setting>().unwrap_err().contains("at least 1"),
            "0 is refused with the reason"
        );
    }

    #[test]
    fn a_request_names_a_count_or_null_and_nothing_else() {
        let setting = |body: &str| {
            serde_json::from_str::<Request>(body)
                .map_err(|e| e.to_string())
                .and_then(|r| r.setting())
        };
        assert_eq!(setting(r#"{"job_parallel":4}"#), Ok(Some(4)));
        assert_eq!(setting(r#"{"job_parallel":null}"#), Ok(None));
        assert!(
            setting(r#"{"job_parallel":0}"#)
                .unwrap_err()
                .contains("at least 1")
        );
        for bad in [
            "{}",
            r#"{"job_parallel":-1}"#,
            r#"{"job_parallel":"4"}"#,
            r#"{"job_parallel":1.5}"#,
            r#"{"parallel":4}"#,
        ] {
            assert!(setting(bad).is_err(), "{bad}");
        }
        assert_eq!(
            serde_json::to_string(&Request {
                job_parallel: Some(4)
            })
            .unwrap(),
            r#"{"job_parallel":4}"#
        );
        assert_eq!(
            serde_json::to_string(&Request { job_parallel: None }).unwrap(),
            r#"{"job_parallel":null}"#
        );
        let old: View = serde_json::from_str("{}").unwrap();
        assert_eq!(old.job_parallel, None);
    }
}
