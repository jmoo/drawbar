//! Tags that label the list on this computer.
//!
//! The library keeps them as names on the entity of each asset that wears them, so a tag
//! follows its asset's file through a rename, and every tag made as a name in its list
//! of tags, so one that nothing wears stays. What this list shows is what the library's
//! view says: a change the user makes is asked for, and shows once the library has
//! committed it, but a new tag, a rename or a removal shows at once, over the views that
//! arrive until the library has answered it. An asset can have any number of tags.

use std::collections::{BTreeMap, BTreeSet};

use crate::named::{List, Named};

/// A tag is only a name.
pub type Tag = Named;

/// A change to the tags the user asked for, by tag name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Asked {
    /// A new tag, worn by nothing yet.
    Make(String),
    Tag {
        assets: Vec<u64>,
        name: String,
    },
    Untag {
        assets: Vec<u64>,
        name: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Remove(String),
    /// Let go of an asset whose file is gone, with its tags.
    Forget(u64),
}

/// Which assets have which tags.
#[derive(Default)]
pub struct Tags {
    list: List,
    /// The tags on each asset, by workspace id. An asset with no entry is untagged.
    of: BTreeMap<u64, BTreeSet<u64>>,
    /// The tags made here that the library does not name yet.
    unworn: BTreeSet<u64>,
    asked: Vec<Asked>,
    /// The renames and removals asked for that the library may not have made yet, in
    /// order: each name, and what it is called now, or `None` once removed.
    pending: Vec<(String, Option<String>)>,
    /// Counts changes to the names and to which asset wears which tag.
    revision: u64,
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

    /// Counts changes to the names and to which asset wears which tag.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// A new tag with a name no other tag uses, worn by nothing, or `None` when the list
    /// has no id left ([`List::make`]).
    pub(crate) fn make(&mut self, wanted: &str) -> Option<u64> {
        let id = self.list.make(wanted)?;
        self.revision += 1;
        self.unworn.insert(id);
        let name = self.list.name_of(id)?.to_string();
        self.asked.push(Asked::Make(name));
        Some(id)
    }

    /// Rename a tag here, and ask the library to rename it on every asset that wears it.
    pub(crate) fn rename(&mut self, id: u64, name: String) {
        let Some(from) = self.list.name_of(id).map(str::to_string) else {
            return;
        };
        if from == name {
            return;
        }
        self.revision += 1;
        self.list.rename(id, name.clone());
        self.pending.push((from.clone(), Some(name.clone())));
        self.asked.push(Asked::Rename { from, to: name });
    }

    /// Remove a tag from the list, and ask the library to take it off every asset. Their
    /// other tags stay.
    pub(crate) fn remove(&mut self, id: u64) {
        let Some(name) = self.list.name_of(id).map(str::to_string) else {
            return;
        };
        self.revision += 1;
        self.list.remove(id);
        for worn in self.of.values_mut() {
            worn.remove(&id);
        }
        self.of.retain(|_, worn| !worn.is_empty());
        self.unworn.remove(&id);
        self.pending.push((name.clone(), None));
        self.asked.push(Asked::Remove(name));
    }

    /// Ask the library to put a tag on each of these assets.
    pub(crate) fn tag(&mut self, assets: &[u64], tag: u64) {
        let name = self.list.name_of(tag).filter(|_| !assets.is_empty());
        if let Some(name) = name {
            let (assets, name) = (assets.to_vec(), name.to_string());
            self.asked.push(Asked::Tag { assets, name });
        }
    }

    /// Ask the library to take a tag off each of these assets.
    pub(crate) fn untag(&mut self, assets: &[u64], tag: u64) {
        let name = self.list.name_of(tag).filter(|_| !assets.is_empty());
        if let Some(name) = name {
            let (assets, name) = (assets.to_vec(), name.to_string());
            self.asked.push(Asked::Untag { assets, name });
        }
    }

    /// Ask the library to let go of an asset whose file is gone, with its tags.
    pub(crate) fn forget(&mut self, asset: u64) {
        self.asked.push(Asked::Forget(asset));
    }

    /// The changes asked for since the last call, in order.
    pub(crate) fn take_asked(&mut self) -> Vec<Asked> {
        std::mem::take(&mut self.asked)
    }

    /// The library has answered every change asked for: what it shows next is all there
    /// is.
    pub(crate) fn settle(&mut self) {
        if self.asked.is_empty() {
            self.pending.clear();
        }
    }

    /// What the name `name` the library's view holds is shown as: renamed or removed
    /// here, until the library has answered.
    fn shown(&self, name: &str) -> Option<String> {
        let mut name = name.to_string();
        for (from, to) in &self.pending {
            if *from == name {
                name = to.clone()?;
            }
        }
        Some(name)
    }

    /// Show what the library's view says: the names it holds, and the names each asset
    /// wears. Each name keeps the id it had; a tag the view no longer names goes, unless it
    /// was made here and nothing wears it yet.
    pub(crate) fn project(
        &mut self,
        named: BTreeSet<String>,
        worn: BTreeMap<u64, BTreeSet<String>>,
    ) {
        self.revision += 1;
        let named: BTreeSet<String> = named.iter().filter_map(|name| self.shown(name)).collect();
        let worn: BTreeMap<u64, BTreeSet<String>> = worn
            .into_iter()
            .map(|(asset, names)| {
                let names = names.iter().filter_map(|name| self.shown(name)).collect();
                (asset, names)
            })
            .collect();
        for name in &named {
            match self.list.id_of(name) {
                Some(id) => _ = self.unworn.remove(&id),
                None => _ = self.list.make(name),
            }
        }
        let gone: Vec<u64> = self
            .list
            .all()
            .iter()
            .filter(|tag| !named.contains(&tag.name) && !self.unworn.contains(&tag.id))
            .map(|tag| tag.id)
            .collect();
        for id in gone {
            self.list.remove(id);
        }
        let list = &self.list;
        self.of = worn
            .into_iter()
            .map(|(asset, names)| {
                let ids = names.iter().filter_map(|name| list.id_of(name)).collect();
                (asset, ids)
            })
            .filter(|(_, ids): &(u64, BTreeSet<u64>)| !ids.is_empty())
            .collect();
    }

    /// Put a tag on an asset, or take it off, as a view the library committed would.
    #[cfg(test)]
    pub(crate) fn set(&mut self, asset: u64, tag: u64, on: bool) {
        if !self.list.holds(tag) {
            return;
        }
        self.revision += 1;
        match on {
            true => _ = self.of.entry(asset).or_default().insert(tag),
            false => {
                if let Some(worn) = self.of.get_mut(&asset) {
                    worn.remove(&tag);
                }
                self.of.retain(|_, worn| !worn.is_empty());
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(of: &[&str]) -> BTreeSet<String> {
        of.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_change_is_asked_for_and_shows_only_once_the_view_says_so() {
        let mut tags = Tags::default();
        let sunday = tags.make("Sunday").unwrap();
        tags.tag(&[7, 8], sunday);
        assert_eq!(
            tags.take_asked(),
            [
                Asked::Make("Sunday".into()),
                Asked::Tag {
                    assets: vec![7, 8],
                    name: "Sunday".into()
                }
            ]
        );
        assert!(
            tags.worn(7).is_empty(),
            "nothing shows before the library says"
        );

        let worn = BTreeMap::from([(7, names(&["Sunday"])), (8, names(&["Sunday"]))]);
        tags.project(names(&["Sunday"]), worn);
        assert_eq!(
            tags.worn(7),
            &BTreeSet::from([sunday]),
            "the id it was made with"
        );
        assert_eq!(tags.count(sunday), 2);
    }

    #[test]
    fn a_tag_made_here_stays_until_worn_and_goes_once_the_view_drops_it() {
        let mut tags = Tags::default();
        let fresh = tags.make("Fresh").unwrap();
        tags.project(BTreeSet::new(), BTreeMap::new());
        assert_eq!(
            tags.name_of(fresh),
            Some("Fresh"),
            "made here, worn by nothing"
        );

        tags.project(names(&["Fresh"]), BTreeMap::from([(7, names(&["Fresh"]))]));
        tags.project(BTreeSet::new(), BTreeMap::new());
        assert_eq!(
            tags.name_of(fresh),
            None,
            "worn once, then untagged everywhere"
        );
    }

    #[test]
    fn a_rename_shows_at_once_over_views_from_before_it_until_the_library_answers() {
        let mut tags = Tags::default();
        let sunday = tags.make("Sunday").unwrap();
        let worn = || BTreeMap::from([(7, names(&["Sunday"]))]);
        tags.project(names(&["Sunday"]), worn());
        tags.take_asked();
        tags.rename(sunday, "Monday".into());
        assert_eq!(
            tags.take_asked(),
            [Asked::Rename {
                from: "Sunday".into(),
                to: "Monday".into()
            }]
        );
        tags.project(names(&["Sunday"]), worn());
        assert_eq!(
            tags.name_of(sunday),
            Some("Monday"),
            "a view from before it"
        );
        assert_eq!(tags.worn(7), &BTreeSet::from([sunday]));

        tags.settle();
        tags.project(names(&["Sunday"]), worn());
        assert_eq!(tags.name_of(sunday), None, "the library kept the old name");
        assert_eq!(tags.all().len(), 1);
    }

    #[test]
    fn a_removal_shows_at_once_and_asks_the_library() {
        let mut tags = Tags::default();
        let sunday = tags.make("Sunday").unwrap();
        tags.project(
            names(&["Sunday"]),
            BTreeMap::from([(7, names(&["Sunday"]))]),
        );
        tags.take_asked();
        tags.remove(sunday);
        assert_eq!(tags.take_asked(), [Asked::Remove("Sunday".into())]);
        tags.project(
            names(&["Sunday"]),
            BTreeMap::from([(7, names(&["Sunday"]))]),
        );
        assert!(tags.all().is_empty(), "a view from before it");
        assert!(tags.worn(7).is_empty());
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
}
