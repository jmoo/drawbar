//! Field-path lookup over a decoded [`Entity`], for the field paths that oracle
//! sidecars name.
//!
//! A path is either a registry field (`center_panel.transpose`), which any
//! `#[bitbody]` format answers from its declaration, or an accessor path: the
//! organ accessors, the part mix, the settings selection, a song's program list,
//! or a sample's zone layout, keyboard map, and preset. Accessor paths are derived values with no single
//! bit placement, so they are spelled here by hand.

use nord_format::bank::Item;
use nord_format::formats::ne5::{self, OrganModel, Preset};
use nord_format::formats::nsmp::{self, KeyTable, Level, Sty};
use nord_format::{Entity, Live, Program, Sample, Settings, Song};

/// Every spelling of the value at `path` in `entity`. A sidecar value that
/// matches any of them matches the field. An error means the entity has no such
/// path, so the sidecar itself is wrong.
pub fn lookup(entity: &Entity, path: &str) -> Result<Vec<String>, String> {
    match entity {
        Entity::Song(Song::Electro5(song)) => match path {
            "location" => return Ok(vec![format!("{:?}", song.location().inner())]),
            "programs" => {
                let refs: Vec<(u16, u16)> = ne5::song::Slot::ALL
                    .into_iter()
                    .map(|slot| song.get(slot).inner())
                    .collect();
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
            _ => {
                if let Some(rest) = path.strip_prefix("key_table.") {
                    let table = s.key_table().map_err(|e| e.to_string())?;
                    return key_table_path(&table, rest).map(|spelled| vec![spelled]);
                }
            }
        },
        Entity::Sample(Sample::V3(s)) => {
            if let Some(spelled) = wide_sample_path(s, path)? {
                return Ok(vec![spelled]);
            }
        }
        Entity::SampleProject(p) => {
            let zones = || p.zones().map_err(|e| e.to_string());
            match path {
                "name" => return Ok(vec![p.name().map_err(|e| e.to_string())?]),
                "version" => {
                    return Ok(vec![p
                        .file_format_version()
                        .map_err(|e| e.to_string())?
                        .to_string()])
                }
                "root_keys" => {
                    let roots: Vec<u8> = zones()?.iter().map(|z| z.root_key).collect();
                    return Ok(vec![format!("{roots:?}")]);
                }
                "top_notes" => {
                    let tops: Vec<u8> = zones()?.iter().map(|z| z.top_note).collect();
                    return Ok(vec![format!("{tops:?}")]);
                }
                "bottom_notes" => {
                    let bottoms: Vec<u8> = zones()?.iter().map(|z| z.bottom_note).collect();
                    return Ok(vec![format!("{bottoms:?}")]);
                }
                "audio_files" => {
                    let files: Vec<String> = p
                        .audio_files()
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .map(|f| f.path)
                        .collect();
                    return Ok(vec![format!("{files:?}")]);
                }
                _ => {}
            }
        }
        _ => {}
    }
    registry_lookup(entity, path)
}

fn registry_lookup(entity: &Entity, name: &str) -> Result<Vec<String>, String> {
    let fields = crate::registry::fields(entity).ok_or("entity declares no field registry")?;
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

/// The organ has two presets. Any other digit is a sidecar defect.
fn preset(digit: &str) -> Result<Preset, String> {
    match digit {
        "1" => Ok(Preset::One),
        "2" => Ok(Preset::Two),
        other => Err(format!("organ preset {other}, which is not 1 or 2")),
    }
}

/// A narrow instrument's keyboard map: `neutral`, `adjusted`, and the gain or detune
/// of `instrument` or `key(<note>)`.
fn key_table_path(table: &KeyTable, path: &str) -> Result<String, String> {
    match path {
        "neutral" => return Ok((*table == KeyTable::NEUTRAL).to_string()),
        "adjusted" => return Ok(format!("{:?}", table.adjusted().collect::<Vec<_>>())),
        _ => {}
    }
    let (record, part) = path
        .rsplit_once('.')
        .ok_or_else(|| format!("unknown keyboard map path {path}"))?;
    let level: Level = match record {
        "instrument" => table.instrument,
        call => {
            let note = call
                .strip_prefix("key(")
                .and_then(|c| c.strip_suffix(')'))
                .and_then(|n| n.parse::<u8>().ok())
                .ok_or_else(|| format!("unknown keyboard map record {call}"))?;
            table.key(note).map_err(|e| e.to_string())?
        }
    };
    match part {
        "gain" => Ok(level.gain().to_string()),
        "detune" => Ok(level.detune().to_string()),
        other => Err(format!("a keyboard map record has no {other}")),
    }
}

/// A wide instrument's zone velocity windows and relative strengths, and its
/// preset's dynamics group and EQ. `None` for a path this reader does not spell.
fn wide_sample_path(
    s: &nord_format::cbin::Cbin<nsmp::SampleV3>,
    path: &str,
) -> Result<Option<String>, String> {
    let zones = || s.zones().map_err(|e| e.to_string());
    let sty = || s.sty().map_err(|e| e.to_string());
    let spelled = match path {
        "velocity_windows" => {
            let windows = zones()?
                .iter()
                .map(|z| {
                    z.velocity
                        .map(|w| (w.low, w.high))
                        .ok_or("this zone layout stores no velocity window")
                })
                .collect::<Result<Vec<_>, _>>()?;
            format!("{windows:?}")
        }
        "rel_strengths" => {
            let strengths = zones()?
                .iter()
                .map(|z| {
                    z.rel_strength
                        .ok_or("this zone layout stores no relative strength")
                })
                .collect::<Result<Vec<_>, _>>()?;
            format!("{strengths:?}")
        }
        "sty.dynamics_enabled" => match sty()? {
            Sty::V3(p) => p.dynamics_enabled().to_string(),
            Sty::V4(p) => p.dynamics_enabled().to_string(),
            Sty::V2(_) => return Err("a wide chain read a narrow preset".into()),
        },
        "sty.dynamics_curve" => match sty()? {
            Sty::V3(p) => p.dynamics_curve().to_string(),
            Sty::V4(p) => format!("{:?}", p.dynamics_curve()),
            Sty::V2(_) => return Err("a wide chain read a narrow preset".into()),
        },
        "sty.dynamics_response" => match sty()? {
            Sty::V3(p) => p.dynamics_response().to_string(),
            Sty::V4(p) => format!("{:?}", p.dynamics_response()),
            Sty::V2(_) => return Err("a wide chain read a narrow preset".into()),
        },
        "sty.eq" => match sty()? {
            Sty::V4(p) => {
                let bands: Vec<_> = p.eq().iter().map(|b| (b.frequency, b.gain, b.q)).collect();
                format!("{bands:?}")
            }
            _ => return Err("only a v4 preset stores an EQ".into()),
        },
        _ => return Ok(None),
    };
    Ok(Some(spelled))
}
