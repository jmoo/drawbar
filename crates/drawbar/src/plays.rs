//! The pianos and samples programs play, found by name.
//!
//! A program names a piano or sample by the [`library_id`] of the name the instrument
//! keeps it under, which is the name a write gives its slot. A file on this computer and a
//! slot the walk read are both found by hashing their names, with or without an
//! instrument attached.

use std::collections::HashMap;

use nord_format::components::library_id;
use nord_usb::{Location, ObjectClass};

use crate::browser::Kind;
use crate::device::{occupied, slot_label, DeviceState};
use crate::summary::Plays;
use crate::workspace::{LocalEntity, Workspace};

/// What a program names a piano or sample by: its class and id.
pub type Key = (ObjectClass, u32);

impl Plays {
    pub fn key(self) -> Key {
        (self.class(), self.id())
    }
}

/// The key a piano or sample file goes by once written to the instrument, or `None` for
/// any other file.
pub fn key_of(entity: &LocalEntity) -> Option<Key> {
    let class = match Kind::of(entity) {
        Kind::Piano => ObjectClass::Piano,
        Kind::Sample => ObjectClass::Sample,
        _ => return None,
    };
    Some((class, library_id(&slot_label(&entity.name)?)))
}

/// The key of a piano or sample slot, by the name the walk read there.
pub fn slot_key(class: ObjectClass, name: &str) -> Key {
    (class, library_id(name.trim()))
}

/// What a program on this computer plays, piano first: from its decode, or from what a
/// read of it found before it was let go of.
pub fn named(entity: &LocalEntity) -> &[Plays] {
    match (&entity.entity, entity.remembered()) {
        (None, Some(known)) => &known.plays,
        _ => &entity.plays,
    }
}

/// The programs on this computer that play `key`, in list order.
pub fn played_by(workspace: &Workspace, key: Key) -> impl Iterator<Item = &LocalEntity> {
    workspace
        .listed()
        .filter(move |entity| named(entity).iter().any(|plays| plays.key() == key))
}

/// The name of the piano or sample `key` names, where a slot of the attached instrument
/// or a file on this computer goes by it.
pub fn name_of(key: Key, workspace: &Workspace, device: &DeviceState) -> Option<String> {
    let class = key.0;
    let there = occupied(device, class)
        .map(|(_, info)| info.name.trim())
        .find(|name| slot_key(class, name) == key);
    match there {
        Some(name) => Some(name.to_string()),
        None => workspace
            .listed()
            .filter(|entity| key_of(entity) == Some(key))
            .find_map(|entity| slot_label(&entity.name)),
    }
}

/// One piano or sample, wherever a program finds it.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Found {
    /// The name it is kept under.
    pub name: String,
    /// The files on this computer, in list order.
    pub here: Vec<u64>,
    /// The slot of the attached instrument holding it.
    pub there: Option<Location>,
}

/// Every piano and sample on this computer and on the attached instrument, by key.
pub struct Catalog(HashMap<Key, Found>);

impl Catalog {
    pub fn of(workspace: &Workspace, device: &DeviceState) -> Catalog {
        let mut by_key: HashMap<Key, Found> = HashMap::new();
        for class in [ObjectClass::Piano, ObjectClass::Sample] {
            for (at, info) in occupied(device, class) {
                let name = info.name.trim();
                let found = by_key.entry(slot_key(class, name)).or_default();
                found.name = name.to_string();
                found.there.get_or_insert(at);
            }
        }
        for entity in workspace.listed() {
            let Some(key) = key_of(entity) else {
                continue;
            };
            let found = by_key.entry(key).or_default();
            if found.name.is_empty() {
                found.name = slot_label(&entity.name).unwrap_or_default();
            }
            found.here.push(entity.id);
        }
        Catalog(by_key)
    }

    pub fn get(&self, key: Key) -> Option<&Found> {
        self.0.get(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Log;
    use crate::testing::Bench;
    use crate::workspace::{Fresh, Origin};

    fn program_playing(piano: u32, sample: u32) -> Vec<u8> {
        let sets = [
            ("piano_panel.id".into(), piano.to_string()),
            ("sample_panel.id".into(), sample.to_string()),
        ];
        crate::fields::apply(&Fresh::Program.bytes().unwrap(), &sets)
            .unwrap()
            .1
    }

    /// 0x92e0cba2 is the `library_id` of `drawbar-tine`, and 0xaeb35621 that of
    /// `tine-other`.
    #[test]
    fn a_piano_is_found_by_the_name_it_is_written_under() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let file = workspace.ingest(
            "drawbar-tine.npno".into(),
            Origin::Fresh,
            crate::testing::piano(1),
            &mut log,
        );
        device.pretend_lengths(ObjectClass::Piano, 3, &[Some(("tine-other ", 1000))]);

        let catalog = Catalog::of(&workspace, &device.state);
        assert_eq!(
            catalog.get((ObjectClass::Piano, 0x92e0_cba2)),
            Some(&Found {
                name: "drawbar-tine".into(),
                here: vec![file],
                there: None,
            })
        );
        assert_eq!(
            catalog.get((ObjectClass::Piano, 0xaeb3_5621)),
            Some(&Found {
                name: "tine-other".into(),
                here: Vec::new(),
                there: Some(Location::from_user(3, 1)),
            })
        );
        assert_eq!(
            catalog.get((ObjectClass::Sample, 0x92e0_cba2)),
            None,
            "a piano is not a sample"
        );
    }

    #[test]
    fn a_piano_is_played_only_by_the_programs_naming_its_id_as_their_piano() {
        let Bench { mut workspace, .. } = Bench::new();
        let mut log = Log::default();
        let (tine, other) = (0x92e0_cba2, 0xaeb3_5621);
        let plays_it = workspace.ingest(
            "Keys.ne5p".into(),
            Origin::Fresh,
            program_playing(tine, 0),
            &mut log,
        );
        workspace.ingest(
            "Pad.ne5p".into(),
            Origin::Fresh,
            program_playing(0, tine),
            &mut log,
        );
        workspace.ingest(
            "Other.ne5p".into(),
            Origin::Fresh,
            program_playing(other, 0),
            &mut log,
        );

        let by: Vec<u64> = played_by(&workspace, (ObjectClass::Piano, tine))
            .map(|entity| entity.id)
            .collect();
        assert_eq!(by, [plays_it]);
    }
}
