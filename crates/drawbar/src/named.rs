//! Named things with ids, which [`crate::tags`] keeps.

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

    #[test]
    fn a_new_one_gets_a_name_nothing_else_is_using() {
        let mut list = List::default();
        let ids: Vec<u64> = (0..3).map(|_| list.make("Sunday").unwrap()).collect();
        assert_eq!(ids, [1, 2, 3]);
        let names: Vec<&str> = ids.iter().map(|id| list.name_of(*id).unwrap()).collect();
        assert_eq!(names, ["Sunday", "Sunday 2", "Sunday 3"]);
    }

    /// Two rows with one id give one thing two names. The first is kept, so loading
    /// never silently renames it.
    #[test]
    fn a_second_row_for_an_id_already_read_is_refused() {
        let mut list = List::default();
        list.restore(1, "Sunday".into());
        list.restore(1, "Monday".into());
        assert_eq!(list.all().len(), 1);
        assert_eq!(list.name_of(1), Some("Sunday"));
    }
}
