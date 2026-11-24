//! `.slot.toml`: per-repository settings, read by the client from the
//! repository root.
//!
//! ```toml
//! local = ["cargo test -p integration", "go test ./e2e/..."]
//! ```
//!
//! `project = "web"` names the project the repository belongs to, for the
//! usage view; without it the project is the namespace of the git remote.
//!
//! `local` lists command prefixes that run on this machine's daemon even
//! when `SLOT_HOST` sends every other build to a build server: tests that
//! need docker, a database or a port on localhost. A prefix matches when its
//! words are the first words of the command.
//!
//! `warm` is what `slot warm` builds after a merge to `main`: a command,
//! split at whitespace (`warm = "task warm"`), or `false` for no warm-up.
//! Without it the warm-up is [`DEFAULT_WARM`].

use std::path::Path;

use serde::Deserialize;

/// File name, at the repository root.
pub const FILE: &str = ".slot.toml";

/// The warm-up of a repository whose `.slot.toml` names none: builds the
/// tests of the tree without running them.
pub const DEFAULT_WARM: &[&str] = &["cargo", "test", "--no-run"];

/// Parsed `.slot.toml`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    /// Command prefixes that never go to a remote host.
    #[serde(default)]
    pub local: Vec<String>,
    /// Project the repository belongs to, over the namespace of its remote
    /// (`crate::project`).
    #[serde(default)]
    pub project: Option<String>,
    /// The warm-up after a merge to `main` (`slot warm`).
    #[serde(default)]
    pub warm: Option<Warm>,
}

/// The `warm` entry: on or off, or the command.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Warm {
    /// `true` is [`DEFAULT_WARM`]; `false` switches the warm-up off.
    On(bool),
    /// The command, split at whitespace.
    Command(String),
}

impl RepoConfig {
    /// Reads `<root>/.slot.toml`. A missing file is the default; a file
    /// that does not parse is an error naming it.
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join(FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Parses the text of a `.slot.toml`.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.message().to_string())
    }

    /// The `local` entry that `argv` starts with, if any.
    pub fn local_match(&self, argv: &[String]) -> Option<&str> {
        self.local
            .iter()
            .find(|p| {
                let words: Vec<&str> = p.split_whitespace().collect();
                !words.is_empty()
                    && words.len() <= argv.len()
                    && words.iter().zip(argv).all(|(w, a)| *w == a.as_str())
            })
            .map(String::as_str)
    }

    /// The warm-up command, or `None` when `warm` switches it off (`false`
    /// or a command without a word).
    pub fn warm_argv(&self) -> Option<Vec<String>> {
        match &self.warm {
            None | Some(Warm::On(true)) => {
                Some(DEFAULT_WARM.iter().map(|w| (*w).to_string()).collect())
            }
            Some(Warm::On(false)) => None,
            Some(Warm::Command(c)) => {
                let argv: Vec<String> = c.split_whitespace().map(String::from).collect();
                (!argv.is_empty()).then_some(argv)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn prefix_matches_whole_words_only() {
        let c = RepoConfig::parse(r#"local = ["cargo test -p integration", "go test ./e2e/..."]"#)
            .unwrap();
        assert_eq!(
            c.local_match(&argv("cargo test -p integration -- --nocapture")),
            Some("cargo test -p integration")
        );
        assert_eq!(c.local_match(&argv("cargo test -p integration2")), None);
        assert_eq!(c.local_match(&argv("cargo test -p")), None);
        assert_eq!(
            c.local_match(&argv("go test ./e2e/...")),
            Some("go test ./e2e/...")
        );
        assert_eq!(c.local_match(&argv("cargo build")), None);
    }

    #[test]
    fn empty_entry_matches_nothing() {
        let c = RepoConfig::parse(r#"local = ["", "  "]"#).unwrap();
        assert_eq!(c.local_match(&argv("cargo test")), None);
    }

    #[test]
    fn the_warm_up_is_cargos_test_build_a_named_command_or_off() {
        let default = argv("cargo test --no-run");
        assert_eq!(RepoConfig::default().warm_argv(), Some(default.clone()));
        let on = RepoConfig::parse("warm = true").unwrap();
        assert_eq!(on.warm_argv(), Some(default));
        let named = RepoConfig::parse(r#"warm = "task  warm""#).unwrap();
        assert_eq!(named.warm_argv(), Some(argv("task warm")));
        for off in ["warm = false", r#"warm = "  ""#] {
            assert_eq!(RepoConfig::parse(off).unwrap().warm_argv(), None, "{off}");
        }
        assert!(RepoConfig::parse("warm = 3").is_err());
    }

    #[test]
    fn missing_file_is_default_and_bad_file_names_itself() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(RepoConfig::load(d.path()).unwrap(), RepoConfig::default());
        std::fs::write(d.path().join(FILE), "locl = []").unwrap();
        let e = RepoConfig::load(d.path()).unwrap_err();
        assert!(e.contains(".slot.toml"), "{e}");
        assert!(e.contains("locl"), "{e}");
    }
}
