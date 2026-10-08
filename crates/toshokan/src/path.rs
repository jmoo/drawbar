use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// A path relative to a root: `/`-separated names, none of them empty, `.` or `..`,
/// and none containing NUL. The empty path is the root itself.
///
/// Ordered by its text, so sorted paths are deterministic on every machine.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

impl RelPath {
    pub const ROOT: Self = Self(String::new());

    pub fn new(text: &str) -> Result<Self> {
        if !text.is_empty() {
            text.split('/')
                .try_for_each(|name| check_name(text, name))?;
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The path of `name` inside this directory. `name` must be one component.
    pub fn join(&self, name: &str) -> Result<Self> {
        check_name(name, name)?;
        Ok(match self.is_root() {
            true => Self(name.to_owned()),
            false => Self(format!("{}/{name}", self.0)),
        })
    }

    /// The directory holding this path; `None` for the root.
    pub fn parent(&self) -> Option<Self> {
        match self.0.rsplit_once('/') {
            Some((parent, _)) => Some(Self(parent.to_owned())),
            None if self.is_root() => None,
            None => Some(Self::ROOT),
        }
    }

    /// The last component; `None` for the root.
    pub fn name(&self) -> Option<&str> {
        match self.is_root() {
            true => None,
            false => self.0.rsplit('/').next(),
        }
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|name| !name.is_empty())
    }

    /// Whether `prefix` is this path or one of its ancestors, compared by component.
    pub fn starts_with(&self, prefix: &RelPath) -> bool {
        prefix.is_root()
            || self.0 == prefix.0
            || (self.0.starts_with(&prefix.0) && self.0.as_bytes()[prefix.0.len()] == b'/')
    }

    /// This path moved from under `from` to under `to`; `None` when it is not under
    /// `from`.
    pub fn rebase(&self, from: &RelPath, to: &RelPath) -> Option<Self> {
        if !self.starts_with(from) {
            return None;
        }
        let rest = self.0[from.0.len()..].trim_start_matches('/');
        Some(match (to.is_root(), rest.is_empty()) {
            (_, true) => to.clone(),
            (true, false) => Self(rest.to_owned()),
            (false, false) => Self(format!("{}/{rest}", to.0)),
        })
    }
}

fn check_name(path: &str, name: &str) -> Result<()> {
    let reason = match name {
        "" => "a component is empty",
        "." | ".." => "a component is `.` or `..`",
        _ if name.contains('/') => "a name contains `/`",
        _ if name.contains('\0') => "a component contains NUL",
        _ => return Ok(()),
    };
    Err(Error::InvalidPath {
        path: path.to_owned(),
        reason,
    })
}

/// A path compares, orders and hashes as its text.
impl std::borrow::Borrow<str> for RelPath {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// The root prints as `.`; every other path prints as its text.
impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_root() { "." } else { &self.0 })
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.0)
    }
}

impl Serialize for RelPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RelPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Self::new(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    #[test]
    fn a_path_refuses_empty_dot_and_nul_components() {
        for text in ["/a", "a/", "a//b", ".", "a/./b", "..", "a/../b", "a\0b"] {
            assert!(RelPath::new(text).is_err(), "{text:?} accepted");
        }
        assert!(RelPath::ROOT.join("a/b").is_err());
        assert!(RelPath::ROOT.join("").is_err());
    }

    #[test]
    fn a_path_knows_its_parent_and_name() {
        let file = path("Organ/B3/Gospel.npno");
        assert_eq!(file.parent(), Some(path("Organ/B3")));
        assert_eq!(file.name(), Some("Gospel.npno"));
        assert_eq!(path("top").parent(), Some(RelPath::ROOT));
        assert_eq!(RelPath::ROOT.parent(), None);
        assert_eq!(RelPath::ROOT.name(), None);
        assert_eq!(
            file.components().collect::<Vec<_>>(),
            ["Organ", "B3", "Gospel.npno"]
        );
        assert_eq!(RelPath::ROOT.components().count(), 0);
    }

    #[test]
    fn a_path_starts_with_whole_components_only() {
        let file = path("ab/c");
        assert!(file.starts_with(&path("ab")));
        assert!(file.starts_with(&file));
        assert!(file.starts_with(&RelPath::ROOT));
        assert!(!file.starts_with(&path("a")));
        assert!(!path("ab").starts_with(&file));
    }

    #[test]
    fn rebasing_moves_a_path_between_trees() {
        let to = path("x/y");
        assert_eq!(path("a/b/c").rebase(&path("a"), &to), Some(path("x/y/b/c")));
        assert_eq!(path("a").rebase(&path("a"), &to), Some(to.clone()));
        assert_eq!(
            path("a/b").rebase(&path("a"), &RelPath::ROOT),
            Some(path("b"))
        );
        assert_eq!(path("ab").rebase(&path("a"), &to), None);
    }

    #[test]
    fn a_path_deserializes_only_when_valid() {
        assert_eq!(serde_json::to_string(&RelPath::ROOT).unwrap(), "\"\"");
        assert_eq!(
            serde_json::from_str::<RelPath>("\"a/b\"").unwrap(),
            path("a/b")
        );
        assert!(serde_json::from_str::<RelPath>("\"a/../b\"").is_err());
    }
}
