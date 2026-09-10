//! What the library is narrowed to, and the rows of the tree that narrow it.

use std::collections::BTreeSet;

use crate::browser::Kind;

/// Which of the two places a row lives in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    Computer,
    Keyboard,
}

/// What a row wants doing about it, where anything does.
///
/// ⚠️ The two are exclusive: a write already waiting is what the instrument will hold, so
/// a row that both differs and is queued is waiting rather than differing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// A write to its slot is waiting to go.
    Waiting,
    /// It is in a slot that no longer holds it, and nothing is waiting to put that right.
    Differs,
}

impl State {
    /// The row of the tree that asks for it.
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

    /// The whole of it, which is what a hover says.
    pub fn sentence(self) -> &'static str {
        match self {
            State::Waiting => "everything owed back to the instrument",
            State::Differs => "here and on the instrument, and the two bodies differ",
        }
    }
}

/// What the library shows. Every field narrows, and an empty one asks for nothing.
#[derive(Default)]
pub struct Filter {
    pub kind: Option<Kind>,
    pub tags: BTreeSet<u64>,
    pub place: Option<Place>,
    pub state: Option<State>,
}

/// One turn of one of the filter's knobs, as a row of the tree asks for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Narrow {
    /// A kind row: on, or off again when it is already the one.
    Kind(Kind),
    /// A tag row: in or out of the set.
    Tag(u64),
    /// A place row: what the library is over. The row asking for it is opening that
    /// place's own tab in the same click, which it does either way.
    Place(Place),
    /// A state row: what wants doing about it, and nothing else.
    State(State),
}

impl Filter {
    /// Whether a row of the library survives the narrowing.
    ///
    /// ⚠️ Tags **AND**. A row must wear every tag asked for, which is what makes a
    /// second tag narrow the list rather than widen it.
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

    /// Turn one of the filter's knobs. ⚠️ Every row turns off when it is asked for
    /// twice — the tree has no second gesture for stopping.
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

    /// Whether the row that would ask for this is the one the library is already on.
    pub fn on(&self, narrow: Narrow) -> bool {
        match narrow {
            Narrow::Kind(kind) => self.kind == Some(kind),
            Narrow::Tag(tag) => self.tags.contains(&tag),
            Narrow::Place(place) => self.place == Some(place),
            Narrow::State(state) => self.state == Some(state),
        }
    }

    /// A tag that has gone takes its narrowing with it.
    pub fn forget_tag(&mut self, tag: u64) {
        self.tags.remove(&tag);
    }

    /// ⚠️ A kind that is nowhere any more takes its narrowing with it. The row that
    /// would turn it off has gone with it — an instrument let go while its folders are
    /// what the library is narrowed to would otherwise leave an empty table and no way
    /// back to a full one.
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

    /// Nothing asked for admits everything, and a kind and a place each cut it down.
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

    /// ⚠️ A second tag narrows. A row must wear every tag asked for, so asking for two
    /// leaves what wears both rather than what wears either.
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

        // A kind and the tags compose: both have to be satisfied.
        filter.narrow(Narrow::Kind(Kind::Program));
        assert!(filter.admits(Kind::Program, Place::Computer, &worn(&[1, 2]), None));
        assert!(!filter.admits(Kind::Live, Place::Computer, &worn(&[1, 2]), None));
    }

    /// Every row of the tree that narrows turns off when it is asked for twice — the
    /// tree has no second gesture for stopping.
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

    /// The state axis asks for one of the two things a row can want doing, and a row
    /// wanting nothing survives neither.
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

        // One state at a time: asking for the other lets the first go.
        filter.narrow(Narrow::State(State::Differs));
        assert!(admits(&filter, Some(State::Differs)));
        assert!(!admits(&filter, Some(State::Waiting)));

        // And it composes with the axes beside it rather than replacing them.
        filter.narrow(Narrow::Kind(Kind::Live));
        assert!(!admits(&filter, Some(State::Differs)));
    }

    /// A tag that has been removed cannot go on narrowing the library from nowhere.
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
