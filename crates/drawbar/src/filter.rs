//! What the library is filtered by, and the tree rows that set the filter.

use std::collections::BTreeSet;

use crate::browser::Kind;

/// Which of the two places a row lives in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    Computer,
    Keyboard,
}

/// What a row needs done, if anything.
///
/// ⚠️ The two are exclusive. A waiting write is what the instrument will hold, so a row
/// that differs and is queued counts as waiting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// A write to its slot is waiting to go.
    Waiting,
    /// Its slot holds something different, and no write is waiting to fix that.
    Differs,
}

impl State {
    /// The label of the tree row that filters by it.
    pub fn title(self) -> &'static str {
        match self {
            State::Waiting => "Waiting to send",
            State::Differs => "Differs",
        }
    }

    /// The word the library's bar puts after the count.
    pub fn word(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Differs => "differ",
        }
    }

    /// The full description, shown on hover.
    pub fn sentence(self) -> &'static str {
        match self {
            State::Waiting => "everything waiting to be sent to the instrument",
            State::Differs => "on this computer and on the instrument, with different contents",
        }
    }
}

/// What the library shows. Each field that is set narrows the list; an empty one does
/// not filter.
#[derive(Default)]
pub struct Filter {
    pub kind: Option<Kind>,
    pub tags: BTreeSet<u64>,
    pub place: Option<Place>,
    pub state: Option<State>,
}

/// One change to the filter, as a tree row requests it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Narrow {
    /// A kind row: on, or off if it is already selected.
    Kind(Kind),
    /// A tag row: in or out of the set.
    Tag(u64),
    /// A place row. The same click also opens that place's own tab.
    Place(Place),
    /// A state row.
    State(State),
}

impl Filter {
    /// Whether a library row passes the filter.
    ///
    /// ⚠️ Tags combine with AND: a row must have every selected tag, so a second tag
    /// narrows the list.
    pub fn admits(
        &self,
        kind: Kind,
        place: Place,
        tags: &BTreeSet<u64>,
        state: Option<State>,
    ) -> bool {
        self.kind.is_none_or(|want| want == kind)
            && self.place.is_none_or(|want| want == place)
            && self.state.is_none_or(|want| Some(want) == state)
            && self.tags.is_subset(tags)
    }

    /// Apply one change. ⚠️ Every row toggles: asking for it again turns it off, because
    /// the tree has no other gesture for that.
    pub fn narrow(&mut self, narrow: Narrow) {
        match narrow {
            Narrow::Kind(kind) => self.kind = (self.kind != Some(kind)).then_some(kind),
            Narrow::Tag(tag) => {
                if !self.tags.remove(&tag) {
                    self.tags.insert(tag);
                }
            }
            Narrow::Place(place) => self.place = (self.place != Some(place)).then_some(place),
            Narrow::State(state) => self.state = (self.state != Some(state)).then_some(state),
        }
    }

    /// Whether this change is already in effect.
    pub fn on(&self, narrow: Narrow) -> bool {
        match narrow {
            Narrow::Kind(kind) => self.kind == Some(kind),
            Narrow::Tag(tag) => self.tags.contains(&tag),
            Narrow::Place(place) => self.place == Some(place),
            Narrow::State(state) => self.state == Some(state),
        }
    }

    /// Drop a deleted tag from the filter.
    pub fn forget_tag(&mut self, tag: u64) {
        self.tags.remove(&tag);
    }

    /// ⚠️ Drop a kind that is no longer present anywhere. Its tree row is gone too, so if
    /// an instrument disconnects while the library is filtered to its folders, the table
    /// would otherwise stay empty with no way to clear the filter.
    pub fn keep_kinds(&mut self, present: &[Kind]) {
        self.kind = self.kind.filter(|kind| present.contains(kind));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worn(tags: &[u64]) -> BTreeSet<u64> {
        tags.iter().copied().collect()
    }

    /// An empty filter admits everything, and a kind and a place each narrow it.
    #[test]
    fn an_empty_filter_admits_every_row() {
        let mut filter = Filter::default();
        assert!(filter.admits(Kind::Program, Place::Computer, &worn(&[]), None));
        assert!(filter.admits(Kind::Piano, Place::Keyboard, &worn(&[]), None));

        filter.narrow(Narrow::Kind(Kind::Program));
        assert!(filter.admits(Kind::Program, Place::Keyboard, &worn(&[]), None));
        assert!(!filter.admits(Kind::Piano, Place::Keyboard, &worn(&[]), None));

        filter.narrow(Narrow::Place(Place::Computer));
        assert!(filter.admits(Kind::Program, Place::Computer, &worn(&[]), None));
        assert!(!filter.admits(Kind::Program, Place::Keyboard, &worn(&[]), None));
    }

    #[test]
    fn tags_narrow_together_rather_than_apart() {
        let mut filter = Filter::default();
        filter.narrow(Narrow::Tag(1));
        assert!(filter.admits(Kind::Program, Place::Computer, &worn(&[1]), None));
        assert!(filter.admits(Kind::Program, Place::Computer, &worn(&[1, 2]), None));
        assert!(!filter.admits(Kind::Program, Place::Computer, &worn(&[2]), None));

        filter.narrow(Narrow::Tag(2));
        assert!(filter.admits(Kind::Program, Place::Computer, &worn(&[1, 2]), None));
        assert!(!filter.admits(Kind::Program, Place::Computer, &worn(&[1]), None));

        // A kind and the tags combine: both must match.
        filter.narrow(Narrow::Kind(Kind::Program));
        assert!(filter.admits(Kind::Program, Place::Computer, &worn(&[1, 2]), None));
        assert!(!filter.admits(Kind::Live, Place::Computer, &worn(&[1, 2]), None));
    }

    #[test]
    fn a_row_asked_for_twice_stops_narrowing() {
        let mut filter = Filter::default();
        for narrow in [
            Narrow::Kind(Kind::Program),
            Narrow::Tag(1),
            Narrow::Place(Place::Computer),
            Narrow::State(State::Waiting),
        ] {
            filter.narrow(narrow);
            assert!(filter.on(narrow), "{narrow:?}");
            filter.narrow(narrow);
            assert!(!filter.on(narrow), "{narrow:?}");
        }
    }

    /// A row that needs nothing passes neither state filter.
    #[test]
    fn a_state_admits_only_the_rows_in_it() {
        let mut filter = Filter::default();
        let admits = |filter: &Filter, state| {
            filter.admits(Kind::Program, Place::Computer, &worn(&[]), state)
        };
        for state in [None, Some(State::Waiting), Some(State::Differs)] {
            assert!(admits(&filter, state), "{state:?}");
        }

        filter.narrow(Narrow::State(State::Waiting));
        assert!(admits(&filter, Some(State::Waiting)));
        assert!(!admits(&filter, Some(State::Differs)));
        assert!(!admits(&filter, None));

        // One state at a time: selecting the other replaces the first.
        filter.narrow(Narrow::State(State::Differs));
        assert!(admits(&filter, Some(State::Differs)));
        assert!(!admits(&filter, Some(State::Waiting)));

        // It combines with the other filters.
        filter.narrow(Narrow::Kind(Kind::Live));
        assert!(!admits(&filter, Some(State::Differs)));
    }

    #[test]
    fn a_tag_that_is_gone_stops_narrowing() {
        let mut filter = Filter::default();
        filter.narrow(Narrow::Tag(1));
        filter.narrow(Narrow::Tag(2));
        filter.forget_tag(1);
        assert!(!filter.on(Narrow::Tag(1)));
        assert!(filter.on(Narrow::Tag(2)));
    }
}
