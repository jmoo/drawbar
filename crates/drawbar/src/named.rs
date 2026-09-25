//! Things with an id and a name, and the file a grouping of the local list is kept in.
//!
//! [`crate::folders`] and [`crate::tags`] are the same module above this one: the same
//! naming, the same store. What differs is the membership — an asset is in one folder and
//! wears as many tags as it is given — and that stays with each of them.

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

    /// Whether the list holds this id, which is what makes a membership naming it mean
    /// anything.
    pub fn holds(&self, id: u64) -> bool {
        self.name_of(id).is_some()
    }

    /// A new one, called `wanted` where nothing else in the list is and `wanted 2`,
    /// `wanted 3` … where something is.
    ///
    /// ⚠️ Nothing where the list holds [`u64::MAX`], which a stored grouping can name.
    /// Ids only rise, so that a removed one never comes back under a membership still
    /// meaning the row that had it, and there is no id above the last.
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

    /// Take one row of the store back.
    ///
    /// ⚠️ A second row for an id the list already holds is refused. Every membership
    /// naming that id means the row that claimed it, and there is nothing to choose
    /// between two names for one thing.
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
    /// What one asset is in, or wears.
    Member { asset: u64, group: u64 },
}

/// What a membership line is headed with, whichever grouping wrote it.
const MEMBER: &str = "m";

/// The version line and one row per named thing — the head of both stores, to which the
/// caller adds its own memberships with [`member`].
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
/// A version line this build does not know is no grouping at all — half a grouping is
/// worse than none, because a name nobody made is one nobody can explain.
pub fn read(text: &str, version: &str, kind: &str) -> Vec<Line> {
    let mut lines = text.lines();
    if lines.next() != Some(version) {
        return Vec::new();
    }
    lines.filter_map(|line| parse(line, kind)).collect()
}

/// One line, or nothing where it is not one this build wrote: an unknown head, a column
/// missing, a column too many, or an id that is not a number.
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

    /// ⚠️ A stored row can name any id, and the one above the last does not exist. Ids
    /// only rise, so a list holding it has none left to make a new thing under rather
    /// than an id to wrap onto.
    #[test]
    fn a_list_holding_the_last_id_makes_nothing_more() {
        let mut list = List::default();
        list.restore(u64::MAX, "Sunday".into());
        assert_eq!(list.make("Loud"), None);
        assert_eq!(list.all().len(), 1, "and nothing was added");
        assert_eq!(list.name_of(u64::MAX), Some("Sunday"));
    }

    /// A new one is called what nothing else is, so two of them are two rows rather
    /// than one row twice, and the name asked for is the name it starts from.
    #[test]
    fn a_new_one_gets_a_name_nothing_else_is_using() {
        let mut list = List::default();
        let ids: Vec<u64> = (0..3).map(|_| list.make("Sunday").unwrap()).collect();
        assert_eq!(ids, [1, 2, 3]);
        let names: Vec<&str> = ids.iter().map(|id| list.name_of(*id).unwrap()).collect();
        assert_eq!(names, ["Sunday", "Sunday 2", "Sunday 3"]);
    }

    /// ⚠️ Two rows claiming one id is a file with two names for one thing, and every
    /// membership naming that id means whichever of them is kept. The first is, so the
    /// second is refused rather than quietly renaming it on the way in.
    #[test]
    fn a_second_row_for_an_id_already_read_is_refused() {
        let mut list = List::default();
        list.restore(1, "Sunday".into());
        list.restore(1, "Monday".into());
        assert_eq!(list.all().len(), 1);
        assert_eq!(list.name_of(1), Some("Sunday"));
    }

    /// A line this build did not write is dropped rather than guessed at, and the rest of
    /// the file is still read.
    #[test]
    fn a_line_that_is_not_a_line_is_dropped_and_the_rest_is_read() {
        for (line, why) in [
            ("t\tx\tNot a number", "an id that is not a number"),
            ("m\t7", "a membership missing its group"),
            ("m\t7\t1\textra", "a line with a column too many"),
            ("x\t7\t1", "a head this build does not write"),
        ] {
            let read = read(&format!("v\n{line}\nt\t1\tSunday\n"), "v", "t");
            assert!(
                matches!(read.as_slice(), [Line::Named { id: 1, name }] if name == "Sunday"),
                "{why}"
            );
        }
    }

    /// A name holding a newline would otherwise be two lines, and the second of them a
    /// line this build refuses.
    #[test]
    fn a_name_across_two_lines_comes_back_as_one_name() {
        let mut list = List::default();
        let id = list.make("Sunday").unwrap();
        list.rename(id, "Sunday\nmorning".into());

        let read = read(&written("v", "t", &list), "v", "t");
        assert!(
            matches!(read.as_slice(), [Line::Named { id: 1, name }] if name == "Sunday\nmorning"),
            "{}",
            read.len()
        );
    }
}
