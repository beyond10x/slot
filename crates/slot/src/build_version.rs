//! The version a binary prints. `build.rs` calls [`name`] with what git says
//! about the checkout; the library compiles this file too, for its tests.

/// What git says about the checkout a binary is built from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkout {
    /// Short commit of `HEAD`; `None` without git or outside a checkout.
    pub head: Option<String>,
    /// Tags that point at `HEAD`.
    pub tags: Vec<String>,
    /// Tracked files differ from `HEAD`.
    pub dirty: bool,
    /// The tag a CI release run was started for (`GITHUB_REF_NAME` when
    /// `GITHUB_REF_TYPE` is `tag`).
    pub ci_tag: Option<String>,
}

/// The package version for a build of the release tag of that version with
/// no change to a tracked file; otherwise the version with `+g<commit>`, and
/// `.dirty` when tracked files changed. Without git it is the package
/// version: a source archive carries no commit to name.
pub fn name(package: &str, c: &Checkout) -> String {
    let tagged = c.tags.iter().any(|t| t == package) || c.ci_tag.as_deref() == Some(package);
    match &c.head {
        None => package.to_string(),
        Some(_) if tagged && !c.dirty => package.to_string(),
        Some(head) => format!("{package}+g{head}{}", if c.dirty { ".dirty" } else { "" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(head: &str) -> Checkout {
        Checkout {
            head: Some(head.into()),
            ..Checkout::default()
        }
    }

    #[test]
    fn a_build_of_a_branch_names_its_commit() {
        assert_eq!(name("0.34.0", &at("31e76f1")), "0.34.0+g31e76f1");
    }

    #[test]
    fn a_build_with_changed_files_says_so_even_at_the_tag() {
        let c = Checkout {
            tags: vec!["0.34.0".into()],
            dirty: true,
            ..at("31e76f1")
        };
        assert_eq!(name("0.34.0", &c), "0.34.0+g31e76f1.dirty");
    }

    #[test]
    fn a_build_of_the_release_tag_prints_the_bare_version() {
        let tagged = Checkout {
            tags: vec!["0.34.0".into()],
            ..at("31e76f1")
        };
        assert_eq!(name("0.34.0", &tagged), "0.34.0");
        let ci = Checkout {
            ci_tag: Some("0.34.0".into()),
            ..at("31e76f1")
        };
        assert_eq!(name("0.34.0", &ci), "0.34.0");
    }

    #[test]
    fn another_tag_on_the_commit_is_not_this_release() {
        let c = Checkout {
            tags: vec!["0.33.1".into()],
            ..at("31e76f1")
        };
        assert_eq!(name("0.34.0", &c), "0.34.0+g31e76f1");
    }

    #[test]
    fn without_git_the_package_version_stands() {
        assert_eq!(name("0.34.0", &Checkout::default()), "0.34.0");
    }

    #[test]
    fn this_binary_names_its_build() {
        let v = env!("SLOT_VERSION");
        assert!(v.starts_with(env!("CARGO_PKG_VERSION")), "{v}");
    }
}
