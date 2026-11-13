//! Priority rules: the queue level of every job of a repository, set at run
//! time with `slot priority set` and kept across restarts in [`FILE`].
//!
//! A job's level is the rule for its repository, else the level it asked
//! for itself (`--priority`, `SLOT_PRIORITY`), else `normal`. A job that
//! has waited longer than `--promote-after` is ordered as `high`, so no
//! level waits forever.

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

/// What is saved in [`FILE`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    rules: Vec<Rule>,
}

/// Reads the saved rules; none when the file is missing or unreadable.
pub fn load(dir: &Path) -> Vec<Rule> {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<Saved>(&s).ok())
        .map(|s| s.rules)
        .unwrap_or_default()
}

/// Writes the rules.
pub fn save(dir: &Path, rules: &[Rule]) -> std::io::Result<()> {
    let tmp = dir.join(format!("{FILE}.tmp"));
    let saved = Saved {
        rules: rules.to_vec(),
    };
    std::fs::write(&tmp, serde_json::to_vec(&saved)?)?;
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

/// Body of `PUT /api/priority`: the rule for `pattern` gets `level`; a
/// missing or null `level` removes it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Request {
    /// Repository name or glob.
    pub pattern: String,
    /// The level; `None` removes the rule.
    #[serde(default)]
    pub level: Option<Priority>,
}

/// Answer of `GET` and `PUT /api/priority`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The rules, in the order they were set.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Seconds after which a queued job is ordered as `high`; 0 = never.
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
        assert_eq!(load(dir.path()), Vec::<Rule>::new());
        let rules = vec![rule("repo-a", Priority::High), rule("web-*", Priority::Low)];
        save(dir.path(), &rules).unwrap();
        assert_eq!(load(dir.path()), rules);
        std::fs::write(dir.path().join(FILE), "not json").unwrap();
        assert_eq!(load(dir.path()), Vec::<Rule>::new());
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
}
