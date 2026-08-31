//! The accessor-backed editors: bodies whose settable fields come from typed
//! accessors rather than the generated registry — the sample instrument, the
//! set list, and the Sample Editor project. One vocabulary — `--set
//! path=value` — over each, with paths spelled the way `nord inspect` prints
//! the same things.

use nord_format::cbin::Cbin;
use nord_format::formats::ne5::{program, song, Song};
use nord_format::formats::nsmpproj::Project;
use nord_format::Sample;

use crate::note;
use crate::ui::Ui;

/// One settable field: its path, its current value, and what it takes.
pub struct Row {
    pub path: String,
    pub value: String,
    pub accepts: String,
}

/// A body whose fields are listed and set by hand-written accessors.
pub trait Fields {
    fn rows(&self) -> Result<Vec<Row>, String>;
    fn set(&mut self, path: &str, value: &str) -> Result<(), String>;
}

/// List the fields (`--fields`, `None`) or apply every `--set`, returning how
/// many fields moved — the accessor-backed twin of the registry staging.
pub fn stage(
    ui: &Ui,
    fields: bool,
    sets: &[String],
    editor: &mut dyn Fields,
) -> Result<Option<usize>, String> {
    if fields {
        if !sets.is_empty() {
            return Err("--fields lists and writes nothing; drop it to apply --set".into());
        }
        list(ui, editor)?;
        return Ok(None);
    }
    if sets.is_empty() {
        return Err("nothing to do: pass --set PATH=VALUE, or --fields to see what exists".into());
    }

    // Every change lands before anything is written, so a bad path or an
    // out-of-range value cannot leave a half-edited body behind.
    let before = editor.rows()?;
    for assignment in sets {
        let (path, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("expected PATH=VALUE, got {assignment:?}"))?;
        editor.set(path.trim(), value.trim())?;
    }
    let after = editor.rows()?;

    let mut changed = 0;
    for (b, a) in before.iter().zip(&after) {
        if b.value != a.value {
            changed += 1;
            ui.out(format!(
                "{:<24} {} -> {}",
                a.path,
                b.value,
                ui.bold(&a.value)
            ));
        }
    }
    Ok(Some(changed))
}

fn list(ui: &Ui, editor: &dyn Fields) -> Result<(), String> {
    ui.out(format!("{:<24} {:<40} {}", "path", "value", "accepts"));
    for row in editor.rows()? {
        ui.out(format!(
            "{:<24} {:<40} {}",
            row.path, row.value, row.accepts
        ));
    }
    Ok(())
}

const NOTE_ACCEPTS: &str = "a note name (C4, F#3) or 0-127";

fn unknown(path: &str) -> String {
    format!("unknown field {path:?}; --fields lists what exists")
}

/// `zone3` → 3, under any label — the 1-based spelling every listing uses.
fn indexed(part: &str, label: &str) -> Option<usize> {
    part.strip_prefix(label)
        .and_then(|n| n.parse().ok())
        .filter(|&n| n >= 1)
}

/// The sample instrument: its name, plus each zone's root key and boundaries
/// where the keyboard layout can be edited without leaving another map stale.
/// `low_note` is listed only where the generation stores one; elsewhere zones
/// tile and a zone's bottom follows from the one below it.
pub struct SampleEditor<'a>(pub &'a mut Sample);

impl Fields for SampleEditor<'_> {
    fn rows(&self) -> Result<Vec<Row>, String> {
        let sample = &self.0;
        let mut out = vec![Row {
            path: "name".into(),
            value: sample.name().map_err(|e| e.to_string())?,
            accepts: format!("up to {} bytes", sample.max_name_len()),
        }];
        if !sample.zones_are_editable() {
            return Ok(out);
        }
        for (i, zone) in sample
            .zones()
            .map_err(|e| e.to_string())?
            .iter()
            .enumerate()
        {
            let n = i + 1;
            out.push(Row {
                path: format!("zone{n}.root_key"),
                value: note::name(zone.root_key),
                accepts: NOTE_ACCEPTS.into(),
            });
            out.push(Row {
                path: format!("zone{n}.top_note"),
                value: note::name(zone.top_note),
                accepts: NOTE_ACCEPTS.into(),
            });
            if let Some(low) = zone.low_note {
                out.push(Row {
                    path: format!("zone{n}.low_note"),
                    value: note::name(low),
                    accepts: NOTE_ACCEPTS.into(),
                });
            }
        }
        Ok(out)
    }

    fn set(&mut self, path: &str, value: &str) -> Result<(), String> {
        let sample = &mut self.0;
        if path == "name" {
            return sample.set_name(value).map_err(|e| e.to_string());
        }
        let (zone, field) = path.split_once('.').ok_or_else(|| unknown(path))?;
        let index = indexed(zone, "zone").ok_or_else(|| unknown(path))?;
        // Checked here so the message speaks the CLI's 1-based numbering, not
        // the format crate's 0-based one.
        let zones = sample.zones().map_err(|e| e.to_string())?.len();
        if index > zones {
            return Err(format!("no zone {index}: the instrument has {zones}"));
        }
        let value = note::parse(value)?;
        match field {
            "root_key" => sample.set_root_key(index - 1, value),
            "top_note" => sample.set_zone_top_note(index - 1, value),
            "low_note" => sample.set_zone_low_note(index - 1, value),
            _ => return Err(unknown(path)),
        }
        .map_err(|e| e.to_string())
    }
}

/// The set list: the four program slots it points at, spelled `BANK:SLOT` the
/// way the instrument shows them.
pub struct SongEditor<'a>(pub &'a mut Cbin<Song>);

impl Fields for SongEditor<'_> {
    fn rows(&self) -> Result<Vec<Row>, String> {
        Ok((0..song::PROGRAM_COUNT as u16)
            .map(|slot| {
                let (bank, at) = self.0.get(slot).inner();
                Row {
                    path: format!("slot{}", slot + 1),
                    value: format!("{}:{}", bank + 1, at + 1),
                    accepts: format!(
                        "a program slot, BANK:SLOT (1:1 .. {}:{})",
                        program::BANK_COUNT,
                        program::SLOT_COUNT
                    ),
                }
            })
            .collect())
    }

    fn set(&mut self, path: &str, value: &str) -> Result<(), String> {
        let slot = indexed(path, "slot")
            .filter(|&n| n <= song::PROGRAM_COUNT)
            .ok_or_else(|| unknown(path))?;
        let at = crate::slot::parse(value)?;
        let target: program::Location = (at.bank as u16, at.slot as u16)
            .try_into()
            .map_err(|e| format!("{path}: {e}"))?;
        self.0.set(slot as u16 - 1, target);
        Ok(())
    }
}

/// The Sample Editor project: the instrument name, each zone's root key and
/// key range, and each audio file's path. Zones and files are addressed by the
/// ids `nord inspect` prints.
pub struct ProjectEditor<'a>(pub &'a mut Project);

impl Fields for ProjectEditor<'_> {
    fn rows(&self) -> Result<Vec<Row>, String> {
        let project = &self.0;
        let mut out = vec![Row {
            path: "name".into(),
            value: project.name().map_err(|e| e.to_string())?,
            accepts: "the instrument's name".into(),
        }];
        for zone in project.zones().map_err(|e| e.to_string())? {
            let id = zone.zone_id;
            for (field, value) in [
                ("root_key", zone.root_key),
                ("bottom_note", zone.bottom_note),
                ("top_note", zone.top_note),
            ] {
                out.push(Row {
                    path: format!("zone{id}.{field}"),
                    value: note::name(value),
                    accepts: NOTE_ACCEPTS.into(),
                });
            }
        }
        for file in project.audio_files().map_err(|e| e.to_string())? {
            out.push(Row {
                path: format!("file{}.path", file.id),
                value: file.path,
                accepts: "a path the editor resolves from the project's directory".into(),
            });
        }
        Ok(out)
    }

    fn set(&mut self, path: &str, value: &str) -> Result<(), String> {
        let project = &mut self.0;
        if path == "name" {
            return project.set_name(value).map_err(|e| e.to_string());
        }
        let (block, field) = path.split_once('.').ok_or_else(|| unknown(path))?;
        if let Some(id) = indexed(block, "file") {
            if field != "path" {
                return Err(unknown(path));
            }
            return project
                .set_audio_path(id as u32, value)
                .map_err(|e| e.to_string());
        }
        let id = indexed(block, "zone").ok_or_else(|| unknown(path))? as u32;
        let zones = project.zones().map_err(|e| e.to_string())?;
        let zone = zones
            .iter()
            .find(|z| z.zone_id == id)
            .ok_or_else(|| format!("no zone {id}; --fields lists the ids this project holds"))?;
        let note = note::parse(value)?;
        match field {
            "root_key" => project.set_root_key(id, note),
            "bottom_note" => project.set_key_range(id, note, zone.top_note),
            "top_note" => project.set_key_range(id, zone.bottom_note, note),
            _ => return Err(unknown(path)),
        }
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_format::cbin::Header;
    use nord_format::formats::ne5;
    use nord_format::formats::nsmp::{section, SampleV3};
    use nord_format::formats::nsmpproj::NewZone;

    fn project() -> Project {
        Project::new(
            "Marimba",
            &[
                NewZone {
                    path: "low.wav".into(),
                    sample_rate: 44100,
                    frames: 44100,
                    root_key: 48,
                },
                NewZone {
                    path: "high.wav".into(),
                    sample_rate: 44100,
                    frames: 44100,
                    root_key: 72,
                },
            ],
            0,
        )
        .unwrap()
    }

    fn sample_with_key_map() -> Sample {
        let mut map = vec![0u8; 12 + 128 * 10 + 1 + 2 * 16 + 2];
        for key in 0..128 {
            map[12 + key * 10..][..4].fill(key as u8);
        }
        let record = 12 + 128 * 10 + 1;
        map[record - 1] = 2;
        for key in 48..=60 {
            map[12 + key * 10..][..3].fill(62);
        }
        for key in 61..=84 {
            map[12 + key * 10..][..3].fill(60);
        }
        for (i, (root, top, low, gid)) in [(60, 60, 48, 9u32), (62, 84, 61, 10)]
            .into_iter()
            .enumerate()
        {
            let at = record + i * 16;
            map[at] = root;
            map[at + 1] = top;
            map[at + 2] = low;
            map[at + 8..at + 12].copy_from_slice(&gid.to_be_bytes());
        }

        Sample::V3(Cbin {
            header: Header::new("nsmp", (0, 0), 400),
            body: SampleV3 {
                sections: vec![
                    section::Section4 {
                        tag: *section::HDR4,
                        version: 1,
                        payload: vec![0; 76],
                    },
                    section::Section4 {
                        tag: *section::MAP4,
                        version: 21,
                        payload: map,
                    },
                    section::Section4 {
                        tag: *section::STK4,
                        version: 1,
                        payload: vec![0, 0, 0, 9, 0, 60],
                    },
                    section::Section4 {
                        tag: *section::STK4,
                        version: 1,
                        payload: vec![0, 0, 0, 10, 0, 62],
                    },
                ],
            },
        })
    }

    /// A wide sample whose `map` is too short to hold a zone table.
    fn sample_with_unreadable_map() -> Sample {
        let Sample::V3(mut body) = sample_with_key_map() else {
            unreachable!()
        };
        section::find_mut4(&mut body.body.sections, section::MAP4)
            .unwrap()
            .payload = vec![0; 8];
        Sample::V3(body)
    }

    /// A populated keyboard map is recomputed from the zones as they move, so
    /// its zone paths are listed like any other instrument's.
    #[test]
    fn a_populated_key_map_still_lists_its_zones() {
        let mut sample = sample_with_key_map();
        let paths: Vec<String> = SampleEditor(&mut sample)
            .rows()
            .unwrap()
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert!(paths.contains(&"zone1.root_key".to_string()), "{paths:?}");
    }

    /// An instrument whose zones cannot be read lists its name and stops there,
    /// rather than failing the listing outright.
    #[test]
    fn uneditable_zone_paths_are_not_listed() {
        let mut sample = sample_with_unreadable_map();
        let rows = SampleEditor(&mut sample).rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "name");
    }

    /// The paths a listing prints are the paths `set` takes, ids included.
    #[test]
    fn project_paths_round_trip_from_the_listing() {
        let mut project = project();
        let rows = ProjectEditor(&mut project).rows().unwrap();
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert!(paths.contains(&"name"), "{paths:?}");
        // Ids rise with the root key from FIRST_ZONE_ID, and inspect prints them.
        assert!(paths.iter().any(|p| p.starts_with("zone129.")), "{paths:?}");
        assert!(paths.contains(&"file1.path"), "{paths:?}");

        let mut editor = ProjectEditor(&mut project);
        for (path, value) in [
            ("name", "Vibes"),
            ("zone129.root_key", "C2"),
            ("file1.path", "verylow.wav"),
        ] {
            editor.set(path, value).unwrap();
        }
        assert_eq!(project.name().unwrap(), "Vibes");
        // Zones are stored high to low; 129 is the lowest, whatever its position.
        let zones = project.zones().unwrap();
        let edited = zones.iter().find(|z| z.zone_id == 129).unwrap();
        assert_eq!(edited.root_key, 36);
        assert_eq!(project.audio_files().unwrap()[0].path, "verylow.wav");
    }

    /// Setting one end of a key range keeps the other end where it was.
    #[test]
    fn a_key_range_moves_one_end_at_a_time() {
        let mut project = project();
        let before = project.zones().unwrap()[0].clone();
        ProjectEditor(&mut project)
            .set(&format!("zone{}.top_note", before.zone_id), "C7")
            .unwrap();
        let after = &project.zones().unwrap()[0];
        assert_eq!(after.top_note, 96);
        assert_eq!(after.bottom_note, before.bottom_note);
    }

    /// A slot is spelled the way the instrument shows it, and an impossible one
    /// is refused by the location type rather than clamped.
    #[test]
    fn song_slots_speak_the_instruments_numbering() {
        let mut song = ne5::song::new(
            (0, 0).try_into().unwrap(),
            ne5::song::DEFAULT_VERSION,
            [(0, 0).try_into().unwrap(); 4],
        );
        SongEditor(&mut song).set("slot2", "3:14").unwrap();
        assert_eq!(song.get(1).inner(), (2, 13));
        let rows = SongEditor(&mut song).rows().unwrap();
        assert_eq!(rows[1].path, "slot2");
        assert_eq!(rows[1].value, "3:14");

        for bad in ["9:1", "1:51"] {
            assert!(SongEditor(&mut song).set("slot1", bad).is_err(), "{bad}");
        }
        assert!(SongEditor(&mut song).set("slot5", "1:1").is_err());
    }
}
