//! Tags that label the list on this computer.
//!
//! Stored the same way as [`crate::folders`], under its own `KEY` and version line. A
//! folder is exclusive and a tag is not; that is the only difference.

use std::collections::{BTreeMap, BTreeSet};

use crate::named::{self, Line, List, Named};
use crate::workspace::Workspace;

/// Where the tags and their membership are kept between sessions.
///
/// ⚠️ Membership is by workspace id, the same id the local list is stored under. The
/// two files are read back separately into one list, so they must agree about what an id
/// means. Only a kept asset has an id that survives a session, so a view is kept before
/// it can be tagged.
pub(crate) const KEY: &str = "drawbar.tags";

const VERSION: &str = "drawbar tags 1";

/// The marker that starts a tag line.
const TAG: &str = "t";

/// A tag is only a name.
pub type Tag = Named;

/// Which assets have which tags.
#[derive(Default)]
pub struct Tags {
    list: List,
    /// The tags on each asset, by workspace id. An asset with no entry is untagged.
    of: BTreeMap<u64, BTreeSet<u64>>,
}

/// The tags of an untagged asset, so a caller need not tell absent from empty.
static NOTHING: BTreeSet<u64> = BTreeSet::new();

impl Tags {
    pub fn all(&self) -> &[Tag] {
        self.list.all()
    }

    pub fn name_of(&self, id: u64) -> Option<&str> {
        self.list.name_of(id)
    }

    /// A new tag with a name no other tag uses, or `None` when the list has no id left
    /// ([`List::make`]).
    pub(crate) fn make(&mut self, wanted: &str) -> Option<u64> {
        self.list.make(wanted)
    }

    pub(crate) fn rename(&mut self, id: u64, name: String) {
        self.list.rename(id, name);
    }

    /// Remove a tag from the list and from every asset. Their other tags stay.
    pub(crate) fn remove(&mut self, id: u64) {
        self.list.remove(id);
        for worn in self.of.values_mut() {
            worn.remove(&id);
        }
        self.of.retain(|_, worn| !worn.is_empty());
    }

    /// Add a tag to an asset, or remove it. A tag not in the list cannot be added.
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

    /// The tags on an asset.
    pub fn worn(&self, asset: u64) -> &BTreeSet<u64> {
        self.of.get(&asset).unwrap_or(&NOTHING)
    }

    /// Whether every one of these assets has this tag. An empty selection has none, so
    /// the answer is no, not vacuously yes.
    pub fn on_all(&self, assets: &[u64], tag: u64) -> bool {
        !assets.is_empty() && assets.iter().all(|asset| self.worn(*asset).contains(&tag))
    }

    /// How many assets have this tag.
    pub fn count(&self, tag: u64) -> usize {
        self.of.values().filter(|worn| worn.contains(&tag)).count()
    }

    pub(crate) fn forget(&mut self, asset: u64) {
        self.of.remove(&asset);
    }

    /// Drop the memberships of assets the list does not hold.
    ///
    /// The store keeps tags and assets in two files read back separately, and only the
    /// asset file decides what survived: an asset too big to keep, or dropped for lack of
    /// room, leaves its membership behind. Left alone, these would accumulate for as long
    /// as the app is installed.
    pub(crate) fn forget_missing(&mut self, workspace: &Workspace) {
        self.of.retain(|asset, _| workspace.get(*asset).is_some());
    }

    /// The tags and their membership as one string, for the store.
    ///
    /// `t` lines are tags and `m` lines are memberships, so an unused tag survives a
    /// session like any other.
    pub(crate) fn written(&self) -> String {
        let mut out = named::written(VERSION, TAG, &self.list);
        for (asset, worn) in &self.of {
            for tag in worn {
                out.push_str(&named::member(*asset, *tag));
            }
        }
        out
    }

    /// Read back what [`Tags::written`] wrote. An unknown version reads as no tags; a
    /// malformed line is dropped and the rest is read.
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
        // A membership naming a tag missing from the file would leave a tag on the asset
        // that nothing can show or remove.
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

    /// Unlike a folder, an asset can have any number of tags.
    #[test]
    fn an_asset_wears_every_tag_it_is_given() {
        let mut tags = Tags::default();
        let (sunday, loud) = (tags.make("Sunday").unwrap(), tags.make("Loud").unwrap());
        tags.set(7, sunday, true);
        tags.set(7, loud, true);
        assert_eq!(tags.worn(7).len(), 2);

        tags.set(7, sunday, false);
        assert_eq!(tags.worn(7), &BTreeSet::from([loud]));
        // An id not in the list cannot be added.
        tags.set(7, 99, true);
        assert_eq!(tags.worn(7), &BTreeSet::from([loud]));
        assert!(tags.worn(8).is_empty(), "an untagged asset has no tags");
    }

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

    /// The menu checks a tag that is on the whole selection, and an empty selection is
    /// not the whole of anything.
    #[test]
    fn a_tag_is_on_all_only_when_every_picked_asset_wears_it() {
        let mut tags = Tags::default();
        let sunday = tags.make("Sunday").unwrap();
        tags.set(7, sunday, true);
        assert!(tags.on_all(&[7], sunday));
        assert!(!tags.on_all(&[7, 8], sunday), "8 does not have it");
        assert!(!tags.on_all(&[], sunday), "nothing is picked");

        tags.set(8, sunday, true);
        assert!(tags.on_all(&[7, 8], sunday));
    }

    #[test]
    fn the_tags_and_what_wears_them_survive_a_session() {
        let mut tags = Tags::default();
        let (sunday, unworn) = (tags.make("Sunday").unwrap(), tags.make("Loud").unwrap());
        tags.rename(sunday, "Sunday\tmorning".into());
        tags.set(7, sunday, true);
        tags.set(8, sunday, true);

        let after = Tags::read(&tags.written());
        assert_eq!(after.all().len(), 2, "an unused tag is still a tag");
        assert_eq!(after.name_of(sunday), Some("Sunday\tmorning"));
        assert_eq!(after.name_of(unworn), Some("Loud"));
        assert_eq!(after.count(sunday), 2);

        // An empty file or an unknown version reads as no tags.
        assert!(Tags::read("").all().is_empty());
        assert!(Tags::read("drawbar tags 99\nt\t1\tSunday\n")
            .all()
            .is_empty());
        let orphaned = Tags::read(&format!("{VERSION}\nm\t7\t3\n"));
        assert!(orphaned.worn(7).is_empty());
    }
}
