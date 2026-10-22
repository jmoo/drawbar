//! Oracle-sidecar checking. `<specimen>.oracle.json` records what a differential
//! capture established about its specimen, in machine-readable form. Adding a
//! sidecar beside a specimen adds its checks to the sweep; no case is written
//! here, and no specimen is named.

use crate::lookup;
use crate::samples::{self, related};
use crate::sidecar::{Expectation, Impulses, Render, Sidecar};
use crate::Context;
use nord_format::formats::ne5::{OrganModel, Preset};
use nord_format::formats::{npno, nsmp};
use nord_format::{Entity, Live, Program, Sample};
use std::fs;
use std::io::Cursor;
use std::path::Path;

/// Check one specimen against its sidecar, if it has one.
pub fn check_specimen(path: &Path, bytes: &[u8], entity: &Entity) -> Result<(), String> {
    let Some(sidecar) = crate::sidecar::of(path)? else {
        return Ok(());
    };
    let specimen = Specimen {
        path,
        bytes,
        entity,
        sidecar: &sidecar,
    };
    let mut wrong: Vec<String> = Vec::new();
    for (key, check) in CLAIMS {
        if let Err(e) = check(&specimen) {
            wrong.push(format!("{key}: {e}"));
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} oracle mismatches:\n  {}",
            wrong.len(),
            wrong.join("\n  ")
        ))
    }
}

/// A file named in a sidecar must be refused by the reader, with an error that
/// contains the sidecar's text.
pub fn check_refusal(specimen: &Path, refusal: &str) -> Result<(), String> {
    let bytes = fs::read(specimen).context("read")?;
    match nord_format::from_stream(&mut Cursor::new(&bytes)) {
        Ok(entity) => Err(format!(
            "read as {}; the sidecar says the reader refuses it",
            entity.identity().kind
        )),
        Err(e) if e.to_string().contains(refusal) => Ok(()),
        Err(e) => Err(format!("refused as {e:?}, which does not say {refusal:?}")),
    }
}

struct Specimen<'a> {
    path: &'a Path,
    bytes: &'a [u8],
    entity: &'a Entity,
    sidecar: &'a Sidecar,
}

type Claim = fn(&Specimen) -> Result<(), String>;

/// One checker per sidecar claim, each a no-op when its key is absent.
const CLAIMS: &[(&str, Claim)] = &[
    ("same_body_as", same_body_as),
    ("fields", fields),
    ("traits", traits),
    ("source", source),
    ("wide_renders", wide_renders),
    ("wide_renders", twin_law),
    ("edited_from", edited_from),
    ("audio_differs_from", audio_differs_from),
    ("render", render),
    ("impulses", impulses),
    ("recordings_from", recordings_from),
];

fn same_body_as(s: &Specimen) -> Result<(), String> {
    let Some(sibling) = &s.sidecar.same_body_as else {
        return Ok(());
    };
    let other = fs::read(s.path.parent().unwrap().join(sibling)).context(sibling)?;
    ensure!(
        other == s.bytes,
        "not byte-identical to {sibling}; replace `same_body_as` with the fields that differ"
    );
    Ok(())
}

fn fields(s: &Specimen) -> Result<(), String> {
    let wrong: Vec<String> = s
        .sidecar
        .fields
        .iter()
        .filter_map(|(path, expected)| match lookup::lookup(s.entity, path) {
            Err(e) => Some(format!("{path}: {e}")),
            Ok(spellings) if matches(expected, &spellings) => None,
            Ok(spellings) => Some(format!(
                "{path}: decodes to {spellings:?}, sidecar says {:?}",
                expected.value
            )),
        })
        .collect();
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    Ok(())
}

/// `+5`, `5`, and ` 5 ` are one value, and case is ignored.
fn normalize(s: &str) -> String {
    s.trim().trim_start_matches('+').to_ascii_lowercase()
}

/// A value as a number, if it is one: `0x…` as hex, anything else as decimal.
fn number(s: &str) -> Option<f64> {
    if let Some(hex) = s.strip_prefix("0x") {
        return u64::from_str_radix(hex, 16).ok().map(|v| v as f64);
    }
    s.parse().ok()
}

/// A sidecar value matches if it equals any of the field's spellings or, when
/// both read as numbers, is within its slack of one. Without a slack, the numbers
/// must be equal.
fn matches(expected: &Expectation, spellings: &[String]) -> bool {
    let want = normalize(&expected.value);
    if spellings.iter().any(|s| normalize(s) == want) {
        return true;
    }
    let Some(a) = number(&want) else {
        return false;
    };
    let tolerance = expected.slack.unwrap_or(0.0);
    spellings
        .iter()
        .filter_map(|s| number(&normalize(s)))
        .any(|b| (a - b).abs() <= tolerance)
}

/// The trait naming a vendor per-key table the partner law does not describe.
pub const KEY_MAP_OUTSIDE_PARTNER_LAW: &str = "key_map_outside_partner_law";

/// The trait vocabulary, one checker per name. A trait this reader does not
/// know is an error, so the vocabulary cannot drift.
fn traits(s: &Specimen) -> Result<(), String> {
    let wrong: Vec<String> = s
        .sidecar
        .traits
        .iter()
        .filter_map(|name| {
            let check: Claim = match name.as_str() {
                "b3_bass_manual" => b3_bass_manual,
                "builds_from_source" => builds_from_source,
                "editor_render" => editor_render,
                KEY_MAP_OUTSIDE_PARTNER_LAW => key_map_outside_partner_law,
                "left_channel_only" => |s| one_channel(s, Channel::Left),
                "right_channel_only" => |s| one_channel(s, Channel::Right),
                "sine_at_root_key" => sine_at_root_key,
                "stroke_ids_past_a_byte" => stroke_ids_past_a_byte,
                "written_by_this_crate" => written_by_this_crate,
                "zone_top_notes_derived" => |s| zone_top_notes(s, true),
                "zone_top_notes_overridden" => |s| zone_top_notes(s, false),
                other => return Some(format!("unknown trait {other:?}")),
            };
            check(s).err().map(|e| format!("{name}: {e}"))
        })
        .collect();
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    Ok(())
}

/// The planner does not reproduce the populated per-key table, and the zones refuse
/// edits, so an edit cannot rewrite a table the planner cannot derive.
fn key_map_outside_partner_law(s: &Specimen) -> Result<(), String> {
    let wide = samples::wide(s.entity)?;
    let plan = samples::planned_key_map(wide)?.ok_or("the map holds no per-key table")?;
    ensure!(
        plan.kind == nsmp::zone::KeyMap::Populated,
        "the per-key table is {:?}",
        plan.kind
    );
    ensure!(
        plan.planned != plan.stored,
        "the planner reproduces the table"
    );
    ensure!(!wide.zones_are_editable(), "the zones are editable");
    Ok(())
}

/// B3+bass preset 1 must read its separate bass drawbars, not stale nibbles.
fn b3_bass_manual(s: &Specimen) -> Result<(), String> {
    let (Entity::Program(Program::Electro5(p)) | Entity::Live(Live::Electro5(p))) = s.entity else {
        return Err("not an Electro 5 program".into());
    };
    let organ = &p.organ_panel;
    let bass = organ.b3_bass_drawbars();
    let main = organ.drawbars(OrganModel::B3, Preset::One);
    ensure!(
        bass == [0, 0] || [main[0], main[1]] != bass,
        "bass drawbars also appear in the main block's shadow nibbles; the accessor may be \
         reading the wrong place"
    );
    Ok(())
}

/// The zone upper keys are, or with `derived` false are not, the layout the root
/// keys imply.
fn zone_top_notes(s: &Specimen, derived: bool) -> Result<(), String> {
    let sample = samples::narrow(s.entity)?;
    let roots: Vec<u8> = sample
        .strokes()
        .context("strokes")?
        .iter()
        .map(|stroke| stroke.root_key)
        .collect();
    let stored: Vec<u8> = sample
        .zones()
        .context("zones")?
        .iter()
        .map(|zone| zone.top_note)
        .collect();
    ensure!(
        (stored == nsmp::zone::derive_top_notes(&roots)) == derived,
        "roots {roots:?} and upper keys {stored:?}"
    );
    Ok(())
}

/// The editor's render of its own source: each stroke's statistic A is built from
/// the file peak and the stroke's gain. A wide header states the gain in decibels.
/// A narrow zone record stores it mod `2^24`, so above a gain of 16 it reads back
/// quieter than the gain the mantissa was built from, and a wide render of the same
/// source, where the sidecar names one, supplies the gain instead.
///
/// Inferred from specimens; not confirmed on hardware.
fn editor_render(s: &Specimen) -> Result<(), String> {
    match samples::sample(s.entity)? {
        Sample::V3(sample) => {
            let layout = nsmp::codec::Layout::from_version(sample.header.version)
                .ok_or("a content version the codec does not model")?;
            let streams = sample.stroke_streams();
            let peak = samples::peak(&streams, layout);
            for (_, stroke) in &streams {
                let decibels =
                    nsmp::codec::zone_gain_db(stroke, layout).ok_or("no decibel field")?;
                samples::statistic_a(stroke, peak, samples::gain_units(decibels))?;
            }
        }
        Sample::V2(sample) => {
            let wide: Vec<_> = s
                .sidecar
                .wide_renders
                .iter()
                .map(|name| related(s.path, name))
                .collect::<Result<_, _>>()?;
            let zones = sample.zones().context("zones")?;
            let streams = sample.stroke_streams();
            let peak = samples::peak(&streams, nsmp::codec::Layout::V2);
            for (_, stroke) in &streams {
                let id = stroke[3];
                let from_wide = wide.iter().find_map(|(_, entity)| {
                    samples::wide(entity)
                        .ok()
                        .and_then(|w| samples::wide_stroke_gain(w, id))
                });
                let gain = from_wide.unwrap_or_else(|| {
                    u64::from(
                        zones
                            .iter()
                            .find(|z| z.stroke_id == id)
                            .map_or(nsmp::zone::GAIN_UNITY, |z| z.gain),
                    )
                });
                samples::statistic_a(stroke, peak, gain)?;
            }
        }
    }
    Ok(())
}

/// The instrument has a stroke whose id does not fit the byte a zone record names
/// it by, and every zone still pairs with its stroke.
fn stroke_ids_past_a_byte(s: &Specimen) -> Result<(), String> {
    let sample = samples::narrow(s.entity)?;
    ensure!(
        sample
            .stroke_streams()
            .iter()
            .any(|(_, st)| u32::from_be_bytes([st[0], st[1], st[2], st[3]]) > u32::from(u8::MAX)),
        "no stroke id past 255"
    );
    let zones = sample.zones().context("zones")?.len();
    let strokes = sample.strokes().context("strokes")?.len();
    ensure!(zones == strokes, "{zones} zones for {strokes} strokes");
    Ok(())
}

/// The first stroke is a sine at the pitch of its zone's root key, within 10 cents.
fn sine_at_root_key(s: &Specimen) -> Result<(), String> {
    let sample = samples::sample(s.entity)?;
    let zones = sample.zones().context("zones")?;
    let zone = zones.first().ok_or("no zone")?;
    let audio = nsmp::codec::decode(zone.stream, zone.at, sample.layout().context("layout")?)
        .context("decode")?;
    let want = 440.0 * 2f64.powf((f64::from(zone.root_key) - 69.0) / 12.0);
    let window: Vec<f64> = audio
        .samples
        .iter()
        .skip(audio.samples.len() / 4)
        .take(2048)
        .map(|&sample| f64::from(sample))
        .collect();
    let power = |hz: f64| {
        let radians = std::f64::consts::TAU * hz / f64::from(nsmp::codec::FIELD_RATE);
        let (mut real, mut imaginary) = (0.0, 0.0);
        for (index, &sample) in window.iter().enumerate() {
            real += sample * (radians * index as f64).cos();
            imaginary -= sample * (radians * index as f64).sin();
        }
        real * real + imaginary * imaginary
    };
    let cent = 2f64.powf(1.0 / 1200.0);
    let best = (-100..=100)
        .map(|offset| want * cent.powi(offset))
        .max_by(|a, b| power(*a).total_cmp(&power(*b)))
        .unwrap();
    let error = 1200.0 * (best / want).log2();
    let in_tune = error.abs() < 10.0;
    ensure!(in_tune, "{error:.1} cents from the root key");
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Channel {
    Left,
    Right,
}

/// The first stroke's two channels, deinterleaved.
fn channels(sample: &Sample) -> Result<[Vec<i16>; 2], String> {
    let (at, stroke) = *sample.stroke_streams().first().ok_or("no stroke")?;
    let audio =
        nsmp::codec::decode(stroke, at, sample.layout().context("layout")?).context("decode")?;
    ensure!(audio.channels == 2, "{} channels", audio.channels);
    Ok([
        audio.samples.iter().step_by(2).copied().collect(),
        audio.samples[1..].iter().step_by(2).copied().collect(),
    ])
}

/// The source was loud in one channel and silent in the other.
fn one_channel(s: &Specimen, loud: Channel) -> Result<(), String> {
    let [left, right] = channels(samples::sample(s.entity)?)?;
    let peak = |v: &[i16]| v.iter().map(|s| i32::from(*s).abs()).max().unwrap_or(0);
    let (on, off) = match loud {
        Channel::Left => (peak(&left), peak(&right)),
        Channel::Right => (peak(&right), peak(&left)),
    };
    ensure!(
        on > 10_000 && off == 0,
        "{loud:?} peak {on}, the other channel's {off}"
    );
    Ok(())
}

fn impulses(s: &Specimen) -> Result<(), String> {
    let Some(Impulses {
        left: want_left,
        right: want_right,
    }) = &s.sidecar.impulses
    else {
        return Ok(());
    };
    let [left, right] = channels(samples::sample(s.entity)?)?;
    let hits = |v: &[i16]| -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for (f, &s) in v.iter().enumerate() {
            if i32::from(s).abs() > 4_000 && out.last().is_none_or(|&last| f - last > 8) {
                out.push(f);
            }
        }
        out.iter()
            .map(|&f| {
                f * usize::try_from(nsmp::codec::SOURCE_RATE).unwrap()
                    / usize::try_from(nsmp::codec::FIELD_RATE).unwrap()
            })
            .collect()
    };
    for (channel, got, want) in [
        ("left", hits(&left), want_left),
        ("right", hits(&right), want_right),
    ] {
        ensure!(
            got.len() == want.len() && got.iter().zip(want).all(|(g, w)| g.abs_diff(*w) <= 2),
            "{channel} impulses at {got:?}, the sidecar says {want:?}"
        );
    }
    Ok(())
}

/// The editor's loader replaces `samplib_attrs` with a preset chosen by the
/// instrument's category, and the narrow encoder copies that preset's two velocity
/// depths into the file. No project can set the depths directly, so the project a
/// render came from identifies them.
fn source(s: &Specimen) -> Result<(), String> {
    let Some(name) = &s.sidecar.source else {
        return Ok(());
    };
    let sample = samples::narrow(s.entity)?;
    let (_, entity) = related(s.path, name)?;
    let installed = samples::project(&entity)?
        .velocity_defaults()
        .context("velocity defaults")?;
    let sty = sample.sty().context("sty")?;
    let depth = |value: u8| {
        nsmp::velocity_level(value).ok_or_else(|| format!("unsupported velocity depth {value}"))
    };
    let want = (depth(installed.amplitude)?, depth(installed.timbre)?);
    let held = (sty.velocity_to_amplitude(), sty.velocity_to_timbre());
    ensure!(
        held == want,
        "velocity depths {held:?}, and {name}'s preset {installed:?} installs {want:?}"
    );
    Ok(())
}

/// Building the source project reproduces the render's container header, section
/// chain, `hdr`, `cat` and `map` payloads, and each zone's field count, and the built
/// instrument walks and agrees with its own word directory.
fn builds_from_source(s: &Specimen) -> Result<(), String> {
    let name = s.sidecar.source.as_ref().ok_or("no source")?;
    let twin = samples::narrow(s.entity)?;
    let (_, entity) = related(s.path, name)?;
    let project = samples::project(&entity)?;
    let zones = samples::built_zones(project)?;
    let built = samples::built_v2(project, &zones)?;

    let ours = built.to_bytes().context("write")?;
    ensure!(ours[..0x18] == s.bytes[..0x18], "container header");
    let chain = |body: &nsmp::Sample| -> Vec<(String, u8)> {
        body.sections
            .iter()
            .map(|section| (section.tag_str(), section.version))
            .collect()
    };
    ensure!(
        chain(&built.body) == chain(&twin.body),
        "section chain {:?}, the editor's {:?}",
        chain(&built.body),
        chain(&twin.body)
    );
    for tag in [nsmp::section::HDR, nsmp::section::CAT, nsmp::section::MAP] {
        ensure!(
            nsmp::section::find(&built.body.sections, tag).map(|s| &s.payload)
                == nsmp::section::find(&twin.body.sections, tag).map(|s| &s.payload),
            "{} section",
            String::from_utf8_lossy(tag)
        );
    }

    for (index, zone) in zones.iter().enumerate() {
        let (at, stream) = twin.zone_stream(index).context(format!("zone {index}"))?;
        let editor = nsmp::codec::decode(stream, at, nsmp::codec::Layout::V2)
            .context(format!("zone {index}"))?;
        let plan = nsmp::encode::Plan::new(
            nsmp::codec::Layout::V2,
            zone.audio.len(),
            1,
            zone.secondary_start,
        )
        .context(format!("zone {index} plan"))?;
        ensure!(
            plan.fields == editor.samples.len(),
            "zone {index}: {} fields planned for {} frames, the editor wrote {}",
            plan.fields,
            zone.audio.len(),
            editor.samples.len()
        );
    }

    let section_len = |tag| {
        nsmp::section::find(&built.body.sections, tag)
            .map(|s| s.payload.len())
            .ok_or_else(|| format!("built instrument has no {}", String::from_utf8_lossy(tag)))
    };
    let (map_len, cat_len) = (
        section_len(nsmp::section::MAP)?,
        section_len(nsmp::section::CAT)?,
    );
    let layout = nsmp::codec::Layout::V2;
    for (index, (at, stream)) in built.stroke_streams().iter().enumerate() {
        let head = nsmp::stroke::header_len(layout, nsmp::Chain::Library2, index, cat_len, map_len);
        ensure!(
            (stream.len() - head).is_multiple_of(nsmp::stroke::packet_len(layout)),
            "built stroke {index}: {} bytes over a {head}-byte header",
            stream.len()
        );
        let walk =
            nsmp::codec::walk(stream, *at, layout).context(format!("built stroke {index}"))?;
        let directory = nsmp::codec::Directory::read(stream)
            .ok_or_else(|| format!("built stroke {index}: no directory"))?;
        let resolve = |p| nsmp::codec::Directory::resolve(p, *at, layout);
        ensure!(
            resolve(directory.first_record) == walk.first_record
                && resolve(directory.terminator) == walk.terminator
                && walk
                    .records
                    .iter()
                    .any(|r| r.at == resolve(directory.resync)),
            "built stroke {index}: the directory disagrees with the walk"
        );
    }
    Ok(())
}

/// Each wide render of the same source decodes, first stroke against first stroke,
/// within one quantizer step of this narrow one.
///
/// Inferred from specimens; not confirmed on hardware.
fn wide_renders(s: &Specimen) -> Result<(), String> {
    if s.sidecar.wide_renders.is_empty() {
        return Ok(());
    }
    let first = |sample: &Sample| -> Result<(nsmp::codec::Audio, i32, bool), String> {
        let layout = sample.layout().context("layout")?;
        let (at, stroke) = *sample.stroke_streams().first().ok_or("no stroke")?;
        let directory = nsmp::codec::Directory::read(stroke).ok_or("no directory")?;
        Ok((
            nsmp::codec::decode(stroke, at, layout).context("decode")?,
            nsmp::codec::shift(stroke, layout).ok_or("no shift")?,
            directory.mark != directory.terminator,
        ))
    };
    let (narrow, narrow_shift, marked) = first(samples::sample(s.entity)?)?;
    for name in &s.sidecar.wide_renders {
        let (_, entity) = related(s.path, name)?;
        let (wide, wide_shift, _) = first(samples::sample(&entity)?).context(name)?;
        // Each generation keeps a loop mark at least its own minimum distance past the
        // resync point, so a loop that starts near the resync repeats more of itself
        // at v2 than in the wide generations. Only that repeat differs: the shorter
        // stream is a prefix of the longer one.
        ensure!(
            marked || narrow.samples.len() == wide.samples.len(),
            "{name}: lengths {} and {}, and nothing is marked",
            narrow.samples.len(),
            wide.samples.len()
        );
        let allowed = 4.max(1i32 << narrow_shift.max(wide_shift).clamp(0, 30));
        let worst = narrow
            .samples
            .iter()
            .zip(&wide.samples)
            .map(|(&a, &b)| (i32::from(a) - i32::from(b)).abs())
            .max()
            .unwrap_or(0);
        ensure!(
            worst <= allowed,
            "{name}: difference {worst}, limit {allowed}"
        );
    }
    Ok(())
}

/// The twin law. Of an editor render and its renders in the other generations, the
/// render at the finer shift, converted to another's generation, is that render. Each
/// converted stroke decodes to the source's fields at the target's shift, and its
/// record stream, shift and statistic B are the twin's. Where the report drops and
/// changes nothing, the whole file is the twin's, except bytes the report says a rule
/// filled in.
///
/// The choices are the editor's own: a loop mark is pushed to the narrow floor, and a
/// zone gain past the narrow record is clamped, which the report states. A wide
/// render the sidecar lists under `twin_sign_differs` holds the other sign of
/// statistic B from the one inferred from the narrow render's content.
///
/// A stream past the reach of the stroke directory's 16-bit word pointers is refused:
/// the editor writes them, and this crate's writer does not.
///
/// Inferred from specimens; not confirmed on hardware.
fn twin_law(s: &Specimen) -> Result<(), String> {
    use nord_format::convert::{self, Choices, GainChoice, LoopMarkChoice, NameChoice};
    use nord_format::convert::{OverlapChoice, Target};

    if s.sidecar.wide_renders.is_empty() {
        return Ok(());
    }
    let own = s.path.file_name().unwrap().to_string_lossy().into_owned();
    let mut renders = vec![(own, s.bytes.to_vec(), samples::parse(s.bytes)?)];
    for name in &s.sidecar.wide_renders {
        let (bytes, entity) = related(s.path, name)?;
        renders.push((name.clone(), bytes, entity));
    }
    if let Some(stray) = s
        .sidecar
        .twin_sign_differs
        .iter()
        .find(|name| !s.sidecar.wide_renders.contains(name))
    {
        return Err(format!(
            "twin_sign_differs names {stray}, which is no wide render"
        ));
    }
    let choices = Choices {
        gain: Some(GainChoice::Clamp),
        name: Some(NameChoice::Truncate),
        overlap: Some(OverlapChoice::Lower),
        loop_mark: Some(LoopMarkChoice::Push),
    };
    let mut wrong = Vec::new();
    for (_, _, source) in &renders {
        for (twin_name, twin_bytes, twin) in &renders {
            let (from, to) = (samples::sample(source)?, samples::sample(twin)?);
            let (finer, coarser) = (samples::lattices(from)?, samples::lattices(to)?);
            let layout = to.layout().context("layout")?;
            if from.layout().context("layout")? == layout
                || finer.iter().zip(&coarser).any(|(f, c)| f.shift > c.shift)
            {
                continue;
            }
            let pair = format!("{} as {}", from.generation(), to.generation());
            let past_reach = samples::past_directory_reach(from)?;
            let plan = convert::plan(source, Target::Nsmp(layout), &choices);
            let converted = plan.and_then(|plan| {
                let report = plan.report().clone();
                plan.apply().map(|out| (report, out))
            });
            let (report, out) = match (converted, past_reach) {
                (Err(_), true) => continue,
                (Ok(_), true) => {
                    wrong.push(format!(
                        "{pair}: a stream past the directory's reach converts"
                    ));
                    continue;
                }
                (Err(e), false) => {
                    wrong.push(format!("{pair}: {e}"));
                    continue;
                }
                (Ok(converted), false) => converted,
            };
            let sign_differs = s.sidecar.twin_sign_differs.contains(twin_name)
                && from.layout().context("layout")? == nsmp::codec::Layout::V2;
            let ours = samples::lattices(&out)?;
            let (out_streams, twin_streams) = (out.stroke_streams(), to.stroke_streams());
            let mut signs_differ = false;
            for (index, ((source, ours), (our_stroke, their_stroke))) in finer
                .iter()
                .zip(&ours)
                .zip(out_streams.iter().zip(&twin_streams))
                .enumerate()
            {
                let at = format!("{pair} zone {index}");
                if let Err(e) = samples::holds_fields(source, ours) {
                    wrong.push(format!("{at}: {e}"));
                }
                let header = layout.header_len();
                let (ours, theirs) = (our_stroke.1, their_stroke.1);
                if ours.get(header..) != theirs.get(header..) || ours.get(12) != theirs.get(12) {
                    wrong.push(format!("{at}: the record stream or shift differs"));
                }
                let peak = |stroke: &[u8]| nsmp::codec::peak(stroke, layout);
                match (peak(ours) == peak(theirs), sign_differs) {
                    (true, _) => {}
                    (false, true) if peak(ours).map(i32::abs) == peak(theirs).map(i32::abs) => {
                        signs_differ = true
                    }
                    (false, _) => wrong.push(format!(
                        "{at}: statistic B is {:?}, the twin's {:?}",
                        peak(ours),
                        peak(theirs)
                    )),
                }
            }
            if sign_differs && !signs_differ {
                wrong.push(format!("{pair}: twin_sign_differs, and every sign agrees"));
            }
            let strokes = out_streams.len();
            let bytes = nord_format::to_bytes(&Entity::Sample(out)).context("write")?;
            let compared = report.dropped.is_empty() && report.changed.is_empty();
            if compared {
                let reparsed = samples::parse(&bytes)?;
                let filled = filled_by_rules(&report, sign_differs, strokes);
                if let Err(e) = same_file(samples::sample(&reparsed)?, to, &filled) {
                    wrong.push(format!("{pair}: nothing is dropped or changed, and {e}"));
                } else if filled.is_empty() && bytes != *twin_bytes {
                    wrong.push(format!("{pair}: the bytes differ outside every section"));
                }
            }
        }
    }
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    Ok(())
}

/// A byte range of one section, by tag, or of one zone's stroke.
type Filled = Vec<(String, std::ops::Range<usize>)>;

/// The bytes the report says a rule filled in, where a twin may hold something else.
fn filled_by_rules(
    report: &nord_format::convert::Report,
    sign_differs: bool,
    strokes: usize,
) -> Filled {
    use nord_format::convert::{Field, Reason, ZoneField};

    let stroke = |index: usize| format!("stk {index}");
    let mut filled: Filled = Vec::new();
    for line in &report.from_rules {
        let ranges: Vec<(String, std::ops::Range<usize>)> = match &line.field {
            Field::Category => vec![("cat".into(), 0..1)],
            Field::SubCategory => vec![("cat".into(), 1..2)],
            Field::Timbre => vec![("cat".into(), 2..3)],
            Field::Envelope => vec![("cat".into(), 3..4)],
            Field::Motion => vec![("cat".into(), 4..5)],
            Field::Production | Field::Origin => vec![("cat".into(), 5..usize::MAX)],
            Field::VelocityToAmplitude => vec![("sty".into(), 4..5)],
            Field::VelocityToTimbre => vec![("sty".into(), 5..6)],
            Field::Bytes { section, range, .. } => vec![(section.clone(), range.clone())],
            Field::Zone {
                index,
                field: ZoneField::LoopDecay,
            } => vec![(stroke(*index), 62..66)],
            Field::Zone {
                index,
                field: ZoneField::Gain,
            } => match line.reason {
                Reason::NarrowGain => vec![(stroke(*index), 9..12), (stroke(*index), 57..61)],
                _ => {
                    let record = nsmp::zone::RECORDS_AT + nsmp::zone::RECORD_LEN * index;
                    vec![("map".into(), record + 3..record + 6)]
                }
            },
            _ => vec![],
        };
        filled.extend(ranges);
    }
    if sign_differs {
        filled.extend((0..strokes).map(|index| (stroke(index), 13..16)));
    }
    filled
}

/// `ours` against `theirs` header by header and section by section, skipping `filled`.
fn same_file(ours: &Sample, theirs: &Sample, filled: &Filled) -> Result<(), String> {
    let header = |sample: &Sample| match sample {
        Sample::V2(file) => file.header.clone(),
        Sample::V3(file) => file.header.clone(),
    };
    ensure!(
        header(ours) == header(theirs),
        "the container header differs"
    );
    let sections = |sample: &Sample| -> Vec<(String, u32, Vec<u8>)> {
        let mut strokes = 0;
        let mut named = |tag: String| match tag.as_str() {
            "stk" => {
                strokes += 1;
                format!("stk {}", strokes - 1)
            }
            _ => tag,
        };
        match sample {
            Sample::V2(file) => file
                .body
                .sections
                .iter()
                .map(|s| (named(s.tag_str()), u32::from(s.version), s.payload.clone()))
                .collect(),
            Sample::V3(file) => file
                .body
                .sections
                .iter()
                .map(|s| (named(s.tag_str()), s.version, s.payload.clone()))
                .collect(),
        }
    };
    let (ours, theirs) = (sections(ours), sections(theirs));
    ensure!(
        ours.len() == theirs.len(),
        "{} sections, the twin's {}",
        ours.len(),
        theirs.len()
    );
    for ((tag, version, payload), (their_tag, their_version, their_payload)) in
        ours.iter().zip(&theirs)
    {
        ensure!(
            (tag, version) == (their_tag, their_version) && payload.len() == their_payload.len(),
            "section {tag} v{version} of {} bytes, the twin's {their_tag} v{their_version} of {}",
            payload.len(),
            their_payload.len()
        );
        let skipped = |at: usize| filled.iter().any(|(t, r)| t == tag && r.contains(&at));
        if let Some(at) =
            (0..payload.len()).find(|&at| payload[at] != their_payload[at] && !skipped(at))
        {
            return Err(format!("section {tag} differs at byte {at}"));
        }
    }
    Ok(())
}

/// Setting the instrument the file was saved from to this one's name and zone upper
/// keys reproduces this file byte for byte.
fn edited_from(s: &Specimen) -> Result<(), String> {
    let Some(name) = &s.sidecar.edited_from else {
        return Ok(());
    };
    let target = samples::sample(s.entity)?;
    let want_name = target.name().context("name")?;
    let want_tops: Vec<u8> = target
        .zones()
        .context("zones")?
        .iter()
        .map(|zone| zone.top_note)
        .collect();
    let (before, _) = related(s.path, name)?;
    let after = samples::edited(&before, |sample| {
        sample.set_name(&want_name).context("rename")?;
        for (index, top) in want_tops.iter().enumerate() {
            sample
                .set_zone_top_note(index, *top)
                .context(format!("zone {index}"))?;
        }
        Ok(())
    })?;
    ensure!(
        after == s.bytes,
        "{name} edited to this name and layout differs at {:#x?}",
        samples::moved(&after, s.bytes)
    );
    Ok(())
}

fn audio_differs_from(s: &Specimen) -> Result<(), String> {
    let Some(name) = &s.sidecar.audio_differs_from else {
        return Ok(());
    };
    let (_, entity) = related(s.path, name)?;
    ensure!(
        samples::audio(samples::sample(s.entity)?)? != samples::audio(samples::sample(&entity)?)?,
        "decodes to the same audio as {name}"
    );
    Ok(())
}

/// The first stroke's walked landmarks are the ones planned for its source, and a
/// silent source rendered by this crate reproduces the file.
fn render(s: &Specimen) -> Result<(), String> {
    let Some(render) = &s.sidecar.render else {
        return Ok(());
    };
    let sample = samples::sample(s.entity)?;
    let layout = sample.layout().context("layout")?;
    let (at, stroke) = *sample.stroke_streams().first().ok_or("no stroke")?;
    let stream = nsmp::codec::walk(stroke, at, layout).context("walk")?;
    ensure!(
        stream.channels == usize::from(render.channels)
            && stream.cell == Some(usize::from(render.channels) * layout.cell()),
        "{} channels with cell {:?}",
        stream.channels,
        stream.cell
    );
    let plan = nsmp::encode::Plan::new(
        layout,
        render.frames,
        usize::from(render.channels),
        render.secondary_start,
    )
    .context("plan")?;
    let walked = samples::landmarks(&stream)?;
    ensure!(
        walked == samples::planned(&plan),
        "walked fields, warmup, resync field and resync length {walked:?}, planned {:?}",
        samples::planned(&plan)
    );
    if render.silent {
        silent_render(s, sample, layout, render)?;
    }
    Ok(())
}

fn silent_render(
    s: &Specimen,
    sample: &Sample,
    layout: nsmp::codec::Layout,
    render: &Render,
) -> Result<(), String> {
    let root = sample
        .zones()
        .context("zones")?
        .first()
        .ok_or("no zone")?
        .root_key;
    let ours = nsmp::encode::instrument(
        &vec![0i16; render.frames * usize::from(render.channels)],
        &nsmp::encode::Options::new(&sample.name().context("name")?)
            .root_key(root)
            .channels(render.channels)
            .layout(layout)
            .secondary_start(render.secondary_start),
    )
    .context("encode")?
    .to_bytes()
    .context("write")?;
    ensure!(
        ours.len() == s.bytes.len(),
        "{} bytes, the editor's {}",
        ours.len(),
        s.bytes.len()
    );
    let differing: Vec<usize> = (0..ours.len()).filter(|&i| ours[i] != s.bytes[i]).collect();
    ensure!(
        differing == render.differs_at,
        "differs at {differing:#x?}, the sidecar says {:#x?}",
        render.differs_at
    );
    Ok(())
}

/// A piano library whose every block this crate's coder wrote recodes byte for
/// byte: each block is identical, including its declared attenuation, and the
/// container is laid out the same way.
fn written_by_this_crate(s: &Specimen) -> Result<(), String> {
    let Entity::Piano(piano) = s.entity else {
        return Err("not a piano library".into());
    };
    let library = piano.library().context("parse")?;
    let again = npno::encode::rebuild(&library).context("recode")?;
    for (stroke, recoded) in library.strokes().iter().zip(&again.strokes) {
        ensure!(
            recoded.identical == recoded.blocks,
            "{stroke:?} came back with {} block(s) restated",
            recoded.blocks - recoded.identical
        );
    }
    let rebuilt = again
        .library
        .to_piano()
        .and_then(|p| nord_format::to_bytes(&Entity::Piano(p)))
        .context("write")?;
    ensure!(
        rebuilt == s.bytes,
        "the recode differs at {}",
        first_difference(&rebuilt, s.bytes)
    );
    Ok(())
}

/// A library built from rules alone, from the named library's decoded strokes,
/// reproduces this file. A script outside this crate wrote it, so it is an
/// independent oracle for the rules.
///
/// Confirmed on hardware.
fn recordings_from(s: &Specimen) -> Result<(), String> {
    let Some(name) = &s.sidecar.recordings_from else {
        return Ok(());
    };
    let (_, entity) = related(s.path, name)?;
    let Entity::Piano(source) = &entity else {
        return Err(format!("{name} is not a piano library"));
    };
    let library = source.library().context(name)?;
    let recordings: Vec<npno::encode::Recording> = library
        .strokes()
        .iter()
        .map(|stroke| {
            let audio =
                npno::codec::decode(stroke, library.channels()).context(format!("{stroke:?}"))?;
            ensure!(audio.clipped == 0, "{stroke:?} saturates the decode");
            Ok(npno::encode::Recording {
                root: stroke.root,
                bank: stroke
                    .bank()
                    .ok_or_else(|| format!("{stroke:?} names no bank"))?,
                layer: stroke.layer(),
                channels: audio.lanes,
            })
        })
        .collect::<Result<_, String>>()?;
    let (library_name, variant) = library.name();
    let built = npno::encode::build(
        &npno::encode::Donor::Rules(npno::encode::Rules::new(npno::encode::Kind::Grand)),
        &npno::encode::Options::new(&library_name).variant(&variant),
        &recordings,
    )
    .context("build")?;
    let bytes = built
        .to_piano()
        .and_then(|p| nord_format::to_bytes(&Entity::Piano(p)))
        .context("write")?;
    ensure!(
        bytes == s.bytes,
        "the rule-written library differs at {}",
        first_difference(&bytes, s.bytes)
    );
    Ok(())
}

pub fn first_difference(ours: &[u8], theirs: &[u8]) -> String {
    let at = ours
        .iter()
        .zip(theirs)
        .position(|(a, b)| a != b)
        .map_or_else(|| "the length".to_string(), |i| format!("{i:#x}"));
    format!("{at} (ours {} bytes, theirs {})", ours.len(), theirs.len())
}
