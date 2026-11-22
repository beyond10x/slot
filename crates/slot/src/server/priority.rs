//! Priority rules: the queue level of every job of a repository, set at run
//! time with `slot priority set` and kept across restarts in [`FILE`].
//!
//! A job's level is the rule for its repository, else the level it asked
//! for itself (`--priority`, `SLOT_PRIORITY`), else `normal`. A job that
//! has waited longer than the promotion delay is ordered as `high`, so no
//! level waits forever. The delay is the daemon's `--promote-after` until
//! `slot priority promote-after` sets one at run time, kept in [`FILE`] too.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use crate::types::Priority;

/// File in the state directory that keeps the rules across restarts.
pub const FILE: &str = "priority.json";

/// One rule: every job of a repository `pattern` matches gets `level`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Repository name, or a glob with `*`.
    pub pattern: String,
    /// The level.
    pub level: Priority,
}

/// What is saved in [`FILE`] and what the daemon orders the queue by: the
/// rules and the promotion delay set at run time. A field the reader does
/// not know is ignored, so an older daemon reads a newer file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// The rules, in the order they were set.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Seconds after which a queued job is ordered as `high`, set with
    /// `slot priority promote-after`; 0 never promotes. `None`, and absent
    /// from the file: the daemon's `--promote-after` applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promote_after_s: Option<u64>,
}

impl Table {
    /// The promotion delay in force: the one set at run time, else
    /// `option`, the daemon's `--promote-after`.
    pub fn promote_after(&self, option: Duration) -> Duration {
        self.promote_after_s.map_or(option, Duration::from_secs)
    }

    /// Sets the delay, or returns to the daemon's option. Returns whether
    /// the table changed.
    pub fn set_promote_after(&mut self, to: PromoteAfter) -> bool {
        let to = match to {
            PromoteAfter::Seconds(s) => Some(s),
            PromoteAfter::Default => None,
        };
        std::mem::replace(&mut self.promote_after_s, to) != to
    }
}

/// Reads the saved table; empty when the file is missing or unreadable.
pub fn load(dir: &Path) -> Table {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes the table.
pub fn save(dir: &Path, table: &Table) -> std::io::Result<()> {
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(table)?)?;
    std::fs::rename(tmp, dir.join(FILE))
}

/// Sets the rule for `pattern` to `level`, or removes it for `None`.
/// Returns whether the table changed.
pub fn set(rules: &mut Vec<Rule>, pattern: &str, level: Option<Priority>) -> bool {
    let at = rules.iter().position(|r| r.pattern == pattern);
    match (at, level) {
        (Some(i), Some(l)) if rules[i].level == l => false,
        (Some(i), Some(l)) => {
            rules[i].level = l;
            true
        }
        (Some(i), None) => {
            rules.remove(i);
            true
        }
        (None, Some(l)) => {
            rules.push(Rule {
                pattern: pattern.to_string(),
                level: l,
            });
            true
        }
        (None, None) => false,
    }
}

/// Whether `name` matches `pattern`: `*` is any run of characters, anything
/// else itself.
pub fn glob(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == n[ni] {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// The rule for repository `repo`: the one naming it exactly, else the
/// longest glob that matches it, the earliest set on a tie.
pub fn rule_for<'a>(rules: &'a [Rule], repo: &str) -> Option<&'a Rule> {
    if let Some(r) = rules.iter().find(|r| r.pattern == repo) {
        return Some(r);
    }
    rules.iter().filter(|r| glob(&r.pattern, repo)).fold(
        None,
        |best: Option<&Rule>, r| match best {
            Some(b) if b.pattern.len() >= r.pattern.len() => Some(b),
            _ => Some(r),
        },
    )
}

/// The level a queued job of `repo` is ordered by, and whether it is
/// `high` only because it has waited `waited`, longer than `promote_after`
/// (zero never promotes). A rule wins over the job's own level `own`.
pub fn level(
    rules: &[Rule],
    repo: &str,
    own: Option<Priority>,
    waited: Duration,
    promote_after: Duration,
) -> (Priority, bool) {
    let set = rule_for(rules, repo).map_or_else(|| own.unwrap_or_default(), |r| r.level);
    if set > Priority::High && !promote_after.is_zero() && waited > promote_after {
        return (Priority::High, true);
    }
    (set, false)
}

/// The value of `slot priority promote-after` and of `promote_after_s` in
/// the body of `PUT /api/priority`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoteAfter {
    /// A queued job is ordered as `high` after this many seconds; 0 never
    /// promotes. In JSON a number.
    Seconds(u64),
    /// The daemon's `--promote-after` applies again. In JSON `null`.
    Default,
}

impl std::str::FromStr for PromoteAfter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        if s == "default" {
            return Ok(Self::Default);
        }
        s.parse()
            .map(Self::Seconds)
            .map_err(|_| format!("{s:?} is neither a number of seconds nor `default`"))
    }
}

impl Serialize for PromoteAfter {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Seconds(n) => s.serialize_u64(*n),
            Self::Default => s.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for PromoteAfter {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Option::<u64>::deserialize(d).map(|s| s.map_or(Self::Default, Self::Seconds))
    }
}

/// A field that is present, `null` included: `None` is left to the field's
/// default, a field left out.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<PromoteAfter>, D::Error> {
    PromoteAfter::deserialize(d).map(Some)
}

/// Body of `PUT /api/priority`. With `pattern`: the rule for it gets
/// `level`; a missing or null `level` removes it. With `promote_after_s`:
/// the promotion delay is set, or with `null` the daemon's
/// `--promote-after` applies again. A body names one of the two or both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Request {
    /// Repository name or glob.
    #[serde(default)]
    pub pattern: Option<String>,
    /// The level; `None` removes the rule.
    #[serde(default)]
    pub level: Option<Priority>,
    /// The promotion delay; `None`, a field left out, does not change it.
    #[serde(default, deserialize_with = "present")]
    pub promote_after_s: Option<PromoteAfter>,
}

impl Request {
    /// Why the daemon refuses this body, if it does.
    pub fn refused(&self) -> Option<&'static str> {
        match self.pattern.as_deref().map(str::trim) {
            Some("") => Some("slot: the pattern is empty"),
            None if self.level.is_some() => Some("slot: a level needs a pattern"),
            None if self.promote_after_s.is_none() => {
                Some("slot: the body names neither pattern nor promote_after_s")
            }
            _ => None,
        }
    }
}

/// Answer of `GET` and `PUT /api/priority`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The rules, in the order they were set.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Seconds after which a queued job is ordered as `high`, as in force
    /// now (the delay set at run time, else the daemon's
    /// `--promote-after`); 0 = never.
    #[serde(default)]
    pub promote_after_s: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, level: Priority) -> Rule {
        Rule {
            pattern: pattern.into(),
            level,
        }
    }

    #[test]
    fn globs_match_whole_names() {
        assert!(glob("repo-a", "repo-a"));
        assert!(!glob("repo-a", "repo-ab"));
        assert!(glob("repo-*", "repo-a"));
        assert!(glob("repo-*", "repo-"));
        assert!(glob("*-b", "web-b"));
        assert!(glob("w*b-b", "web-b"));
        assert!(!glob("w*b-?", "web-b"), "? is no wildcard");
        assert!(glob("*", ""));
        assert!(!glob("repo-*", "web-repo-a"));
    }

    #[test]
    fn an_exact_name_wins_over_a_glob_and_a_longer_glob_over_a_shorter() {
        let rules = vec![
            rule("*", Priority::Low),
            rule("repo-*", Priority::High),
            rule("repo-a", Priority::Normal),
        ];
        assert_eq!(rule_for(&rules, "repo-a").unwrap().level, Priority::Normal);
        assert_eq!(rule_for(&rules, "repo-b").unwrap().level, Priority::High);
        assert_eq!(rule_for(&rules, "web-b").unwrap().level, Priority::Low);
        assert!(rule_for(&rules[1..], "web-b").is_none());
    }

    #[test]
    fn set_replaces_and_unset_removes() {
        let mut rules = Vec::new();
        assert!(set(&mut rules, "repo-a", Some(Priority::High)));
        assert!(!set(&mut rules, "repo-a", Some(Priority::High)));
        assert!(set(&mut rules, "repo-a", Some(Priority::Low)));
        assert_eq!(rules, [rule("repo-a", Priority::Low)]);
        assert!(!set(&mut rules, "web-b", None));
        assert!(set(&mut rules, "repo-a", None));
        assert_eq!(rules, Vec::<Rule>::new());
    }

    #[test]
    fn rules_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), Table::default());
        let table = Table {
            rules: vec![rule("repo-a", Priority::High), rule("web-*", Priority::Low)],
            promote_after_s: None,
        };
        save(dir.path(), &table).unwrap();
        assert_eq!(load(dir.path()), table);
        std::fs::write(dir.path().join(FILE), "not json").unwrap();
        assert_eq!(load(dir.path()), Table::default());
    }

    #[test]
    fn the_delay_is_saved_beside_the_rules_and_absent_until_set() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        let text = || std::fs::read_to_string(&file).unwrap();
        let option = Duration::from_secs(1800);
        let mut table = Table {
            rules: vec![rule("repo-a", Priority::High)],
            promote_after_s: None,
        };
        save(dir.path(), &table).unwrap();
        assert_eq!(
            text(),
            r#"{"rules":[{"pattern":"repo-a","level":"high"}]}"#,
            "no delay set: the file is what a daemon without the setting writes"
        );
        assert_eq!(load(dir.path()).promote_after(option), option);

        assert!(table.set_promote_after(PromoteAfter::Seconds(300)));
        assert!(!table.set_promote_after(PromoteAfter::Seconds(300)));
        save(dir.path(), &table).unwrap();
        assert_eq!(
            text(),
            r#"{"rules":[{"pattern":"repo-a","level":"high"}],"promote_after_s":300}"#
        );
        let back = load(dir.path());
        assert_eq!(back, table);
        assert_eq!(back.promote_after(option), Duration::from_secs(300));

        assert!(table.set_promote_after(PromoteAfter::Seconds(0)));
        save(dir.path(), &table).unwrap();
        assert_eq!(
            load(dir.path()).promote_after(option),
            Duration::ZERO,
            "0 is a delay of its own, not the option"
        );

        assert!(table.set_promote_after(PromoteAfter::Default));
        assert!(!table.set_promote_after(PromoteAfter::Default));
        save(dir.path(), &table).unwrap();
        assert!(!text().contains("promote_after_s"), "{}", text());
        assert_eq!(load(dir.path()).rules, table.rules);
    }

    #[test]
    fn an_old_file_and_a_file_with_fields_of_a_newer_daemon_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        let rules = vec![rule("web-*", Priority::Low)];
        std::fs::write(&file, r#"{"rules":[{"pattern":"web-*","level":"low"}]}"#).unwrap();
        assert_eq!(
            load(dir.path()),
            Table {
                rules: rules.clone(),
                promote_after_s: None
            },
            "a file of a daemon without the setting"
        );
        std::fs::write(
            &file,
            r#"{"rules":[{"pattern":"web-*","level":"low"}],"promote_after_s":300,"later":{"x":1}}"#,
        )
        .unwrap();
        assert_eq!(
            load(dir.path()),
            Table {
                rules,
                promote_after_s: Some(300)
            },
            "a field this daemon does not know is ignored"
        );
        std::fs::write(&file, r#"{"promote_after_s":60}"#).unwrap();
        assert_eq!(
            load(dir.path()),
            Table {
                rules: Vec::new(),
                promote_after_s: Some(60)
            }
        );
    }

    #[test]
    fn the_request_body_names_a_rule_a_delay_or_both() {
        let parse = |s: &str| serde_json::from_str::<Request>(s);
        let rule = parse(r#"{"pattern":"repo-a","level":"high"}"#).unwrap();
        assert_eq!(
            rule,
            Request {
                pattern: Some("repo-a".into()),
                level: Some(Priority::High),
                promote_after_s: None,
            }
        );
        assert_eq!(rule.refused(), None);
        let unset = parse(r#"{"pattern":"repo-a","level":null}"#).unwrap();
        assert_eq!((unset.level, unset.promote_after_s), (None, None));
        assert_eq!(parse(r#"{"pattern":"repo-a"}"#).unwrap(), unset);
        assert_eq!(unset.refused(), None);

        let delay = parse(r#"{"promote_after_s":300}"#).unwrap();
        assert_eq!(delay.promote_after_s, Some(PromoteAfter::Seconds(300)));
        assert_eq!((delay.pattern.as_deref(), delay.refused()), (None, None));
        let never = parse(r#"{"promote_after_s":0}"#).unwrap();
        assert_eq!(never.promote_after_s, Some(PromoteAfter::Seconds(0)));
        let default = parse(r#"{"promote_after_s":null}"#).unwrap();
        assert_eq!(default.promote_after_s, Some(PromoteAfter::Default));
        assert_eq!(default.refused(), None);
        let both = parse(r#"{"pattern":"web-*","level":"low","promote_after_s":60}"#).unwrap();
        assert_eq!(both.refused(), None);

        assert!(parse("{}").unwrap().refused().is_some());
        assert!(parse(r#"{"level":"high"}"#).unwrap().refused().is_some());
        assert!(
            parse(r#"{"level":"high","promote_after_s":60}"#)
                .unwrap()
                .refused()
                .is_some(),
            "a level without a pattern"
        );
        assert!(parse(r#"{"pattern":" "}"#).unwrap().refused().is_some());
        assert!(
            parse(r#"{"pattern":"","promote_after_s":60}"#)
                .unwrap()
                .refused()
                .is_some()
        );
        for bad in [
            r#"{"promote_after_s":-1}"#,
            r#"{"promote_after_s":"300"}"#,
            r#"{"promote_after_s":1.5}"#,
            r#"{"pattern":"repo-a","level":"asap"}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_delay_parses_from_seconds_or_default_and_writes_as_json() {
        assert_eq!("300".parse(), Ok(PromoteAfter::Seconds(300)));
        assert_eq!("0".parse(), Ok(PromoteAfter::Seconds(0)));
        assert_eq!("default".parse(), Ok(PromoteAfter::Default));
        for bad in ["", "-1", "5m", "Default", "1.5"] {
            assert!(bad.parse::<PromoteAfter>().is_err(), "{bad}");
        }
        let json = |v: PromoteAfter| serde_json::to_string(&v).unwrap();
        assert_eq!(json(PromoteAfter::Seconds(300)), "300");
        assert_eq!(json(PromoteAfter::Default), "null");
    }

    #[test]
    fn a_rule_wins_over_the_jobs_own_level() {
        let rules = vec![rule("repo-a", Priority::Low)];
        let never = Duration::ZERO;
        assert_eq!(
            level(
                &rules,
                "repo-a",
                Some(Priority::High),
                Duration::ZERO,
                never
            ),
            (Priority::Low, false)
        );
        assert_eq!(
            level(&rules, "web-b", Some(Priority::High), Duration::ZERO, never),
            (Priority::High, false)
        );
        assert_eq!(
            level(&rules, "web-b", None, Duration::ZERO, never),
            (Priority::Normal, false)
        );
    }

    #[test]
    fn a_job_that_waited_past_the_limit_is_ordered_high() {
        let rules = vec![rule("repo-a", Priority::Low)];
        let limit = Duration::from_secs(1800);
        let min = |m: u64| Duration::from_secs(m * 60);
        assert_eq!(
            level(&rules, "repo-a", None, min(29), limit),
            (Priority::Low, false)
        );
        assert_eq!(
            level(&rules, "repo-a", None, min(31), limit),
            (Priority::High, true)
        );
        assert_eq!(
            level(&rules, "web-b", None, min(31), limit),
            (Priority::High, true)
        );
        assert_eq!(
            level(&rules, "web-b", Some(Priority::High), min(31), limit),
            (Priority::High, false),
            "already high: not promoted"
        );
        assert_eq!(
            level(&rules, "web-b", Some(Priority::Urgent), min(31), limit),
            (Priority::Urgent, false),
            "urgent stays urgent"
        );
        assert_eq!(
            level(&rules, "repo-a", None, min(600), Duration::ZERO),
            (Priority::Low, false),
            "zero never promotes"
        );
    }

    #[test]
    fn a_delay_set_at_run_time_wins_over_the_option() {
        let option = Duration::from_secs(1800);
        let waited = Duration::from_secs(400);
        let mut table = Table::default();
        let at = |t: &Table| level(&t.rules, "web-b", None, waited, t.promote_after(option));
        assert_eq!(at(&table), (Priority::Normal, false), "400 s of 1800 s");
        table.set_promote_after(PromoteAfter::Seconds(300));
        assert_eq!(at(&table), (Priority::High, true), "400 s of 300 s");
        table.set_promote_after(PromoteAfter::Seconds(0));
        assert_eq!(at(&table), (Priority::Normal, false), "0 never promotes");
        table.set_promote_after(PromoteAfter::Default);
        assert_eq!(at(&table), (Priority::Normal, false), "the option again");
    }
}
