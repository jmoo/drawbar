//! Tags that label the list on this computer.
//!
//! Kept in the library's index by asset id, so a tag follows its asset's file through a
//! rename. An asset can have any number of tags.

use std::collections::{BTreeMap, BTreeSet};

use crate::named::{List, Named};

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

    /// Take the tags an index held: their names by id, and each asset's tags.
    ///
    /// ⚠️ A membership naming a tag the index does not list is dropped: it would leave a
    /// tag on the asset that nothing can show or remove.
    pub(crate) fn restore(
        &mut self,
        names: BTreeMap<u64, String>,
        worn: impl Iterator<Item = (u64, impl Iterator<Item = u64>)>,
    ) {
        for (id, name) in names {
            self.list.restore(id, name);
        }
        for (asset, tags) in worn {
            let tags: BTreeSet<u64> = tags.filter(|tag| self.list.holds(*tag)).collect();
            if !tags.is_empty() {
                self.of.insert(asset, tags);
            }
        }
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
    fn a_membership_naming_no_listed_tag_is_dropped() {
        let mut tags = Tags::default();
        tags.restore(
            BTreeMap::from([(1, "Sunday".to_string())]),
            [(7, vec![1, 3].into_iter()), (8, vec![3].into_iter())].into_iter(),
        );
        assert_eq!(tags.worn(7), &BTreeSet::from([1]));
        assert!(tags.worn(8).is_empty());
        assert_eq!(tags.name_of(1), Some("Sunday"));
    }
}
