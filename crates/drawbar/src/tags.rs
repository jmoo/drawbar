//! How the list on this computer is labelled.
//!
//! Beside [`crate::folders`], and stored the same way: its own key, its own version
//! line, membership by the workspace id the list is stored under. A folder is exclusive
//! and a tag is not, which is the whole difference between them.

use std::collections::{BTreeMap, BTreeSet};

use crate::store::{escape, unescape};
use crate::workspace::Workspace;

/// Where the tags and their membership are kept between sessions.
///
/// ⚠️ Membership is by workspace id, which is the same id the local list is stored
/// under — the two files are read back into one list, so they have to agree about what
/// an id means. Only a **kept** asset has one that survives a session, so a view is
/// kept before it can be tagged.
pub(crate) const KEY: &str = "drawbar.tags";

const VERSION: &str = "drawbar tags 1";

/// A tag is a name and nothing else.
pub struct Tag {
    pub id: u64,
    pub name: String,
}

/// What is labelled with what.
#[derive(Default)]
pub struct Tags {
    list: Vec<Tag>,
    /// The tags an asset wears, by its workspace id. Absent is untagged.
    of: BTreeMap<u64, BTreeSet<u64>>,
}

/// What an untagged asset wears, so a caller need not tell absent from empty.
static NOTHING: BTreeSet<u64> = BTreeSet::new();

impl Tags {
    pub fn all(&self) -> &[Tag] {
        &self.list
    }

    pub fn name_of(&self, id: u64) -> Option<&str> {
        self.list
            .iter()
            .find(|tag| tag.id == id)
            .map(|tag| tag.name.as_str())
    }

    /// A new tag, under a name nothing else in the list is using.
    pub(crate) fn make(&mut self, wanted: &str) -> u64 {
        let id = self.list.iter().map(|tag| tag.id).max().unwrap_or(0) + 1;
        let taken = |name: &str| self.list.iter().any(|tag| tag.name == name);
        let mut name = wanted.to_string();
        for nth in 2.. {
            if !taken(&name) {
                break;
            }
            name = format!("{wanted} {nth}");
        }
        self.list.push(Tag { id, name });
        id
    }

    pub(crate) fn rename(&mut self, id: u64, name: String) {
        if let Some(tag) = self.list.iter_mut().find(|tag| tag.id == id) {
            tag.name = name;
        }
    }

    /// Drop a tag. What wore it keeps everything else it wore — a tag holds nothing.
    pub(crate) fn remove(&mut self, id: u64) {
        self.list.retain(|tag| tag.id != id);
        for worn in self.of.values_mut() {
            worn.remove(&id);
        }
        self.of.retain(|_, worn| !worn.is_empty());
    }

    /// Put a tag on an asset, or take it off. A tag nobody made goes on nothing.
    pub(crate) fn set(&mut self, asset: u64, tag: u64, on: bool) {
        if on && self.name_of(tag).is_some() {
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
        let mut out = format!("{VERSION}\n");
        for tag in &self.list {
            out.push_str(&format!("t\t{}\t{}\n", tag.id, escape(&tag.name)));
        }
        for (asset, worn) in &self.of {
            for tag in worn {
                out.push_str(&format!("m\t{asset}\t{tag}\n"));
            }
        }
        out
    }

    /// Read back what [`Tags::written`] wrote. Anything unaccounted for is no tags at
    /// all — half a labelling is worse than none, because a tag nobody made is one
    /// nobody can explain.
    pub(crate) fn read(text: &str) -> Tags {
        let mut lines = text.lines();
        if lines.next() != Some(VERSION) {
            return Tags::default();
        }
        let mut tags = Tags::default();
        for line in lines {
            let mut parts = line.split('\t');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("t"), Some(id), Some(name)) => {
                    if let Ok(id) = id.parse() {
                        tags.list.push(Tag {
                            id,
                            name: unescape(name),
                        });
                    }
                }
                (Some("m"), Some(asset), Some(tag)) => {
                    if let (Ok(asset), Ok(tag)) = (asset.parse(), tag.parse()) {
                        tags.of.entry(asset).or_default().insert(tag);
                    }
                }
                _ => {}
            }
        }
        // A membership naming a tag that is not in the file would be an asset wearing
        // something nothing can show and nothing can take off.
        let known: Vec<u64> = tags.list.iter().map(|tag| tag.id).collect();
        for worn in tags.of.values_mut() {
            worn.retain(|tag| known.contains(tag));
        }
        tags.of.retain(|_, worn| !worn.is_empty());
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
                let id = tags.make(wanted);
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
        let (sunday, loud) = (tags.make("Sunday"), tags.make("Loud"));
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
        let (gone, kept) = (tags.make("Sunday"), tags.make("Loud"));
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
        let sunday = tags.make("Sunday");
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
        let (sunday, unworn) = (tags.make("Sunday"), tags.make("Loud"));
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
}
