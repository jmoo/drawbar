//! How the list on this computer is labelled.
//!
//! Beside [`crate::folders`], and stored the same way: its own key, its own version
//! line, membership by the workspace id the list is stored under. A folder is exclusive
//! and a tag is not, which is the whole difference between them.

use std::collections::{BTreeMap, BTreeSet};

use crate::named::{self, Line, List, Named};
use crate::workspace::Workspace;

/// Where the tags and their membership are kept between sessions.
///
/// ⚠️ Membership is by workspace id, which is the same id the local list is stored
/// under — the two files are read back into one list, so they have to agree about what
/// an id means. Only a **kept** asset has one that survives a session, so a view is
/// kept before it can be tagged.
pub(crate) const KEY: &str = "drawbar.tags";

const VERSION: &str = "drawbar tags 1";

/// What a tag line is headed with.
const TAG: &str = "t";

/// A tag is a name and nothing else.
pub type Tag = Named;

/// What is labelled with what.
#[derive(Default)]
pub struct Tags {
    list: List,
    /// The tags an asset wears, by its workspace id. Absent is untagged.
    of: BTreeMap<u64, BTreeSet<u64>>,
}

/// What an untagged asset wears, so a caller need not tell absent from empty.
static NOTHING: BTreeSet<u64> = BTreeSet::new();

impl Tags {
    pub fn all(&self) -> &[Tag] {
        self.list.all()
    }

    pub fn name_of(&self, id: u64) -> Option<&str> {
        self.list.name_of(id)
    }

    /// A new tag, under a name nothing else in the list is using, or nothing where the
    /// list has no id left ([`List::make`]).
    pub(crate) fn make(&mut self, wanted: &str) -> Option<u64> {
        self.list.make(wanted)
    }

    pub(crate) fn rename(&mut self, id: u64, name: String) {
        self.list.rename(id, name);
    }

    /// Drop a tag. What wore it keeps everything else it wore — a tag holds nothing.
    pub(crate) fn remove(&mut self, id: u64) {
        self.list.remove(id);
        for worn in self.of.values_mut() {
            worn.remove(&id);
        }
        self.of.retain(|_, worn| !worn.is_empty());
    }

    /// Put a tag on an asset, or take it off. A tag nobody made goes on nothing.
    pub(crate) fn set(&mut self, asset: u64, tag: u64, on: bool) {
        if on && self.list.holds(tag) {
            self.of.entry(asset).or_default().insert(tag);
            return;
        }
        if let Some(worn) = self.of.get_mut(&asset) {
            worn.remove(&tag);
            if worn.is_empty() {
                self.of.remove(&asset);
            }
        }
    }

    /// The tags an asset wears.
    pub fn worn(&self, asset: u64) -> &BTreeSet<u64> {
        self.of.get(&asset).unwrap_or(&NOTHING)
    }

    /// Whether every one of these assets wears this tag. An empty selection wears
    /// nothing, so the answer is no rather than vacuously yes.
    pub fn on_all(&self, assets: &[u64], tag: u64) -> bool {
        !assets.is_empty() && assets.iter().all(|asset| self.worn(*asset).contains(&tag))
    }

    /// How many assets wear this tag.
    pub fn count(&self, tag: u64) -> usize {
        self.of.values().filter(|worn| worn.contains(&tag)).count()
    }

    pub(crate) fn forget(&mut self, asset: u64) {
        self.of.remove(&asset);
    }

    /// Drop the memberships of assets the list does not hold.
    ///
    /// The store keeps the tags and the assets in two files that are read back
    /// separately, and only the asset file decides what survived — anything too big to
    /// keep, or dropped for want of room, leaves its membership behind. Left alone they
    /// accumulate for as long as the app is installed.
    pub(crate) fn forget_missing(&mut self, workspace: &Workspace) {
        self.of.retain(|asset, _| workspace.get(*asset).is_some());
    }

    /// The tags and their membership as one string, for the store.
    ///
    /// `t` lines are the tags and `m` lines are what wears them, so a tag nothing wears
    /// survives a session like any other.
    pub(crate) fn written(&self) -> String {
        let mut out = named::written(VERSION, TAG, &self.list);
        for (asset, worn) in &self.of {
            for tag in worn {
                out.push_str(&named::member(*asset, *tag));
            }
        }
        out
    }

    /// Read back what [`Tags::written`] wrote. Anything unaccounted for is no tags at
    /// all — half a labelling is worse than none, because a tag nobody made is one
    /// nobody can explain.
    pub(crate) fn read(text: &str) -> Tags {
        let mut tags = Tags::default();
        for line in named::read(text, VERSION, TAG) {
            match line {
                Line::Named { id, name } => tags.list.restore(id, name),
                Line::Member { asset, group } => {
                    tags.of.entry(asset).or_default().insert(group);
                }
            }
        }
        // A membership naming a tag that is not in the file would be an asset wearing
        // something nothing can show and nothing can take off.
        let Tags { list, of } = &mut tags;
        for worn in of.values_mut() {
            worn.retain(|tag| list.holds(*tag));
        }
        of.retain(|_, worn| !worn.is_empty());
        tags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new tag is one nothing else is called, so two of them are two rows rather than
    /// one row twice — and the name asked for is the name it starts from.
    #[test]
    fn a_new_tag_gets_a_name_no_other_tag_is_using() {
        let mut tags = Tags::default();
        let names: Vec<String> = ["Sunday", "Sunday", "Sunday"]
            .iter()
            .map(|wanted| {
                let id = tags.make(wanted).unwrap();
                tags.name_of(id).expect("it was made").to_string()
            })
            .collect();
        assert_eq!(names, ["Sunday", "Sunday 2", "Sunday 3"]);
    }

    /// A tag is not a folder: an asset wears as many as it is given, and losing one
    /// leaves the rest where they were.
    #[test]
    fn an_asset_wears_every_tag_it_is_given() {
        let mut tags = Tags::default();
        let (sunday, loud) = (tags.make("Sunday").unwrap(), tags.make("Loud").unwrap());
        tags.set(7, sunday, true);
        tags.set(7, loud, true);
        assert_eq!(tags.worn(7).len(), 2);

        tags.set(7, sunday, false);
        assert_eq!(tags.worn(7), &BTreeSet::from([loud]));
        // And an id nobody made is not something anything can wear.
        tags.set(7, 99, true);
        assert_eq!(tags.worn(7), &BTreeSet::from([loud]));
        assert!(tags.worn(8).is_empty(), "an untagged asset wears nothing");
    }

    /// Removing a tag takes it off everything, and takes nothing else with it.
    #[test]
    fn removing_a_tag_leaves_every_other_tag_where_it_was() {
        let mut tags = Tags::default();
        let (gone, kept) = (tags.make("Sunday").unwrap(), tags.make("Loud").unwrap());
        tags.set(7, gone, true);
        tags.set(7, kept, true);
        tags.set(8, gone, true);
        tags.remove(gone);

        assert_eq!(tags.worn(7), &BTreeSet::from([kept]));
        assert!(tags.worn(8).is_empty());
        assert_eq!(tags.count(kept), 1);
    }

    /// Whether a tag is on the whole selection is what the menu shows a check against;
    /// nothing picked is not everything picked.
    #[test]
    fn a_tag_is_on_all_only_when_every_picked_asset_wears_it() {
        let mut tags = Tags::default();
        let sunday = tags.make("Sunday").unwrap();
        tags.set(7, sunday, true);
        assert!(tags.on_all(&[7], sunday));
        assert!(!tags.on_all(&[7, 8], sunday), "8 does not wear it");
        assert!(!tags.on_all(&[], sunday), "nothing picked wears nothing");

        tags.set(8, sunday, true);
        assert!(tags.on_all(&[7, 8], sunday));
    }

    /// The labelling comes back as it was left, unworn tags included — and a membership
    /// naming a tag the file does not hold is dropped rather than left on an asset that
    /// can never show it or take it off.
    #[test]
    fn the_tags_and_what_wears_them_survive_a_session() {
        let mut tags = Tags::default();
        let (sunday, unworn) = (tags.make("Sunday").unwrap(), tags.make("Loud").unwrap());
        tags.rename(sunday, "Sunday\tmorning".into());
        tags.set(7, sunday, true);
        tags.set(8, sunday, true);

        let after = Tags::read(&tags.written());
        assert_eq!(after.all().len(), 2, "a tag nothing wears is still a tag");
        assert_eq!(after.name_of(sunday), Some("Sunday\tmorning"));
        assert_eq!(after.name_of(unworn), Some("Loud"));
        assert_eq!(after.count(sunday), 2);

        // Nothing readable is no tags at all, never half a labelling.
        assert!(Tags::read("").all().is_empty());
        assert!(Tags::read("drawbar tags 99\nt\t1\tSunday\n")
            .all()
            .is_empty());
        let orphaned = Tags::read(&format!("{VERSION}\nm\t7\t3\n"));
        assert!(orphaned.worn(7).is_empty());
    }

    /// A line this build did not write is dropped rather than guessed at, and the rest of
    /// the file is still read.
    #[test]
    fn a_line_that_is_not_a_line_is_dropped_and_the_rest_is_read() {
        let read = |lines: &str| Tags::read(&format!("{VERSION}\n{lines}"));

        let kept = read("t\tx\tNot a number\nt\t1\tSunday\n");
        assert_eq!(kept.all().len(), 1, "an id that is not a number");
        assert_eq!(kept.name_of(1), Some("Sunday"));

        let short = read("t\t1\tSunday\nm\t7\n");
        assert!(short.worn(7).is_empty(), "a membership missing its tag");

        let wide = read("t\t1\tSunday\nm\t7\t1\textra\n");
        assert!(wide.worn(7).is_empty(), "a line with a column too many");

        let unknown = read("t\t1\tSunday\nx\t7\t1\n");
        assert_eq!(unknown.all().len(), 1, "a head this build does not write");
    }

    /// ⚠️ Two `t` lines claiming one id is a file with two names for one tag, and
    /// everything wearing that id wears whichever of them is kept. The first is, so the
    /// second is refused rather than quietly renaming a tag on the way in.
    #[test]
    fn a_second_tag_line_for_an_id_already_read_is_refused() {
        let tags = Tags::read(&format!("{VERSION}\nt\t1\tSunday\nt\t1\tMonday\nm\t7\t1\n"));

        assert_eq!(tags.all().len(), 1);
        assert_eq!(tags.name_of(1), Some("Sunday"));
        assert_eq!(tags.worn(7), &BTreeSet::from([1]));
    }

    /// A name holding a newline would otherwise be two lines, and the second of them a
    /// line this build refuses.
    #[test]
    fn a_tag_named_across_two_lines_comes_back_as_one_name() {
        let mut tags = Tags::default();
        let id = tags.make("Sunday").unwrap();
        tags.rename(id, "Sunday\nmorning".into());

        let after = Tags::read(&tags.written());
        assert_eq!(after.name_of(id), Some("Sunday\nmorning"));
        assert_eq!(after.all().len(), 1);
    }
}
