//! Named things with ids, and the stored file for a grouping of the local list.
//!
//! [`crate::folders`] and [`crate::tags`] share this naming and storage. They differ in
//! membership (an asset is in one folder but can have any number of tags), and each
//! module keeps its own.

use crate::store::{escape, unescape};

/// One named thing on this computer.
pub struct Named {
    pub id: u64,
    pub name: String,
}

/// A list of them, holding one entry per id and one name per entry.
#[derive(Default)]
pub struct List(Vec<Named>);

impl List {
    pub fn all(&self) -> &[Named] {
        &self.0
    }

    pub fn name_of(&self, id: u64) -> Option<&str> {
        self.0
            .iter()
            .find(|held| held.id == id)
            .map(|held| held.name.as_str())
    }

    /// Whether the list holds this id. A membership naming an id it does not hold means
    /// nothing.
    pub fn holds(&self, id: u64) -> bool {
        self.name_of(id).is_some()
    }

    /// A new entry named `wanted`, or `wanted 2`, `wanted 3`, … if that name is taken.
    ///
    /// ⚠️ `None` when the list holds [`u64::MAX`], which a stored grouping can name. Ids
    /// only rise, so a removed id is never reused while a membership still refers to it,
    /// and there is no id above the last.
    pub fn make(&mut self, wanted: &str) -> Option<u64> {
        let id = self
            .0
            .iter()
            .map(|held| held.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)?;
        let mut name = wanted.to_string();
        for nth in 2.. {
            if !self.0.iter().any(|held| held.name == name) {
                break;
            }
            name = format!("{wanted} {nth}");
        }
        self.0.push(Named { id, name });
        Some(id)
    }

    pub fn rename(&mut self, id: u64, name: String) {
        if let Some(held) = self.0.iter_mut().find(|held| held.id == id) {
            held.name = name;
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.0.retain(|held| held.id != id);
    }

    /// Restore one stored row.
    ///
    /// ⚠️ A second row for an id already in the list is refused. Memberships naming that
    /// id mean the first row, and there is no way to choose between two names for one
    /// thing.
    pub fn restore(&mut self, id: u64, name: String) {
        if !self.holds(id) {
            self.0.push(Named { id, name });
        }
    }
}

/// One line of a stored grouping.
pub enum Line {
    /// A named thing.
    Named { id: u64, name: String },
    /// The folder one asset is in, or a tag it has.
    Member { asset: u64, group: u64 },
}

/// The marker that starts a membership line, in either grouping.
const MEMBER: &str = "m";

/// The version line and one row per named thing. The caller appends its memberships
/// with [`member`].
pub fn written(version: &str, kind: &str, list: &List) -> String {
    let mut out = format!("{version}\n");
    for held in list.all() {
        out.push_str(&format!("{kind}\t{}\t{}\n", held.id, escape(&held.name)));
    }
    out
}

/// One membership line.
pub fn member(asset: u64, group: u64) -> String {
    format!("{MEMBER}\t{asset}\t{group}\n")
}

/// Read back what [`written`] and [`member`] wrote.
///
/// An unknown version line reads as no grouping: a partial one could show names nobody
/// made.
pub fn read(text: &str, version: &str, kind: &str) -> Vec<Line> {
    let mut lines = text.lines();
    if lines.next() != Some(version) {
        return Vec::new();
    }
    lines.filter_map(|line| parse(line, kind)).collect()
}

/// One line, or `None` if this build did not write it: an unknown line marker, a missing
/// or extra column, or an id that is not a number.
fn parse(line: &str, kind: &str) -> Option<Line> {
    let mut parts = line.split('\t');
    let (head, first, second) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    if head == kind {
        return Some(Line::Named {
            id: first.parse().ok()?,
            name: unescape(second),
        });
    }
    if head == MEMBER {
        return Some(Line::Member {
            asset: first.parse().ok()?,
            group: second.parse().ok()?,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_holding_the_last_id_makes_nothing_more() {
        let mut list = List::default();
        list.restore(u64::MAX, "Sunday".into());
        assert_eq!(list.make("Loud"), None);
        assert_eq!(list.all().len(), 1, "nothing was added");
        assert_eq!(list.name_of(u64::MAX), Some("Sunday"));
    }
}
