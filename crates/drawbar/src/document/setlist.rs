//! The set list document: the four program slots a song points at, each
//! editable as the `BANK:SLOT` pair the instrument shows.

use eframe::egui;
use nord_format::cbin::Cbin;
use nord_format::formats::ne5::{program, song, Song};
use nord_format::Entity;

use super::controls::Sets;

fn song(entity: &Entity) -> Option<&Cbin<Song>> {
    match entity {
        Entity::Song(nord_format::Song::Electro5(song)) => Some(song),
        _ => None,
    }
}

fn song_mut(entity: &mut Entity) -> Option<&mut Cbin<Song>> {
    match entity {
        Entity::Song(nord_format::Song::Electro5(song)) => Some(song),
        _ => None,
    }
}

/// Apply one `path = value`: `slot1 = 2:5`, both numbers as the panel shows them.
fn set(file: &mut Cbin<Song>, path: &str, value: &str) -> Result<(), String> {
    let slot = path
        .strip_prefix("slot")
        .and_then(|n| n.parse::<u16>().ok())
        .filter(|&n| (1..=song::PROGRAM_COUNT as u16).contains(&n))
        .ok_or_else(|| format!("unknown field {path:?}"))?;
    let (bank, at) = value
        .split_once(':')
        .ok_or_else(|| format!("{path}: expected BANK:SLOT, got {value:?}"))?;
    let bank: u16 = bank
        .trim()
        .parse()
        .map_err(|_| format!("bad bank {bank:?}"))?;
    let at: u16 = at.trim().parse().map_err(|_| format!("bad slot {at:?}"))?;
    if bank == 0 || at == 0 {
        return Err("banks and slots are numbered from 1, as shown on the instrument".into());
    }
    let target: program::Location = (bank - 1, at - 1)
        .try_into()
        .map_err(|e| format!("{path}: {e}"))?;
    file.set(slot - 1, target);
    Ok(())
}

/// Apply every set to a fresh decode and re-encode, the same all-or-nothing rule
/// the registry bodies follow.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut entity =
        nord_format::from_stream(&mut std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let file = song_mut(&mut entity).ok_or("not an Electro 5 set list")?;
    for (path, value) in sets {
        set(file, path, value)?;
    }
    nord_format::to_bytes(&entity).map_err(|e| e.to_string())
}

/// The four programs the set list plays — the whole of what a set list is.
pub fn ui(ui: &mut egui::Ui, entity: &Entity, sets: &mut Sets) {
    let Some(file) = song(entity) else {
        ui.label(egui::RichText::new("Nothing about this format is editable here yet.").weak());
        return;
    };
    ui.label(
        egui::RichText::new("The four programs this set list plays.")
            .small()
            .weak(),
    );
    for (i, at) in file.programs().iter().enumerate() {
        let n = i + 1;
        let (bank, slot) = at.inner();
        ui.horizontal(|ui| {
            ui.add_sized(
                [90.0, ui.spacing().interact_size.y],
                egui::Label::new(format!("Slot {n}")).halign(egui::Align::LEFT),
            );
            ui.label("program");
            let picked_bank = number(ui, ("song_bank", n), bank + 1, 1..=program::BANK_COUNT);
            let picked_slot = number(ui, ("song_slot", n), slot + 1, 1..=program::SLOT_COUNT);
            if picked_bank != bank + 1 || picked_slot != slot + 1 {
                sets.push((format!("slot{n}"), format!("{picked_bank}:{picked_slot}")));
            }
        });
    }
}

/// One half of a program address, kept inside the instrument's own range.
fn number(
    ui: &mut egui::Ui,
    id: (&str, usize),
    value: u16,
    range: std::ops::RangeInclusive<u16>,
) -> u16 {
    let mut shown = value;
    ui.push_id(id, |ui| {
        ui.add(egui::DragValue::new(&mut shown).range(range).speed(0.2))
    });
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_format::formats::ne5;

    fn set_list() -> Vec<u8> {
        let song = ne5::song::new(
            (0, 0).try_into().unwrap(),
            ne5::song::DEFAULT_VERSION,
            [(0, 0).try_into().unwrap(); 4],
        );
        nord_format::to_bytes(&nord_format::Entity::Song(nord_format::Song::Electro5(
            song,
        )))
        .unwrap()
    }

    /// A slot lands where it was aimed, spelled the way the panel spells it, and
    /// the result still decodes and round-trips.
    #[test]
    fn a_slot_edit_lands_and_round_trips() {
        let bytes = set_list();
        let out = apply(&bytes, &[("slot2".into(), "3:14".into())]).unwrap();
        let entity = nord_format::from_stream(&mut std::io::Cursor::new(&out)).unwrap();
        let file = song(&entity).unwrap();
        assert_eq!(file.get(1).inner(), (2, 13));
        assert_eq!(nord_format::to_bytes(&entity).unwrap(), out);
    }

    /// An address the bank map cannot hold is refused before anything is encoded.
    #[test]
    fn an_impossible_address_is_refused() {
        let bytes = set_list();
        for bad in ["9:1", "1:51", "0:1", "nonsense"] {
            assert!(
                apply(&bytes, &[("slot1".into(), bad.into())]).is_err(),
                "{bad}"
            );
        }
        assert!(apply(&bytes, &[("slot5".into(), "1:1".into())]).is_err());
    }
}
