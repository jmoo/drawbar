//! Field-path lookup over a decoded [`Entity`] — the consumer side of the
//! corpus's oracle-sidecar field paths.
//!
//! A path is either a registry field (`center_panel.transpose`), which any
//! `#[bitbody]` format answers by declaration, or one of the documented
//! accessor paths — the organ accessors, the part mix, the settings selection,
//! a song's program list, a sample's zone layout — which are meaning rather
//! than placement and are spelled here by hand.

use nord_format::bank::Item;
use nord_format::fields::{Field, FieldValue};
use nord_format::formats::ne5::{self, OrganModel};
use nord_format::{Entity, Live, OrganPreset, PianoPreset, Program, Sample, Settings, Song, Synth};

/// The registry fields of any entity whose body declares one. `None` for the
/// container-verified stubs, which decode nothing to ask about.
pub fn field_values(entity: &Entity) -> Option<Vec<FieldValue>> {
    match entity {
        Entity::Program(Program::Electro5(f)) | Entity::Live(Live::Electro5(f)) => {
            Some(f.field_values())
        }
        Entity::Program(Program::Stage2(f)) | Entity::Live(Live::Stage2(f)) => {
            Some(f.field_values())
        }
        Entity::Program(Program::Stage3(f)) | Entity::Live(Live::Stage3(f)) => {
            Some(f.field_values())
        }
        Entity::Program(Program::Stage4(f)) | Entity::Live(Live::Stage4(f)) => {
            Some(f.field_values())
        }
        Entity::Settings(Settings::Electro5(f)) => Some(f.field_values()),
        Entity::Song(Song::Electro5(f)) => Some(f.field_values()),
        Entity::Synth(Synth::Stage3(f)) => Some(f.field_values()),
        Entity::Synth(Synth::Stage4(f)) => Some(f.field_values()),
        Entity::OrganPreset(OrganPreset::Stage4(f)) => Some(f.field_values()),
        Entity::PianoPreset(PianoPreset::Stage4(f)) => Some(f.field_values()),
        _ => None,
    }
}

/// The same dispatch as [`field_values`], for the settable-field view, which
/// carries both of a value's canonical spellings.
fn registry(entity: &Entity) -> Option<Vec<Field>> {
    match entity {
        Entity::Program(Program::Electro5(f)) | Entity::Live(Live::Electro5(f)) => Some(f.fields()),
        Entity::Program(Program::Stage2(f)) | Entity::Live(Live::Stage2(f)) => Some(f.fields()),
        Entity::Program(Program::Stage3(f)) | Entity::Live(Live::Stage3(f)) => Some(f.fields()),
        Entity::Program(Program::Stage4(f)) | Entity::Live(Live::Stage4(f)) => Some(f.fields()),
        Entity::Settings(Settings::Electro5(f)) => Some(f.fields()),
        Entity::Song(Song::Electro5(f)) => Some(f.fields()),
        Entity::Synth(Synth::Stage3(f)) => Some(f.fields()),
        Entity::Synth(Synth::Stage4(f)) => Some(f.fields()),
        Entity::OrganPreset(OrganPreset::Stage4(f)) => Some(f.fields()),
        Entity::PianoPreset(PianoPreset::Stage4(f)) => Some(f.fields()),
        _ => None,
    }
}

/// Every spelling `path` decodes to in `entity` — a sidecar value matching any
/// of them matches the field. Errs on a path the entity cannot answer, which is
/// a sidecar defect, not a mismatch.
pub fn lookup(entity: &Entity, path: &str) -> Result<Vec<String>, String> {
    match entity {
        Entity::Song(Song::Electro5(song)) => match path {
            "location" => return Ok(vec![format!("{:?}", song.location().inner())]),
            "programs" => {
                let refs: Vec<(u16, u16)> = (0..4).map(|slot| song.get(slot).inner()).collect();
                return Ok(vec![format!("{refs:?}")]);
            }
            _ => {}
        },
        Entity::Program(Program::Electro5(p)) | Entity::Live(Live::Electro5(p)) => {
            if let Some(spelled) = ne5_program_path(p, path)? {
                return Ok(spelled);
            }
        }
        Entity::Settings(Settings::Electro5(_)) => {
            // The sidecar's `selection.*` names are the `startup_*` fields; its
            // `panel.*` names are the flat menu-settings registry.
            let mapped = if let Some(rest) = path.strip_prefix("selection.") {
                format!("startup_{rest}")
            } else if let Some(rest) = path.strip_prefix("panel.") {
                rest.to_string()
            } else {
                path.to_string()
            };
            return registry_lookup(entity, &mapped);
        }
        Entity::Sample(Sample::V2(s)) => match path {
            "name" => return Ok(vec![s.name().map_err(|e| e.to_string())?]),
            "version" => return Ok(vec![s.header.version.to_string()]),
            "root_keys" => {
                let roots: Vec<u8> = s
                    .strokes()
                    .map_err(|e| e.to_string())?
                    .iter()
                    .map(|s| s.root_key)
                    .collect();
                return Ok(vec![format!("{roots:?}")]);
            }
            "top_notes" => {
                let tops: Vec<u8> = s
                    .zones()
                    .map_err(|e| e.to_string())?
                    .iter()
                    .map(|z| z.top_note)
                    .collect();
                return Ok(vec![format!("{tops:?}")]);
            }
            _ => {}
        },
        _ => {}
    }
    registry_lookup(entity, path)
}

fn registry_lookup(entity: &Entity, name: &str) -> Result<Vec<String>, String> {
    let fields = registry(entity).ok_or("entity declares no field registry")?;
    let field = fields
        .into_iter()
        .find(|f| f.path == name)
        .ok_or_else(|| format!("no field {name}"))?;
    Ok(vec![field.value, field.display])
}

/// The Electro 5 program paths that are not registry fields: the slot, the part
/// mix's two halves as percentages, and the organ accessors.
fn ne5_program_path(
    p: &nord_format::cbin::Cbin<ne5::Program>,
    path: &str,
) -> Result<Option<Vec<String>>, String> {
    if path == "location" {
        return Ok(Some(vec![format!("{:?}", p.location().inner())]));
    }
    if let Some(half) = path.strip_prefix("center_panel.part_mix.") {
        let mix = &p.center_panel.part_mix;
        let value = match half {
            "lower" => mix.lower(),
            "upper" => mix.upper(),
            other => return Err(format!("no part-mix half {other}")),
        };
        return Ok(Some(vec![format!("{value}")]));
    }
    let Some(accessor) = path.strip_prefix("organ_panel.") else {
        return Ok(None);
    };
    let o = &p.organ_panel;
    let spelled = match accessor {
        "b3_perc_third" => o.b3_perc_third().to_string(),
        "b3_perc_speed" => format!("{:?}", o.b3_perc_speed()),
        "b3_bass_drawbars" => format!("{:?}", o.b3_bass_drawbars()),
        call => {
            let (name, args) = call
                .strip_suffix(')')
                .and_then(|c| c.split_once('('))
                .ok_or_else(|| format!("unknown organ path {call}"))?;
            let args: Vec<&str> = args.split(',').map(str::trim).collect();
            match (name, args.as_slice()) {
                ("preset", [m]) => o.preset(model(m)?).to_string(),
                ("drawbars", [m, p]) => format!("{:?}", o.drawbars(model(m)?, preset(p)?)),
                ("vib_on", [m, p]) => o.vib_on(model(m)?, preset(p)?).to_string(),
                ("vib_type", [m]) => format!("{:?}", o.vib_type(model(m)?)),
                ("b3_perc_on", [p]) => o.b3_perc_on(preset(p)?).to_string(),
                _ => return Err(format!("unknown organ accessor {call}")),
            }
        }
    };
    Ok(Some(vec![spelled]))
}

fn model(name: &str) -> Result<OrganModel, String> {
    match name {
        "B3" => Ok(OrganModel::B3),
        "Vox" => Ok(OrganModel::Vox),
        "Farfisa" => Ok(OrganModel::Farfisa),
        "Pipe" => Ok(OrganModel::Pipe),
        other => Err(format!("unknown organ model {other}")),
    }
}

fn preset(digit: &str) -> Result<u8, String> {
    digit
        .parse()
        .map_err(|_| format!("bad preset argument {digit}"))
}
