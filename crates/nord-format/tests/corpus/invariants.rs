//! Claims every specimen of a kind must satisfy, whatever tree it comes from. Each
//! runs as one trial per specimen of its kind, so a tree holding no specimen of a
//! kind runs none of its claims. They hold for files an instrument or editor
//! wrote, not for the fixtures' zero bodies, so they run on the corpus only.

use crate::format_table;
use crate::oracle::KEY_MAP_OUTSIDE_PARTNER_LAW;
use crate::samples::{self, audio, edited, moved, planned_key_map};
use crate::sidecar;
use crate::Context;
use nord_format::formats::{npno, nsmp, nsmpproj};
use nord_format::{Entity, Live, OrganPreset, PianoPreset, Program, Sample, Synth};
use std::io::Cursor;
use std::path::Path;

/// What a specimen is, for choosing its claims. One specimen has several kinds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cbin,
    /// A CBIN format with a fixed body length.
    Tabled,
    Stage4Program,
    /// A Stage 4 program or preset.
    Stage4Panel,
    DrumBank,
    Electro5Program,
    Sample,
    Narrow,
    /// A narrow instrument from before Sample Library 2.0.
    EarlyNarrow,
    Wide,
    /// A wide instrument whose zones can be retuned and remapped.
    EditableWide,
    /// A wide instrument with a v4 preset.
    WideV4Preset,
    /// A wide instrument with a `map` section, unless its sidecar marks the per-key
    /// table as one the partner law does not describe.
    WideMap,
    Project,
    Piano,
}

pub fn kinds(path: &Path, bytes: &[u8], entity: &Entity) -> Vec<Kind> {
    let mut kinds = Vec::new();
    if bytes.starts_with(b"CBIN") {
        kinds.push(Kind::Cbin);
        let tag = &bytes[8..12];
        if format_table::formats()
            .iter()
            .any(|(format, body_len, _)| format.as_bytes() == tag && body_len.is_some())
        {
            kinds.push(Kind::Tabled);
        }
    }
    match entity {
        Entity::Program(Program::Stage4(_)) | Entity::Live(Live::Stage4(_)) => {
            kinds.extend([Kind::Stage4Program, Kind::Stage4Panel]);
        }
        Entity::OrganPreset(OrganPreset::Stage4(_))
        | Entity::PianoPreset(PianoPreset::Stage4(_))
        | Entity::Synth(Synth::Stage4(_)) => kinds.push(Kind::Stage4Panel),
        Entity::Bundle(
            nord_format::Bundle::Drum2Bank(_) | nord_format::Bundle::Drum3KitBank(_),
        ) => {
            kinds.push(Kind::DrumBank);
        }
        Entity::Program(Program::Electro5(_)) => kinds.push(Kind::Electro5Program),
        Entity::Sample(sample) => {
            kinds.push(Kind::Sample);
            match sample {
                Sample::V2(narrow) => {
                    kinds.push(Kind::Narrow);
                    // A chain that does not read lands here too, where the trial names why.
                    if !narrow.chain().is_ok_and(|c| c != nsmp::Chain::Early) {
                        kinds.push(Kind::EarlyNarrow);
                    }
                }
                Sample::V3(wide) => {
                    kinds.push(Kind::Wide);
                    if sample.zones_are_editable() {
                        kinds.push(Kind::EditableWide);
                    }
                    if matches!(wide.sty(), Ok(nsmp::Sty::V4(_))) {
                        kinds.push(Kind::WideV4Preset);
                    }
                    if nsmp::section::find(&wide.body.sections, nsmp::section::MAP4).is_some()
                        && !outside_partner_law(path)
                    {
                        kinds.push(Kind::WideMap);
                    }
                }
            }
        }
        Entity::SampleProject(_) => kinds.push(Kind::Project),
        Entity::Piano(_) => kinds.push(Kind::Piano),
        _ => {}
    }
    kinds
}

/// Whether the specimen's sidecar states [`KEY_MAP_OUTSIDE_PARTNER_LAW`], which its
/// own checker verifies. An unreadable sidecar fails the specimen's sweep trial.
fn outside_partner_law(path: &Path) -> bool {
    sidecar::of(path).is_ok_and(|sidecar| {
        sidecar.is_some_and(|s| s.traits.iter().any(|t| t == KEY_MAP_OUTSIDE_PARTNER_LAW))
    })
}

pub struct Invariant {
    pub name: &'static str,
    pub kind: Kind,
    pub check: fn(&[u8], &Entity) -> Result<(), String>,
}

const fn claim(
    kind: Kind,
    name: &'static str,
    check: fn(&[u8], &Entity) -> Result<(), String>,
) -> Invariant {
    Invariant { name, kind, check }
}

pub const INVARIANTS: &[Invariant] = &[
    claim(Kind::Cbin, "aux word has a documented shape", aux_word),
    claim(Kind::Tabled, "body length is its format's", body_length),
    claim(
        Kind::Stage4Program,
        "body echoes the header version",
        version_echo,
    ),
    claim(
        Kind::Stage4Program,
        "routes a keyboard section",
        routes_a_section,
    ),
    claim(
        Kind::Stage4Panel,
        "selectors stay in panel range",
        stage4_selectors,
    ),
    claim(Kind::DrumBank, "holds fifty named members", drum_bank),
    claim(
        Kind::Electro5Program,
        "drawbars survive a rewrite",
        ne5_drawbars,
    ),
    claim(
        Kind::Sample,
        "renaming to its own name moves no byte",
        rename_to_itself,
    ),
    claim(Kind::Sample, "an overlong name is refused", overlong_name),
    claim(
        Kind::Sample,
        "stream directories name walked landmarks",
        directories,
    ),
    claim(
        Kind::Sample,
        "strokes decode in the terminator's channels",
        strokes_decode,
    ),
    claim(
        Kind::Sample,
        "strokes decode within a shift of statistic B",
        strokes_decode_at_statistic_b,
    ),
    claim(
        Kind::Sample,
        "preset parses under its own schema",
        preset_schema,
    ),
    claim(Kind::Narrow, "zones pair with every stroke", zones_pair),
    claim(
        Kind::Narrow,
        "keyboard map round trips",
        key_table_round_trip,
    ),
    claim(
        Kind::Narrow,
        "setting the keyboard map touches only it",
        key_table_edit,
    ),
    claim(
        Kind::Narrow,
        "a retune moves one root-key byte",
        narrow_retune,
    ),
    claim(
        Kind::EarlyNarrow,
        "decodes its narrower zone table",
        early_chain,
    ),
    claim(Kind::Wide, "names and strokes decode", wide_decodes),
    claim(
        Kind::Wide,
        "meta states the chain ahead of it",
        meta_chain_len,
    ),
    claim(
        Kind::Wide,
        "a rename touches only the name field",
        wide_rename,
    ),
    claim(
        Kind::Wide,
        "a full-length name stops at the sub-name",
        sub_name,
    ),
    claim(
        Kind::WideV4Preset,
        "dynamics response pinned without a curve",
        dynamics,
    ),
    claim(
        Kind::WideMap,
        "the key-map planner reproduces the table",
        key_map_plan,
    ),
    claim(
        Kind::EditableWide,
        "retunes round trip",
        wide_retune_round_trip,
    ),
    claim(
        Kind::EditableWide,
        "a retune moves both root-key copies",
        wide_retune,
    ),
    claim(
        Kind::EditableWide,
        "a remap moves one boundary byte",
        wide_remap,
    ),
    claim(
        Kind::Project,
        "stroke fields move alone",
        project_stroke_fields,
    ),
    claim(
        Kind::Project,
        "velocity defaults move alone",
        project_velocity,
    ),
    claim(Kind::Piano, "rebuilds from its model", piano_rebuild),
    claim(
        Kind::Piano,
        "strokes decode with overlap and length",
        piano_strokes,
    ),
    claim(
        Kind::Piano,
        "recoding leaves the container alone",
        piano_recode,
    ),
    claim(
        Kind::Piano,
        "a second recode is the first",
        piano_fixed_point,
    ),
    claim(
        Kind::Piano,
        "surgery leaves a readable library",
        piano_surgery,
    ),
    claim(
        Kind::Piano,
        "dropping resonance keeps the rest",
        piano_resonance,
    ),
];

fn aux_word(bytes: &[u8], _: &Entity) -> Result<(), String> {
    const BOTH_HALVES: &[&[u8]] = &[b"ns3y", b"nsmp", b"nd2p"];
    let aux = u32::from_le_bytes([bytes[0x10], bytes[0x11], bytes[0x12], bytes[0x13]]);
    ensure!(
        aux == u32::MAX || aux >> 16 == 0 || BOTH_HALVES.contains(&&bytes[8..12]),
        "aux {aux:#010x}"
    );
    Ok(())
}

fn body_length(bytes: &[u8], _: &Entity) -> Result<(), String> {
    let info = nord_format::cbin::inspect(&mut Cursor::new(bytes)).context("inspect")?;
    let tag = String::from_utf8_lossy(&info.header.tag).into_owned();
    let want = format_table::formats()
        .into_iter()
        .find_map(|(format, body_len, _)| body_len.filter(|_| format == tag))
        .ok_or_else(|| format!("{tag:?} has no fixed body length in the format table"))?;
    ensure!(
        info.body_len == want as u64,
        "{tag:?} body is {} bytes, its format's {want}",
        info.body_len
    );
    Ok(())
}

fn stage4_program(
    entity: &Entity,
) -> Result<&nord_format::cbin::Cbin<nord_format::formats::ns4::Program>, String> {
    match entity {
        Entity::Program(Program::Stage4(p)) | Entity::Live(Live::Stage4(p)) => Ok(p),
        _ => Err("not a Stage 4 program".into()),
    }
}

fn version_echo(_: &[u8], entity: &Entity) -> Result<(), String> {
    let p = stage4_program(entity)?;
    ensure!(
        u32::from(p.version_echo) == p.header.version & 0xff,
        "echo {} of header version {}",
        p.version_echo,
        p.header.version
    );
    Ok(())
}

fn routes_a_section(_: &[u8], entity: &Entity) -> Result<(), String> {
    let p = stage4_program(entity)?;
    ensure!(
        p.organ_section_enabled || p.piano_section_enabled || p.synth_section_enabled,
        "no section is routed to the keyboard"
    );
    Ok(())
}

/// A field and the octave shift it holds.
type Octaves = (&'static str, i8);

/// A field, the value it holds, and the highest stored value the panel reaches.
type Selector = (&'static str, u16, u16);

/// Each selector holds a value the panel can select, and octave shifts stay within
/// two octaves.
fn stage4_selectors(_: &[u8], entity: &Entity) -> Result<(), String> {
    let (octaves, selectors): (Vec<Octaves>, Vec<Selector>) = match entity {
        Entity::Program(Program::Stage4(p)) | Entity::Live(Live::Stage4(p)) => (
            vec![("organ_a.octave_shift", p.organ_a.octave_shift.octaves())],
            vec![
                ("organ_a.model", p.organ_a.model.raw(), 5),
                ("organ_b.model", p.organ_b.model.raw(), 5),
                ("piano_a.piano_type", p.piano_a.piano_type.raw(), 5),
                ("piano_b.piano_type", p.piano_b.piano_type.raw(), 5),
                (
                    "synth_a_voice.filter_type",
                    p.synth_a_voice.filter_type.raw(),
                    5,
                ),
                (
                    "synth_a_voice.lfo_shape",
                    p.synth_a_voice.lfo_shape.raw(),
                    4,
                ),
                (
                    "synth_a_performance.voice_priority",
                    p.synth_a_performance.voice_priority.raw(),
                    2,
                ),
                ("organ_fx.reverb_type", p.organ_fx.reverb_type.raw(), 11),
            ],
        ),
        Entity::OrganPreset(OrganPreset::Stage4(p)) => (
            vec![("organ_a.octave_shift", p.organ_a.octave_shift.octaves())],
            vec![
                ("organ_a.model", p.organ_a.model.raw(), 5),
                ("organ_b.model", p.organ_b.model.raw(), 5),
                ("organ_fx.reverb_type", p.organ_fx.reverb_type.raw(), 11),
            ],
        ),
        Entity::PianoPreset(PianoPreset::Stage4(p)) => (
            vec![("piano_a.octave_shift", p.piano_a.octave_shift.octaves())],
            vec![
                ("piano_a.piano_type", p.piano_a.piano_type.raw(), 5),
                ("piano_b.piano_type", p.piano_b.piano_type.raw(), 5),
                ("piano_a_fx.reverb_type", p.piano_a_fx.reverb_type.raw(), 11),
            ],
        ),
        Entity::Synth(Synth::Stage4(p)) => (
            vec![(
                "synth_a_performance.octave_shift",
                p.synth_a_performance.octave_shift.octaves(),
            )],
            vec![
                (
                    "synth_a_voice.filter_type",
                    p.synth_a_voice.filter_type.raw(),
                    5,
                ),
                (
                    "synth_b_voice.filter_type",
                    p.synth_b_voice.filter_type.raw(),
                    5,
                ),
                (
                    "synth_a_voice.lfo_shape",
                    p.synth_a_voice.lfo_shape.raw(),
                    4,
                ),
                (
                    "synth_a_performance.voice_priority",
                    p.synth_a_performance.voice_priority.raw(),
                    2,
                ),
                ("synth_a_fx.reverb_type", p.synth_a_fx.reverb_type.raw(), 11),
            ],
        ),
        _ => return Err("not a Stage 4 program or preset".into()),
    };
    let wrong: Vec<String> = octaves
        .into_iter()
        .filter(|(_, shift)| !(-2..=2).contains(shift))
        .map(|(field, shift)| format!("{field} = {shift}, outside two octaves"))
        .chain(
            selectors
                .into_iter()
                .filter(|(_, value, top)| value > top)
                .map(|(field, value, top)| {
                    format!("{field} = {value}, and the panel stops at {top}")
                }),
        )
        .collect();
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    Ok(())
}

fn drum_bank(_: &[u8], entity: &Entity) -> Result<(), String> {
    use nord_format::Bundle;
    let names: Vec<&str> = match entity {
        Entity::Bundle(Bundle::Drum2Bank(bank)) => bank
            .programs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect(),
        Entity::Bundle(Bundle::Drum3KitBank(bank)) => {
            bank.kits.iter().map(|(name, _)| name.as_str()).collect()
        }
        _ => return Err("not a drum bank".into()),
    };
    ensure!(names.len() == 50, "{} members", names.len());
    ensure!(
        names.iter().all(|name| !name.is_empty()),
        "an unnamed member"
    );
    Ok(())
}

/// Setting each organ model's drawbars to the values just read leaves every byte
/// unchanged.
fn ne5_drawbars(bytes: &[u8], _: &Entity) -> Result<(), String> {
    use nord_format::formats::ne5::OrganModel::{Farfisa, Pipe, Vox, B3};
    use nord_format::formats::ne5::Preset;
    let Entity::Program(Program::Electro5(mut program)) = samples::parse(bytes)? else {
        return Err("not an Electro 5 program".into());
    };
    for model in [B3, Vox, Farfisa, Pipe] {
        for preset in [Preset::One, Preset::Two] {
            let bars = program.organ_panel.drawbars(model, preset);
            if bars.iter().all(|&bar| bar <= 8) {
                program
                    .organ_panel
                    .set_drawbars(model, preset, bars)
                    .context(format!("{model:?} preset {preset:?}"))?;
            }
        }
    }
    let mut rewritten = Vec::new();
    program
        .write_to(&mut Cursor::new(&mut rewritten))
        .context("write")?;
    ensure!(
        rewritten == bytes,
        "differs at {:#x?}",
        moved(bytes, &rewritten)
    );
    Ok(())
}

/// The read stops at the terminator within the generation's name span, and the write
/// covers that span, so writing the name back changes nothing.
fn rename_to_itself(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let name = samples::sample(entity)?.name().context("name")?;
    if name.is_empty() {
        // The oldest narrow `hdr` is 18 bytes and has no name field.
        return Ok(());
    }
    let after = edited(bytes, |s| s.set_name(&name).context("rename"))?;
    ensure!(after == bytes, "renaming to {name:?} changed a byte");
    Ok(())
}

fn overlong_name(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::sample(entity)?;
    let name = sample.name().context("name")?;
    let over = "M".repeat(sample.max_name_len() + 1);
    let after = edited(bytes, |s| {
        ensure!(s.set_name(&over).is_err(), "an overlong name was accepted");
        Ok(())
    })?;
    ensure!(
        after == bytes,
        "the refused rename moved {:#x?}",
        moved(bytes, &after)
    );
    ensure!(
        samples::sample(&samples::parse(&after)?)?
            .name()
            .context("name")?
            == name,
        "the refused rename changed the name"
    );
    Ok(())
}

fn directories(_: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::sample(entity)?;
    let layout = sample.layout().context("layout")?;
    for (index, (at, stroke)) in sample.stroke_streams().into_iter().enumerate() {
        let stream = nsmp::codec::walk(stroke, at, layout).context(format!("stroke {index}"))?;
        let directory = nsmp::codec::Directory::read(stroke)
            .ok_or_else(|| format!("stroke {index}: no directory"))?;
        let resolve = |pointer| nsmp::codec::Directory::resolve(pointer, at, layout);
        let words = (stroke.len() - layout.header_len()) / layout.word();
        ensure!(
            resolve(directory.first_record) == stream.first_record,
            "stroke {index}: first record"
        );
        ensure!(
            nsmp::codec::Directory::resolve_end(directory.terminator, at, layout, words)
                == stream.terminator,
            "stroke {index}: terminator"
        );
        let names =
            |pointer, word| word % nsmp::codec::WRAP == resolve(pointer) % nsmp::codec::WRAP;
        let boundary = |pointer| {
            names(pointer, stream.terminator)
                || stream
                    .records
                    .iter()
                    .any(|record| names(pointer, record.at))
        };
        ensure!(
            boundary(directory.resync),
            "stroke {index}: resync is not a record boundary"
        );
        ensure!(
            boundary(directory.mark),
            "stroke {index}: mark is not a record or terminator"
        );
        let marked: Vec<_> = stream.records.iter().filter(|record| record.mark).collect();
        ensure!(
            marked.is_empty() || (marked.len() == 1 && names(directory.mark, marked[0].at)),
            "stroke {index}: marked record disagrees with the directory"
        );
        ensure!(!stream.records.is_empty(), "stroke {index}: no record");
    }
    Ok(())
}

fn strokes_decode(_: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::sample(entity)?;
    let layout = sample.layout().context("layout")?;
    let file_peak = sample.file_peak().context("file peak")?;
    for (index, (at, stroke)) in sample.stroke_streams().into_iter().enumerate() {
        let stream = nsmp::codec::walk(stroke, at, layout).context(format!("stroke {index}"))?;
        let audio = nsmp::codec::decode(stroke, at, layout, file_peak)
            .context(format!("stroke {index}"))?;
        let channels = if stream.cell == Some(2 * layout.cell()) {
            2
        } else {
            1
        };
        ensure!(
            usize::from(audio.channels) == channels,
            "stroke {index}: {} channels decoded, the terminator states {channels}",
            audio.channels
        );
        let fields: usize = stream
            .records
            .iter()
            .map(|record| record.values.len())
            .sum();
        ensure!(
            audio.samples.len() == fields && fields > 0,
            "stroke {index}: {} samples decoded from {fields} fields",
            audio.samples.len()
        );
    }
    Ok(())
}

/// The v4 `sty` payload comes in two widths under one section version, so each
/// specimen's schema is chosen by its length.
fn preset_schema(_: &[u8], entity: &Entity) -> Result<(), String> {
    match samples::sample(entity)? {
        Sample::V2(sample) => {
            let sty = sample.sty().context("sty")?;
            ensure!(
                sty.raw.len() == nsmp::sty::V2_LEN,
                "v2 sty of {} bytes",
                sty.raw.len()
            );
        }
        Sample::V3(sample) => match sample.sty().context("sty")? {
            nsmp::Sty::V3(block) => {
                ensure!(
                    block.raw.len() == nsmp::sty::V3_LEN,
                    "v3 sty of {} bytes",
                    block.raw.len()
                );
            }
            nsmp::Sty::V4(block) => {
                ensure!(
                    [nsmp::sty::V4_LEN, nsmp::sty::V4_LEN_LONG].contains(&block.raw.len()),
                    "v4 sty of {} bytes",
                    block.raw.len()
                );
                for band in block.eq() {
                    ensure!(band.frequency <= 20_000, "EQ band at {} Hz", band.frequency);
                }
            }
        },
    }
    Ok(())
}

/// A zone record names its stroke in one byte, but a stroke's id is a u32, so
/// pairing on the whole id would lose the zones of strokes past 255.
fn zones_pair(_: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::narrow(entity)?;
    let zones = sample.zones().context("zones")?.len();
    let strokes = sample.strokes().context("strokes")?.len();
    ensure!(zones == strokes, "{zones} zones for {strokes} strokes");
    Ok(())
}

fn key_table_round_trip(_: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::narrow(entity)?;
    let payload = map_payload(sample)?;
    let mut copy = payload.to_vec();
    sample
        .key_table()
        .context("keyboard map")?
        .write(&mut copy)
        .context("write")?;
    ensure!(copy == payload, "differs at {:#x?}", moved(payload, &copy));
    Ok(())
}

/// Setting the neutral keyboard map reads back neutral and leaves every section but
/// `map` alone.
fn key_table_edit(bytes: &[u8], _: &Entity) -> Result<(), String> {
    let Entity::Sample(Sample::V2(mut sample)) = samples::parse(bytes)? else {
        return Err("not a narrow instrument".into());
    };
    let before = sample.body.sections.clone();
    sample
        .set_key_table(&nsmp::KeyTable::NEUTRAL)
        .context("set")?;
    let reread = nsmp::from_bytes(&sample.to_bytes().context("write")?).context("read back")?;
    ensure!(
        reread.key_table().context("keyboard map")? == nsmp::KeyTable::NEUTRAL,
        "the neutral map did not read back"
    );
    ensure!(
        before.len() == reread.body.sections.len(),
        "the section count changed"
    );
    for (was, now) in before.iter().zip(&reread.body.sections) {
        ensure!(
            was.is(nsmp::section::MAP) || was == now,
            "the {} section moved",
            was.tag_str()
        );
    }
    Ok(())
}

/// Retuning zone 0 changes the root-key byte of the stroke it plays and nothing else
/// in the body.
fn narrow_retune(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::narrow(entity)?;
    let zones = samples::sample(entity)?.zones().context("zones")?;
    let (was, at, len) = (zones[0].root_key, zones[0].at, zones[0].stream.len());
    let note = other_than(was, 48);
    let after = edited(bytes, |s| s.set_root_key(0, note).context("retune"))?;
    let changed = moved(bytes, &after);
    let stroke = sample.header.generation.body_start() as usize + at;
    ensure!(
        changed.len() == 1 && (stroke..stroke + len).contains(&changed[0]),
        "moved {changed:#x?}, not one byte of zone 0's stroke at {stroke:#x}"
    );
    ensure!(
        bytes[changed[0]] == was && after[changed[0]] == note,
        "the byte held {} and now holds {}",
        bytes[changed[0]],
        after[changed[0]]
    );
    Ok(())
}

/// Libraries before Sample Library 2.0 write a narrower chain with its own `map`
/// version: no `cat`, an 18-byte `hdr` with no name field, and a zone record three
/// bytes shorter.
fn early_chain(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::narrow(entity)?;
    let chain = sample.chain().context("chain")?;
    ensure!(
        !chain.names_instrument() && sample.name().context("name")?.is_empty(),
        "this chain has no name field"
    );
    ensure!(
        !sample.name_is_editable(),
        "an instrument with no name field reports its name editable"
    );
    let mut copy = nsmp::from_bytes(bytes).context("read")?;
    ensure!(
        copy.set_name("Renamed").is_err(),
        "renamed an instrument with no name field"
    );
    ensure!(
        sample.categories().is_empty(),
        "this chain has no cat section"
    );

    let zones = sample.zones().context("zones")?;
    let strokes = sample.strokes().context("strokes")?;
    ensure!(
        zones.len() == strokes.len(),
        "{} zones for {} strokes",
        zones.len(),
        strokes.len()
    );
    ensure!(
        zones.len() * chain.zone_record_len() + nsmp::zone::RECORDS_AT
            == map_payload(sample)?.len(),
        "the map payload length does not match its zone count"
    );
    ensure!(
        zones
            .windows(2)
            .all(|pair| pair[0].top_note > pair[1].top_note),
        "zones are not stored high to low"
    );
    for (zone, stroke) in zones.iter().zip(&strokes) {
        ensure!(
            zone.top_note <= 127 && stroke.root_key <= 127,
            "a note past 127"
        );
        ensure!(
            stroke.packets.is_some(),
            "stroke length is not this chain's header plus whole packets"
        );
        ensure!(
            zone.rel_strength == nsmp::zone::REL_STRENGTH_DEFAULT,
            "relative strength {}",
            zone.rel_strength
        );
    }
    sample.sty().context("sty")?;
    sample.key_table().context("keyboard map")?;
    Ok(())
}

fn wide_decodes(_: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::wide(entity)?;
    ensure!(!sample.name().context("name")?.is_empty(), "empty name");
    ensure!(sample.stroke_count() > 0, "no strokes");
    // Unexplained: some vendor zone maps do not have one entry per stroke.
    let Ok(zones) = sample.zones() else {
        return Ok(());
    };
    ensure!(
        zones.len() == sample.stroke_count(),
        "{} zones for {} strokes",
        zones.len(),
        sample.stroke_count()
    );
    for zone in zones {
        ensure!(
            zone.top_note <= 127 && zone.root_key <= 127,
            "a note past 127"
        );
        if let Some(low) = zone.low_note {
            ensure!(
                low <= zone.top_note,
                "low note {low} above top {}",
                zone.top_note
            );
        }
    }
    Ok(())
}

/// A writer that resizes a section must update this field.
fn meta_chain_len(_: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::wide(entity)?;
    let meta = sample.meta().context("meta")?;
    ensure!(
        meta.chain_len as usize == sample.chain_len_before_meta(),
        "meta says {}, the chain ahead of it is {}",
        meta.chain_len,
        sample.chain_len_before_meta()
    );
    Ok(())
}

/// Writing one name over another changes at most as many bytes as the longer name,
/// all within the field, and no audio.
fn wide_rename(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    const NEW: &str = "Retitled";
    let old = samples::sample(entity)?.name().context("name")?;
    let after = edited(bytes, |s| s.set_name(NEW).context("rename"))?;
    let moved = moved(bytes, &after);
    if old == NEW {
        return Ok(());
    }
    ensure!(!moved.is_empty(), "the rename moved nothing");
    ensure!(
        moved.len() <= old.len().max(NEW.len())
            && moved[moved.len() - 1] - moved[0] < nsmp::MAX_NAME_V3_LEN,
        "the rename over a {}-byte name moved {moved:#x?}",
        old.len()
    );
    let reread = samples::parse(&after)?;
    ensure!(
        samples::sample(&reread)?.name().context("name")? == NEW,
        "the name read back"
    );
    ensure!(
        audio(samples::sample(entity)?)? == audio(samples::sample(&reread)?)?,
        "the rename moved audio"
    );
    Ok(())
}

fn sub_name(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let was = samples::wide(entity)?.sub_name().context("sub-name")?;
    let long = "x".repeat(nsmp::MAX_NAME_V3_LEN);
    let after = edited(bytes, |s| s.set_name(&long).context("rename"))?;
    let reread = samples::parse(&after)?;
    let sample = samples::wide(&reread)?;
    ensure!(
        sample.name().context("name")? == long,
        "the full-length name read back"
    );
    ensure!(
        sample.sub_name().context("sub-name")? == was,
        "the sub-name {was:?} became {:?}",
        sample.sub_name()
    );
    edited(bytes, |s| {
        ensure!(
            s.set_name(&format!("{long}x")).is_err(),
            "a name past the field was accepted"
        );
        Ok(())
    })?;
    Ok(())
}

/// A v4 `sty` dynamics-response triple is all 127 exactly when no curve is selected.
///
/// Inferred from specimens; not confirmed on hardware.
fn dynamics(_: &[u8], entity: &Entity) -> Result<(), String> {
    let nsmp::Sty::V4(sty) = samples::wide(entity)?.sty().context("sty")? else {
        return Err("not a v4 preset".into());
    };
    ensure!(
        sty.dynamics_curve().is_none() == (sty.dynamics_response() == [127; 3]),
        "curve {:?} against response {:?}",
        sty.dynamics_curve(),
        sty.dynamics_response()
    );
    Ok(())
}

/// The Sample Editor writes the neutral table for any zone layout, so the planner
/// leaves it neutral; a populated table is the one the planner derives. An instrument
/// whose table plans is one whose zones can be edited.
fn key_map_plan(_: &[u8], entity: &Entity) -> Result<(), String> {
    let wide = samples::wide(entity)?;
    match planned_key_map(wide)? {
        None => {}
        Some(plan) if plan.kind == nsmp::zone::KeyMap::Neutral => {
            ensure!(!plan.writes, "the planner wrote into a neutral table");
        }
        Some(plan) => ensure!(
            plan.planned == plan.stored,
            "the planned key map differs at {:#x?}",
            moved(&plan.stored, &plan.planned)
        ),
    }
    ensure!(
        wide.zones_are_editable(),
        "the zones are not editable: {:?}",
        wide.zones().err()
    );
    Ok(())
}

fn wide_retune_round_trip(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let was: Vec<u8> = samples::sample(entity)?
        .zones()
        .context("zones")?
        .iter()
        .map(|z| z.root_key)
        .collect();
    let after = edited(bytes, |s| {
        for (i, root) in was.iter().enumerate() {
            s.set_root_key(i, root ^ 1).context(format!("zone {i}"))?;
        }
        for (i, root) in was.iter().enumerate() {
            s.set_root_key(i, *root).context(format!("zone {i}"))?;
        }
        Ok(())
    })?;
    ensure!(after == bytes, "differs at {:#x?}", moved(bytes, &after));
    Ok(())
}

/// Whether the instrument's per-key table is populated, so a zone edit recomputes
/// it along with the zone.
fn populated(entity: &Entity) -> Result<bool, String> {
    Ok(planned_key_map(samples::wide(entity)?)?
        .is_some_and(|plan| plan.kind == nsmp::zone::KeyMap::Populated))
}

/// The per-key gains and the three bytes after each, an authored curve that no
/// layout predicts, so a recomputed table must keep them.
fn key_levels(entity: &Entity) -> Result<Vec<Vec<u8>>, String> {
    let wide = samples::wide(entity)?;
    let map =
        nsmp::section::find(&wide.body.sections, nsmp::section::MAP4).ok_or("no map section")?;
    Ok((0..128)
        .map(|k| map.payload[6 + k * 10..][..6].to_vec())
        .collect())
}

/// An edit of zone 0 moves `count` bytes, or, where the per-key table is populated
/// and moves with the zone, keeps the table's levels.
fn zone_edit_moved(
    before: &[u8],
    entity: &Entity,
    after: &[u8],
    count: usize,
) -> Result<(), String> {
    if populated(entity)? {
        let levels = key_levels(&samples::parse(after)?)?;
        ensure!(key_levels(entity)? == levels, "the per-key levels moved");
        return Ok(());
    }
    let moved = moved(before, after);
    ensure!(
        moved.len() == count,
        "moved {moved:#x?}, not {count} byte(s)"
    );
    Ok(())
}

/// Retuning zone 0 moves the root key in its stroke header and its zone record.
fn wide_retune(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::sample(entity)?;
    let was = sample.zones().context("zones")?[0].root_key;
    let note = other_than(was, 48);
    let after = edited(bytes, |s| s.set_root_key(0, note).context("retune"))?;
    let reread = samples::parse(&after)?;
    ensure!(
        samples::sample(&reread)?.zones().context("zones")?[0].root_key == note,
        "the root key read back"
    );
    ensure!(
        audio(sample)? == audio(samples::sample(&reread)?)?,
        "the retune moved audio"
    );
    zone_edit_moved(bytes, entity, &after, 2)
}

/// Moving zone 0's upper key moves one byte. Its lowest note moves one byte where
/// the layout stores one; elsewhere the setter refuses.
fn wide_remap(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::sample(entity)?;
    let zone = &sample.zones().context("zones")?[0];
    let (root, top, low) = (zone.root_key, zone.top_note, zone.low_note);
    let want = other_than(top, 96);
    let after = edited(bytes, |s| s.set_zone_top_note(0, want).context("remap"))?;
    zone_edit_moved(bytes, entity, &after, 1).context("upper key")?;
    let reread = samples::parse(&after)?;
    let zone = &samples::sample(&reread)?.zones().context("zones")?[0];
    ensure!(
        (zone.root_key, zone.top_note, zone.low_note) == (root, want, low),
        "zone 0 read back as {:?}",
        (zone.root_key, zone.top_note, zone.low_note)
    );
    ensure!(
        audio(sample)? == audio(samples::sample(&reread)?)?,
        "the remap moved audio"
    );
    let Some(low) = low else {
        let after = edited(bytes, |s| {
            ensure!(
                s.set_zone_low_note(0, 40).is_err(),
                "set a low note the layout does not store"
            );
            Ok(())
        })?;
        ensure!(
            after == bytes,
            "the refused low note moved {:#x?}",
            moved(bytes, &after)
        );
        return Ok(());
    };
    let want = other_than(low, 40);
    let after = edited(bytes, |s| s.set_zone_low_note(0, want).context("low note"))?;
    zone_edit_moved(bytes, entity, &after, 1).context("low note")?;
    ensure!(
        samples::sample(&samples::parse(&after)?)?
            .zones()
            .context("zones")?[0]
            .low_note
            == Some(want),
        "the low note read back"
    );
    Ok(())
}

/// Lines of `before` and `after` that differ, when both have the same line count.
fn changed_lines(before: &str, after: &str) -> Result<usize, String> {
    ensure!(
        before.lines().count() == after.lines().count(),
        "{} lines became {}",
        before.lines().count(),
        after.lines().count()
    );
    Ok(before
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .count())
}

fn project_stroke_fields(_: &[u8], entity: &Entity) -> Result<(), String> {
    use nsmpproj::StrokeField as F;
    let project = samples::project(entity)?;
    let before = project.render();
    // A probe must differ from what the stroke holds, or nothing changes.
    let windows: std::collections::BTreeMap<u32, (u8, u8)> = project
        .zones()
        .context("zones")?
        .iter()
        .flat_map(|z| z.strokes.iter().map(|s| (s.global_id, s.velocity)))
        .collect();
    let elsewhere = |held: u8, probe: u8| if held == probe { probe ^ 1 } else { probe };
    for stroke in project.strokes().context("strokes")? {
        let (vmin, vmax) = windows.get(&stroke.global_id).copied().unwrap_or((0, 127));
        let fields = [
            ("start", F::Start(3.0)),
            ("stop", F::Stop(4000.0)),
            ("gain", F::Gain(0.75)),
            ("velocity_min", F::VelocityMin(elsewhere(vmin, 10))),
            ("velocity_max", F::VelocityMax(elsewhere(vmax, 100))),
            ("loop_enabled", F::LoopEnabled(!stroke.loop_enabled)),
            ("loop_start", F::LoopStart(1234.5)),
            ("loop_length", F::LoopLength(600.0)),
            ("loop_crossfade", F::LoopCrossfade(90.0)),
            ("loop_crossfade_mode", F::LoopCrossfadeMode(1)),
            (
                "loop_decay_enabled",
                F::LoopDecayEnabled(!stroke.loop_decay_enabled),
            ),
            ("loop_decay", F::LoopDecay(3.25)),
            ("loop_detune", F::LoopDetune(-12)),
            (
                "short_loop_enabled",
                F::ShortLoopEnabled(!stroke.short_loop_enabled),
            ),
            ("short_loop_length", F::ShortLoopLength(64.0)),
            ("short_loop_crossfade", F::ShortLoopCrossfade(5)),
            (
                "short_loop_uses_pitch",
                F::ShortLoopUsesPitch(!stroke.short_loop_uses_pitch),
            ),
        ];
        for (name, field) in fields {
            let mut edited = project.clone();
            edited
                .set_stroke_field(stroke.global_id, field)
                .context(format!("stroke {} {name}", stroke.global_id))?;
            let changed = changed_lines(&before, &edited.render())
                .context(format!("stroke {} {name}", stroke.global_id))?;
            ensure!(
                changed == 1,
                "stroke {} {name} changed {changed} lines",
                stroke.global_id
            );
        }
    }
    Ok(())
}

fn project_velocity(_: &[u8], entity: &Entity) -> Result<(), String> {
    let project = samples::project(entity)?;
    let was = project.velocity_defaults().context("velocity defaults")?;
    let defaults = nsmpproj::VelocityDefaults {
        attack_amount: 64,
        amplitude: 0,
        timbre: 0,
    };
    let mut edited = project.clone();
    edited.set_velocity_defaults(defaults).context("set")?;
    ensure!(
        edited.velocity_defaults().context("read back")? == defaults,
        "the velocity defaults read back"
    );
    let asked = [
        was.attack_amount != defaults.attack_amount,
        was.amplitude != defaults.amplitude,
        was.timbre != defaults.timbre,
    ]
    .into_iter()
    .filter(|moved| *moved)
    .count();
    let changed = changed_lines(&project.render(), &edited.render())?;
    ensure!(
        changed == asked,
        "{changed} lines changed for {asked} values"
    );
    Ok(())
}

fn library(entity: &Entity) -> Result<npno::Library<'_>, String> {
    match entity {
        Entity::Piano(piano) => piano.library().context("parse"),
        _ => Err("not a piano library".into()),
    }
}

/// The writer recomputes the per-root counts, every audio offset, the alignment gap,
/// and the container checksum from the model, so a byte-exact rebuild shows those are
/// derived correctly and nothing else was lost.
fn piano_rebuild(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let rebuilt = library(entity)?
        .to_piano()
        .and_then(|p| nord_format::to_bytes(&Entity::Piano(p)))
        .context("rebuild")?;
    ensure!(
        rebuilt == bytes,
        "the rebuild differs at {}",
        crate::oracle::first_difference(&rebuilt, bytes)
    );
    Ok(())
}

/// Each block repeats the previous block's last frames bit for bit, and the blocks'
/// own frames add up to the count the record states, so a decoder that drifts fails
/// here instead of producing noise.
fn piano_strokes(_: &[u8], entity: &Entity) -> Result<(), String> {
    let library = library(entity)?;
    for stroke in library.strokes() {
        let audio =
            npno::codec::decode(stroke, library.channels()).context(format!("{stroke:?}"))?;
        ensure!(
            audio.frames() == stroke.frames() as usize,
            "{stroke:?}: {} frames decoded",
            audio.frames()
        );
        ensure!(
            audio.clipped == 0,
            "{stroke:?} decoded outside the int16 range"
        );
    }
    Ok(())
}

/// Recoding a library from its own audio keeps its size, prefix and directory records,
/// and each block in its place. Only a block's declared attenuation may change, so the
/// written file plays the same audio as the file that was read.
fn piano_recode(_: &[u8], entity: &Entity) -> Result<(), String> {
    let library = library(entity)?;
    let before = library.to_body().context("body")?;
    let again = npno::encode::rebuild(&library).context("recode")?;
    let after = again.library.to_body().context("body")?;
    ensure!(
        after.len() == before.len(),
        "the recode is a different size"
    );
    let audio: usize = again
        .library
        .strokes()
        .iter()
        .map(|s| s.audio().len())
        .sum();
    let head = before.len() - audio;
    ensure!(
        after[..head] == before[..head],
        "the recode changed a byte outside the audio"
    );
    for (stroke, recoded) in library.strokes().iter().zip(&again.strokes) {
        ensure!(
            recoded.blocks == usize::from(stroke.blocks())
                && recoded.identical + recoded.restated == recoded.blocks,
            "{stroke:?} did not come back block for block"
        );
    }
    Ok(())
}

/// A stroke states the frames its blocks own, so a second search has the same room
/// and chooses the same widths.
fn piano_fixed_point(_: &[u8], entity: &Entity) -> Result<(), String> {
    let once = npno::encode::rebuild(&library(entity)?).context("recode")?;
    let twice = npno::encode::rebuild(&once.library).context("recode again")?;
    let (before, after) = (
        once.library.to_body().context("body")?,
        twice.library.to_body().context("body")?,
    );
    ensure!(
        before == after,
        "the second recode differs at {}",
        crate::oracle::first_difference(&after, &before)
    );
    Ok(())
}

/// Every transform leaves spans that tile the body, counts that sum to the directory,
/// and every covered key naming a root that still has strokes. The remaining strokes
/// keep their audio byte for byte, because a transform moves spans and never
/// re-encodes one.
fn piano_surgery(_: &[u8], entity: &Entity) -> Result<(), String> {
    let source = library(entity)?;
    let covered: Vec<u8> = (0..128)
        .filter(|&k| source.key_map()[k as usize] != npno::UNCOVERED)
        .collect();
    let middle = covered[covered.len() / 2];
    let mut cases: Vec<(String, npno::Library<'_>)> = Vec::new();
    for bank in npno::Bank::ALL {
        let mut cut = source.clone();
        cut.drop_bank(bank);
        cases.push((format!("without the {bank} bank"), cut));
    }
    let mut loudest = source.clone();
    loudest.keep_layers(&npno::Layers::Loudest(2));
    cases.push(("two loudest layers".into(), loudest));
    let mut narrowed = source.clone();
    narrowed
        .cut_range(middle..=*covered.last().unwrap())
        .context("cut")?;
    cases.push(("upper half of the keyboard".into(), narrowed));
    let (low, high) = source.split_at(middle).context("split")?;
    cases.push(("split low".into(), low));
    cases.push(("split high".into(), high));

    for (label, cut) in cases {
        let rebuilt = cut.to_piano().context(&label)?;
        let back = rebuilt
            .library()
            .context(format!("{label}: reading it back"))?;
        ensure!(
            back.strokes().len() == cut.strokes().len(),
            "{label}: stroke count"
        );
        ensure!(back.channels() == source.channels(), "{label}: channels");
        for (before, after) in cut.strokes().iter().zip(back.strokes()) {
            ensure!(
                before.root == after.root && before.id() == after.id(),
                "{label}: {before:?} came back as {after:?}"
            );
            ensure!(
                before.audio() == after.audio(),
                "{label}: {before:?} audio was re-encoded"
            );
        }
        for (key, &root) in back.key_map().iter().enumerate() {
            ensure!(
                root == npno::UNCOVERED || back.roots().contains(&root),
                "{label}: key {key} plays root {root}, which has no stroke"
            );
        }
    }
    Ok(())
}

/// Only the dropped strokes go, and only the survivors' audio offsets change.
fn piano_resonance(_: &[u8], entity: &Entity) -> Result<(), String> {
    let source = library(entity)?;
    let expected: Vec<_> = source
        .strokes()
        .iter()
        .filter(|s| s.bank() != Some(npno::Bank::Resonance))
        .collect();
    let mut cut = source.clone();
    let change = cut.drop_bank(npno::Bank::Resonance);
    ensure!(
        change.strokes_removed == source.strokes().len() - expected.len(),
        "{} strokes removed",
        change.strokes_removed
    );
    for (before, after) in expected.iter().zip(cut.strokes()) {
        // The record's first four bytes are the audio offset.
        ensure!(
            before.record()[4..] == after.record()[4..],
            "{before:?} came back as {after:?}"
        );
    }
    Ok(())
}

/// A note other than `held`, near `wanted`.
fn other_than(held: u8, wanted: u8) -> u8 {
    if held == wanted {
        wanted + 1
    } else {
        wanted
    }
}

fn map_payload(sample: &nord_format::cbin::Cbin<nsmp::Sample>) -> Result<&[u8], String> {
    nsmp::section::find(&sample.body.sections, nsmp::section::MAP)
        .map(|section| section.payload.as_slice())
        .ok_or_else(|| "no map section".to_string())
}

/// Statistic B is a stroke's content peak at a shift of two, so a stroke decoded at the
/// right shift peaks near four times it, and a misread shift lands a power of two away.
/// Clipped strokes, and vendor strokes whose streams peak off their statistic B, stay
/// within one shift of it. Below a statistic B of four, its own floor is coarser.
///
/// Inferred from specimens; not confirmed on hardware.
fn strokes_decode_at_statistic_b(_: &[u8], entity: &Entity) -> Result<(), String> {
    let sample = samples::sample(entity)?;
    let layout = sample.layout().context("layout")?;
    let file_peak = sample.file_peak().context("file peak")?;
    let streams = sample.stroke_streams();
    for (index, &(at, stroke)) in streams.iter().enumerate() {
        let statistic_b = nsmp::codec::peak(stroke, layout)
            .ok_or(format!("stroke {index}: no statistic B"))?
            .unsigned_abs();
        if statistic_b < 4 {
            continue;
        }
        let audio = nsmp::codec::decode(stroke, at, layout, file_peak)
            .context(format!("stroke {index}"))?;
        let peak = audio
            .samples
            .iter()
            .map(|&v| u32::from(v.unsigned_abs()))
            .max()
            .unwrap_or(0);
        ensure!(
            2 * statistic_b < peak && peak < 8 * statistic_b,
            "stroke {index} of {}: peaks at {peak}, and statistic B {statistic_b} states about \
             {} (shift {:?} against the file peak {file_peak})",
            streams.len(),
            4 * statistic_b,
            nsmp::codec::shift_against(stroke, file_peak),
        );
    }
    Ok(())
}
