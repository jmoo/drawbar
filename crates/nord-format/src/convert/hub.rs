//! The hub: one model of a sample instrument that every generation reads into and
//! writes out of.

use crate::error::{Error, ParseError};
use crate::formats::nsmp::cat::NarrowCat;
use crate::formats::nsmp::codec::{self, Lattice, Layout};
use crate::formats::nsmp::encode::{self, LatticeInstrument, LatticeZone, Preset};
use crate::formats::nsmp::keymap::{KeyTable, Level};
use crate::formats::nsmp::zone::{VelocityWindow, GAIN_UNITY};
use crate::formats::nsmp::{self, Chain, Sty};
use crate::Sample;

/// A sample instrument as every generation can state it, with each stroke's audio
/// kept as its stream stores it.
///
/// A field one generation does not store is `None` when read from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Instrument {
    /// The container header's `aux` word: the first two categories in the editor's
    /// renders, all ones in library files. Inferred from specimens; not confirmed on
    /// hardware.
    pub aux: u32,
    pub name: String,
    /// The wide chain's sub name; empty where none is stored.
    pub sub_name: String,
    /// The `cat` section's first two categories, which every chain with a `cat`
    /// stores.
    pub category: Option<u8>,
    pub sub_category: Option<u8>,
    /// The narrow `cat`'s other three categories.
    pub timbre: Option<u8>,
    pub envelope: Option<u8>,
    pub motion: Option<u8>,
    /// The narrow `cat`'s two labels.
    pub production: Option<String>,
    pub origin: Option<String>,
    /// The keyboard map. Only the narrow chain's per-key records are read: a wide
    /// one's read neutral here.
    pub keys: KeyTable,
    /// The `sty` preset's dynamics enable, which every generation stores.
    pub dynamics_enabled: bool,
    /// The narrow preset's velocity-to-amplitude level.
    pub velocity_to_amplitude: Option<u8>,
    /// The narrow preset's velocity-to-timbre level.
    pub velocity_to_timbre: Option<u8>,
    /// In stored order.
    pub zones: Vec<Zone>,
}

/// One zone and the stroke that plays it.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub root_key: u8,
    pub top_note: u8,
    /// Where the zone reaches down to. `None` where zones tile, so a zone starts one
    /// above the next zone's top.
    pub low_note: Option<u8>,
    /// The stroke's global id.
    pub global_id: u32,
    /// Playback gain as a linear ratio, 1.0 for unity.
    pub gain: f64,
    /// The wide stroke header's loop decay amount.
    pub loop_decay: Option<f32>,
    /// Where the playing stroke sits on the strength axis.
    pub rel_strength: u16,
    pub velocity: Option<VelocityWindow>,
    pub audio: Lattice,
}

/// Which chain and stream units a file was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    pub chain: Chain,
    pub layout: Layout,
}

/// Read a sample instrument into the hub.
pub(super) fn read(sample: &Sample) -> Result<(Instrument, Origin), Error> {
    match sample {
        Sample::V2(file) => {
            let chain = file.chain()?;
            let sty = file.sty()?;
            let cat = nsmp::section::find(&file.body.sections, nsmp::section::CAT)
                .map(|cat| NarrowCat::parse(&cat.payload))
                .transpose()?;
            let zones = file.zones()?;
            let strokes = file.strokes()?;
            let file_peak = file_peak(&file.stroke_streams(), Layout::V2);
            let zones = zones
                .iter()
                .zip(&strokes)
                .enumerate()
                .map(|(index, (zone, stroke))| {
                    let (at, stream) = file.zone_stream(index)?;
                    Ok(Zone {
                        root_key: stroke.root_key,
                        top_note: zone.top_note,
                        low_note: None,
                        global_id: global_id(stream)?,
                        gain: f64::from(zone.gain) / f64::from(GAIN_UNITY),
                        loop_decay: None,
                        rel_strength: zone.rel_strength,
                        velocity: None,
                        audio: lattice(stream, at, Layout::V2, index, file_peak)?,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            Ok((
                Instrument {
                    aux: file.header.aux,
                    name: file.name()?,
                    sub_name: String::new(),
                    category: cat.as_ref().map(|c| c.category),
                    sub_category: cat.as_ref().map(|c| c.sub_category),
                    timbre: cat.as_ref().map(|c| c.timbre),
                    envelope: cat.as_ref().map(|c| c.envelope),
                    motion: cat.as_ref().map(|c| c.motion),
                    production: cat.as_ref().map(|c| c.production.clone()),
                    origin: cat.as_ref().map(|c| c.origin.clone()),
                    keys: file.key_table()?,
                    dynamics_enabled: sty.dynamics_enabled(),
                    velocity_to_amplitude: Some(sty.velocity_to_amplitude()),
                    velocity_to_timbre: Some(sty.velocity_to_timbre()),
                    zones,
                },
                Origin {
                    chain,
                    layout: Layout::V2,
                },
            ))
        }
        Sample::V3(file) => {
            let layout = sample.layout()?;
            let map = nsmp::section::find(&file.body.sections, nsmp::section::MAP4)
                .ok_or_else(|| ParseError::AssertFail("no map section".into()))?;
            let head = map
                .payload
                .get(..nsmp::keymap::RECORD_LEN)
                .ok_or_else(|| ParseError::AssertFail("the map holds no level".into()))?;
            let mut keys = KeyTable::NEUTRAL;
            keys.instrument = Level::read(head);
            let cat = nsmp::section::find(&file.body.sections, nsmp::section::CAT4)
                .ok_or_else(|| ParseError::AssertFail("no cat section".into()))?;
            let (category, sub_category) = nsmp::cat::parse_wide(&cat.payload)?;
            let dynamics_enabled = match file.sty()? {
                Sty::V3(sty) => sty.dynamics_enabled(),
                Sty::V4(sty) => sty.dynamics_enabled(),
            };
            let file_peak = file_peak(&file.stroke_streams(), layout);
            let zones = file
                .zones()?
                .iter()
                .enumerate()
                .map(|(index, zone)| {
                    let (at, stream) = file.zone_stream(index)?;
                    let decibels =
                        codec::zone_gain_db(stream, layout).ok_or_else(|| short_stroke(index))?;
                    Ok(Zone {
                        root_key: zone.root_key,
                        top_note: zone.top_note,
                        low_note: zone.low_note,
                        global_id: zone.stroke_gid,
                        gain: linear(decibels),
                        loop_decay: Some(
                            codec::loop_decay(stream, layout).ok_or_else(|| short_stroke(index))?,
                        ),
                        rel_strength: zone
                            .rel_strength
                            .unwrap_or(nsmp::zone::REL_STRENGTH_DEFAULT),
                        velocity: zone.velocity,
                        audio: lattice(stream, at, layout, index, file_peak)?,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            Ok((
                Instrument {
                    aux: file.header.aux,
                    name: file.name()?,
                    sub_name: file.sub_name()?,
                    category: Some(category),
                    sub_category: Some(sub_category),
                    timbre: None,
                    envelope: None,
                    motion: None,
                    production: None,
                    origin: None,
                    keys,
                    dynamics_enabled,
                    velocity_to_amplitude: None,
                    velocity_to_timbre: None,
                    zones,
                },
                Origin {
                    chain: Chain::Wide,
                    layout,
                },
            ))
        }
    }
}

/// Write the hub out in the generation `layout` names.
///
/// Fields the generation does not store are left out, and fields the hub does not
/// hold take what the editor writes when nothing sets them.
pub(super) fn write(instrument: &Instrument, layout: Layout) -> Result<Sample, Error> {
    with_parts(instrument, layout, encode::from_lattice)
}

/// The container header and every section [`write`] writes but the strokes and the
/// wide chain's closing length.
pub(super) fn frame(instrument: &Instrument, layout: Layout) -> Result<Sample, Error> {
    with_parts(instrument, layout, encode::frame)
}

/// The writer's inputs for `instrument` in `layout`, handed to `write`.
fn with_parts(
    instrument: &Instrument,
    layout: Layout,
    write: impl FnOnce(&LatticeInstrument<'_>, &[LatticeZone<'_>]) -> Result<Sample, Error>,
) -> Result<Sample, Error> {
    let narrow = layout == Layout::V2;
    let defaults = Preset::default();
    let cat = NarrowCat::editor_default();
    let narrow_only = |value: &Option<u8>, default: u8| match narrow {
        true => value.unwrap_or(default),
        false => default,
    };
    let label = |value: &Option<String>, default: String| match (narrow, value) {
        (true, Some(value)) => value.clone(),
        _ => default,
    };
    let categories = NarrowCat {
        category: instrument.category.unwrap_or(cat.category),
        sub_category: instrument.sub_category.unwrap_or(cat.sub_category),
        timbre: narrow_only(&instrument.timbre, cat.timbre),
        envelope: narrow_only(&instrument.envelope, cat.envelope),
        motion: narrow_only(&instrument.motion, cat.motion),
        production: label(&instrument.production, cat.production.clone()),
        origin: label(&instrument.origin, cat.origin.clone()),
    };
    let mut keys = instrument.keys.clone();
    if !narrow {
        let level = keys.instrument;
        keys = KeyTable::NEUTRAL;
        keys.instrument = level;
    }
    let zones: Vec<LatticeZone<'_>> = instrument
        .zones
        .iter()
        .map(|zone| LatticeZone {
            audio: &zone.audio,
            root_key: zone.root_key,
            top_note: zone.top_note,
            low_note: zone.low_note.filter(|_| !narrow),
            global_id: zone.global_id,
            gain: zone.gain,
            loop_decay: zone.loop_decay.unwrap_or(encode::DEFAULT_LOOP_DECAY),
            rel_strength: zone.rel_strength,
            velocity: zone.velocity.unwrap_or(VelocityWindow::FULL),
        })
        .collect();
    write(
        &LatticeInstrument {
            aux: instrument.aux,
            name: &instrument.name,
            sub_name: match narrow {
                true => "",
                false => &instrument.sub_name,
            },
            categories,
            keys,
            layout,
            preset: Preset {
                dynamics_enabled: instrument.dynamics_enabled,
                velocity_to_amplitude: instrument
                    .velocity_to_amplitude
                    .unwrap_or(defaults.velocity_to_amplitude),
                velocity_to_timbre: instrument
                    .velocity_to_timbre
                    .unwrap_or(defaults.velocity_to_timbre),
            },
        },
        &zones,
    )
}

/// A zone gain back from the decibels a wide header stores.
///
/// ⚠️ Every negative gain stores the same NaN, which then writes the same bytes in
/// every generation, so a NaN reads back as a gain of −1.
fn linear(decibels: f32) -> f64 {
    match decibels.is_nan() {
        true => -1.0,
        false => 10f64.powf(f64::from(decibels) / 20.0),
    }
}

fn global_id(stream: &[u8]) -> Result<u32, Error> {
    stream
        .first_chunk()
        .map(|b| u32::from_be_bytes(*b))
        .ok_or_else(|| ParseError::AssertFail("a stroke too short for its id".into()).into())
}

/// The largest magnitude of statistic B over every stroke, which each stroke's shift
/// is stated against.
fn file_peak(streams: &[(usize, &[u8])], layout: Layout) -> u32 {
    streams
        .iter()
        .filter_map(|(_, stream)| codec::peak(stream, layout))
        .map(i32::unsigned_abs)
        .max()
        .unwrap_or(0)
}

fn lattice(
    stream: &[u8],
    at: usize,
    layout: Layout,
    index: usize,
    file_peak: u32,
) -> Result<Lattice, Error> {
    codec::lattice(stream, at, layout, file_peak).map_err(|why| {
        ParseError::AssertFail(format!("zone {index}'s stroke does not decode: {why}")).into()
    })
}

fn short_stroke(index: usize) -> Error {
    ParseError::AssertFail(format!("zone {index}'s stroke is shorter than its header")).into()
}
