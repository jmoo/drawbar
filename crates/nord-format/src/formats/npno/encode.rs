//! Writing a piano library: recordings coded into blocks, and a container laid out
//! around them.
//!
//! [`build`] takes a **template** library and a set of [`Recording`]s — one per root
//! note, [`Bank`] and velocity layer, frames at [`codec::RATE`] — and returns a
//! [`Library`] the parent module's writer turns into a file. [`rebuild`] re-codes a
//! library's own strokes from the frames they decode to: a file this crate did not
//! write comes back block for block, and one it did write comes back byte for byte.
//!
//! # The coding laws
//!
//! A block's width fixes its frame count, `F(w) = ⌊8·(1022·C − 2)/(w·C)⌋`, so a wider
//! block is a shorter one and the width and the segmentation are one choice. Per
//! block: for each order up to [`codec::MAX_ORDER`], take the narrowest width whose
//! order-`w` residuals all fit `w` signed bits and whose frames still fit what the
//! stroke has left; then take the order that reaches the narrowest width, ties to the
//! lowest order. Width 1 occurs and there is no floor above it.
//!
//! Every block but the first opens by restating the previous block's last
//! [`codec::OVERLAP`] frames against the running history, so a block owns
//! `F(w) − OVERLAP` frames and the stroke owns their sum. The last block is an
//! ordinary full block whose own trailing overlap sits past the stroke's end, which
//! is why coding a stroke again needs [`codec::Audio::tail`].
//!
//! A stroke therefore states whole blocks, and it states every frame it was given.
//! Where the capped search lands on the frame count exactly, that count is what the
//! stroke states. Where it cannot — the remainder shorter than any width's block —
//! the blocks would have to stop short of the audio, so the stroke is laid out again
//! with nothing capping any block and ends at the first block to reach the count, the
//! source read as silent past its end. Such a stroke ends in silence, up to one block
//! of it, and no frame is dropped for falling between block lengths.
//!
//! Both layouts code again unchanged. The cap is what the stroke has left to own, so
//! a stroke whose blocks land on its frame count gives the same search the same room
//! the second time and reaches the same widths. A stroke laid out with no cap states
//! the sum of those blocks, and a capped search over that sum admits every one of
//! them — each is no longer than what is left when it starts, and the cap only ever
//! removes candidates — so it lays out the same blocks and this time lands exactly.
//!
//! Confirmed on hardware: what this codes plays. Libraries built here load and
//! sound — mono and stereo, every key of a full-keyboard library including its lowest
//! and highest root, each of three attack layers, the release stroke at note-off, a
//! long stroke to its end, and the keys between roots transposed — and a vendor
//! library coded again from its own audio plays indistinguishably from the original,
//! in level and in spectrum.
//!
//! That the width and order it *chooses* are the vendor's own choice is inferred from
//! specimens: given each block's width, order and attenuation, this reproduces the
//! blocks of every specimen read, byte for byte, and the width and order it derives
//! are the ones those files declare, apart from a handful of libraries whose headers
//! were decided on a signal that is not the one they store. The attenuation is the
//! same kind of thing one step smaller: it is a statistic the vendor's encoder
//! recorded rather than a function of the frames it went on to store, so a block
//! coded again from its own audio can declare a neighbouring value. Nothing in
//! [`codec`] reads it.
//!
//! # What the audio does not say
//!
//! A stroke record carries fields no audio predicts: four length marks, fifteen
//! one-pole decay coefficients, a velocity window, a per-stroke identifier, and two
//! bytes the later streams use. Nor does the prefix's bank of per-note tables and
//! playback parameters. [`Donor`] is where [`build`] gets them.
//!
//! [`Donor::Template`] copies them from a library — for each recording, the template
//! stroke of the same bank and nearest root, with the marks rescaled to the new
//! stroke's length. They go in as the template donated them: the instrument accepts
//! them, and what it makes of them beyond accepting is not known. What comes with them
//! is the vendor's tuning of an instrument these recordings are not.
//!
//! [`Donor::Rules`] states them instead, so a library can be written from recordings
//! alone. Every one is then a neutral playback parameter: no decay applied over the
//! recordings, each stroke trimmed by its own layer value, and the damper reaching the
//! keys the kind of instrument dampens. Confirmed on hardware: a library written this
//! way plays like the same audio built against a template, within about a decibel at
//! every velocity and key, and sustains longer because nothing is applied over it.

use super::codec::{self, MAX_ORDER, MAX_WIDTH, MIN_WIDTH, OVERLAP};
use super::{
    be32, block_bytes, midi_key, Bank, Library, Stroke, CNSP_MAGIC, DAMPER_TOP_AT, DECAYS,
    DIRECTORY_AT, FINE_TUNE_AT, FORMAT, GAIN_AT, KEY_MAP_AT, KIND_AT, LADDER_UNITY, MARKS, NOTES,
    RECORD, REC_BANK, REC_BLOCKS, REC_DECAY, REC_DECAYS, REC_FRAMES, REC_ID, REC_LAYER, REC_MARKS,
    REC_MARK_BLOCK, REC_SEEDS, REC_START, REC_TRIM, REC_WINDOW, SEEDS, UNCOVERED, VERSION_AT,
    VERSION_ECHO_AT,
};
use crate::cbin::Header;
use crate::error::{Error, ParseError};
use crate::formats::nsmp::kernel;
use std::borrow::Cow;
use std::collections::BTreeSet;

/// Full scale the header's attenuation statistic is measured against.
const FULL_SCALE: f64 = 8192.0;

/// Widest field a header can declare, as an index bound.
const WIDTHS: usize = MAX_WIDTH as usize + 1;

/// What a library states about itself besides its strokes. Everything else — the
/// stream version, the per-note tables, the word at body `0x06` — comes from the
/// [`Donor`] [`build`] is given.
#[derive(Debug, Clone)]
pub struct Options {
    /// The half of the `Name#Variant` field before the separator.
    pub name: String,
    /// The half after it, where the vendor records the voicing and the library's size.
    pub variant: String,
}

impl Options {
    pub fn new(name: &str) -> Options {
        Options {
            name: name.to_owned(),
            variant: String::new(),
        }
    }

    pub fn variant(mut self, variant: &str) -> Options {
        self.variant = variant.to_owned();
        self
    }
}

/// Where [`build`] takes the fields no audio predicts from.
#[derive(Debug, Clone)]
pub enum Donor<'a> {
    /// A library to copy them from: its prefix whole, and per stroke the length marks
    /// and decay ladder of its nearest stroke of the same bank.
    Template(&'a Library<'a>),
    /// The rules that state them instead, which is what a library built from nothing
    /// but recordings carries.
    Rules(Rules),
}

/// The kind of instrument a library states it holds, at body `0x18`.
///
/// The instrument files the library under it. Which code names which kind is inferred
/// from specimens; not confirmed on hardware. Confirmed on hardware: the byte changes
/// nothing a library sounds like.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    ElectricGrand,
    /// The tine electric pianos.
    ElectricPiano,
    /// The reed electric pianos.
    Wurlitzer,
    Clavinet,
    #[default]
    Grand,
    Upright,
    Harpsichord,
    DigitalPiano,
    /// The hybrid and ballad electric pianos.
    Hybrid,
    Mallet,
}

impl Kind {
    pub fn code(self) -> u8 {
        match self {
            Kind::ElectricGrand => 1,
            Kind::ElectricPiano => 2,
            Kind::Wurlitzer => 3,
            Kind::Clavinet => 4,
            Kind::Grand => 5,
            Kind::Upright => 6,
            Kind::Harpsichord => 7,
            Kind::DigitalPiano => 14,
            Kind::Hybrid => 15,
            Kind::Mallet => 16,
        }
    }

    /// The [`Rules::damper_top`] this kind of instrument has: the acoustic pianos damp
    /// to a key well below the top of the keyboard and let the rest ring, the reed
    /// pianos to a higher one, and everything else damps every key.
    pub fn damper_top(self) -> u8 {
        match self {
            Kind::Grand | Kind::Upright => 90,
            Kind::Wurlitzer => 97,
            _ => ALL_KEYS_DAMPED,
        }
    }
}

/// A [`Rules::damper_top`] above the highest key the instrument plays, so that every
/// key is damped at note-off.
pub const ALL_KEYS_DAMPED: u8 = 109;

/// The [`Rules::gain`] a library states unless a caller says otherwise: +5.0 dB.
pub const DEFAULT_GAIN: i8 = 50;

/// What a library states about its playback where no template donates it.
///
/// These are the parameters a recording cannot carry, at their neutral settings: the
/// library is heard at [`Rules::gain`], each stroke is trimmed by its own layer value,
/// nothing is applied over the recordings' own decay, and the damper reaches
/// [`Rules::damper_top`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub kind: Kind,
    /// Body `0x40c`: a gain over the whole library in tenths of a decibel. Confirmed
    /// on hardware.
    pub gain: i8,
    /// Body `0x40d`: the highest key the instrument damps at note-off. Keys above it
    /// ring on, and [`ALL_KEYS_DAMPED`] leaves none of them. Confirmed on hardware.
    pub damper_top: u8,
}

impl Rules {
    /// The neutral rules for one kind of instrument.
    pub fn new(kind: Kind) -> Rules {
        Rules {
            kind,
            gain: DEFAULT_GAIN,
            damper_top: kind.damper_top(),
        }
    }
}

impl Default for Rules {
    fn default() -> Rules {
        Rules::new(Kind::default())
    }
}

/// One recording to code: what it is played for, and its frames.
#[derive(Debug, Clone)]
pub struct Recording {
    /// The note it was recorded at.
    pub root: u8,
    pub bank: Bank,
    /// The layer value the stroke record states, 0 being the loudest recording of the
    /// root and bank. Selection reads this value, not a rank among the layers present
    /// ([`Stroke::layer`]); [`layer_value`] spreads a root's layers across the
    /// velocity range the way a vendor library does.
    pub layer: u8,
    /// One vector per channel at [`codec::RATE`], all the same length. Every
    /// recording of one library states the same channel count, 1 or 2.
    ///
    /// The stroke holds whole blocks and holds all of these frames, so it states them
    /// and whatever silence fills out the block they end in — the recording is read as
    /// silent past its end rather than cut back to a block boundary.
    pub channels: Vec<Vec<i16>>,
}

/// The softest layer value a root is given when [`layer_value`] spreads it: the top
/// of the range vendor libraries use, and well inside [`HIGHEST_PLAYED_LAYER`].
pub const SOFTEST_LAYER: u8 = 27;

/// The largest layer value any velocity sounds: `(127 − 1)·31/127`, the selection
/// bound at velocity 1, the softest note-on a key can send.
///
/// A key sounds the largest value its root holds that is at most
/// `(127 − velocity)·31/127` ([`Stroke::layer`]), and that bound only falls as the
/// velocity rises, so a stroke stating more than this is one no playing reaches.
/// [`build`] refuses one rather than write a library with a silent stroke in it.
pub const HIGHEST_PLAYED_LAYER: u8 = ((127 - 1) * 31 / 127) as u8;

/// The value the `index`-th loudest of a root's `layers` takes when the caller states
/// none: `round(index·27/(layers − 1))`, and 0 for a root holding one.
///
/// A key sounds the largest layer value its root holds that is at most
/// `(127 − velocity)·31/127` ([`Stroke::layer`]), so the spread is what puts a layer
/// change under each part of the velocity range; values packed at the loud end leave
/// the softest layer playing almost everywhere. An `index` past the last is that one.
pub fn layer_value(index: usize, layers: usize) -> u8 {
    let last = layers.saturating_sub(1);
    if last == 0 {
        return 0;
    }
    let index = index.min(last);
    let scale = usize::from(SOFTEST_LAYER);
    ((index * scale * 2 + last) / (last * 2)) as u8
}

/// A library rebuilt from its own audio, and how each stroke's blocks compare with
/// the ones they were coded from.
pub struct Rebuilt {
    pub library: Library<'static>,
    /// One entry per stroke, in directory order.
    pub strokes: Vec<Recoded>,
}

/// How one stroke's blocks came back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Recoded {
    /// Blocks the coder wrote.
    pub blocks: usize,
    /// Blocks byte-identical to the ones read.
    pub identical: usize,
    /// Blocks identical apart from the attenuation byte, which the decode never reads.
    pub restated: usize,
}

impl Recoded {
    /// Blocks that came back with different residuals, a different width or order, or
    /// no counterpart at all.
    pub fn recoded(&self) -> usize {
        self.blocks - self.identical - self.restated
    }
}

/// Build a library from recordings, taking from `donor` every field the audio does
/// not decide.
///
/// The recordings may arrive in any order; the directory sorts them by root, then
/// bank, then layer, which is the ascending root order the per-root counts index by.
/// The key map routes every key up to one semitone above the highest root, and the
/// per-key fine tune starts at zero rather than carrying a template's.
pub fn build(
    donor: &Donor<'_>,
    options: &Options,
    recordings: &[Recording],
) -> Result<Library<'static>, Error> {
    let channels = check_recordings(recordings)?;

    let (header, mut prefix) = match donor {
        Donor::Template(template) => (template.header.clone(), template.prefix.clone()),
        Donor::Rules(rules) => (
            Header::new(FORMAT, (0, 0), CONTENT_VERSION),
            rules_prefix(rules),
        ),
    };
    prefix[FINE_TUNE_AT..FINE_TUNE_AT + NOTES].fill(0);
    let roots: BTreeSet<u8> = recordings.iter().map(|r| r.root).collect();
    prefix[KEY_MAP_AT..KEY_MAP_AT + NOTES].copy_from_slice(&key_map(&roots));

    let mut library = Library {
        header,
        prefix,
        channels,
        strokes: Vec::new(),
    };
    library.set_name(&options.name)?;
    library.set_variant(&options.variant)?;

    let mut order: Vec<&Recording> = recordings.iter().collect();
    order.sort_by_key(|r| (r.root, r.bank.code(), r.layer));

    let mut donors = Vec::with_capacity(order.len());
    for (index, recording) in order.iter().enumerate() {
        donors.push(match donor {
            Donor::Template(template) => *donor_record(template, recording)?,
            Donor::Rules(_) => rules_record(recording, index),
        });
    }
    // A donor serving several recordings would name them all the same, so the
    // identifiers only carry over when they stay distinct.
    let unique: BTreeSet<u32> = donors.iter().map(|d| be32(d, REC_ID)).collect();
    let keep_ids = unique.len() == donors.len();

    for (index, (recording, donor)) in order.iter().zip(&donors).enumerate() {
        let seeds = seeds_for(&recording.channels);
        let target = recording.channels[0].len();
        let coded = code(&recording.channels, &seeds, target)?;
        let id = if keep_ids {
            be32(donor, REC_ID)
        } else {
            index as u32 + 1
        };
        library.strokes.push(Stroke {
            root: recording.root,
            record: record(
                donor,
                &coded,
                recording.bank.code(),
                recording.layer,
                &seeds,
                id,
            )?,
            audio: Cow::Owned(coded.audio),
        });
    }
    Ok(library)
}

/// The content version the container states where no template donates one: a library's
/// own version times a hundred, as the instrument reports it. Confirmed on hardware
/// only in that a library stating it loads and plays.
const CONTENT_VERSION: u32 = 540;

/// The stream version a rule-written prefix states, at [`VERSION_AT`],
/// [`VERSION_REPEAT_AT`] and [`VERSION_ECHO_AT`].
const RULES_VERSION: u16 = 0x450;

/// u32 the vendor makes distinct per library. The instrument does not read it, so a
/// rule-written prefix states one. Confirmed on hardware.
const FILE_ID_AT: usize = 0x06;
const FILE_ID: u32 = 1;

/// The stream version again, ahead of the echo at [`VERSION_ECHO_AT`].
const VERSION_REPEAT_AT: usize = 0x16;

/// The three bytes after [`KIND_AT`]: a model id within the kind and the library's
/// version digit — neither of which a rule-written prefix claims — then a format
/// constant.
const KIND_TRAILER: [u8; 3] = [0, 0, 2];

/// The per-note tables, [`NOTES`] bytes each, at the value that states nothing about
/// the note: no retune at [`FINE_TUNE_AT`], and for the rest the value a library that
/// has been played holds, sweeping any of them having moved nothing measurable.
/// Confirmed on hardware.
const PER_NOTE_TABLES: [(usize, u8); 6] = [
    (0x10c, 0),
    (FINE_TUNE_AT, 0),
    (0x20c, 57),
    (0x28c, 0),
    (0x30c, 0),
    (0x38c, 0),
];

/// The playback parameters, zero but for the fields [`rules_prefix`] writes into them.
const PARAMETERS: std::ops::Range<usize> = 0x40c..0x60f;
/// The three bytes after the damper limit, whose meaning is open; every library holds
/// these.
const PARAMETER_TAIL_AT: usize = 0x40e;
const PARAMETER_TAIL: [u8; 3] = [10, 108, 1];
/// Nineteen bytes ahead of the damper cut whose meaning is open; every library holds
/// these.
const BEFORE_DAMPER_CUT_AT: usize = 0x489;
const BEFORE_DAMPER_CUT: [u8; 19] = [128; 19];
/// The damper cut per note, [`NOTES`] bytes of [`damper_cut`].
const DAMPER_CUT_AT: usize = 0x49d;
const _: () = assert!(DAMPER_CUT_AT + NOTES <= PARAMETERS.end);

/// The prefix a library states where no template donates one: the stream's own
/// constants, the parameters `rules` carries, and zero wherever the vendor writes
/// something only a recording session knows.
///
/// The name, the key map and the counts are laid over this by [`build`] and the
/// container's writer.
fn rules_prefix(rules: &Rules) -> Vec<u8> {
    let mut prefix = vec![0u8; DIRECTORY_AT];
    prefix[..CNSP_MAGIC.len()].copy_from_slice(CNSP_MAGIC);
    for at in [VERSION_AT, VERSION_REPEAT_AT, VERSION_ECHO_AT] {
        prefix[at..at + 2].copy_from_slice(&RULES_VERSION.to_be_bytes());
    }
    prefix[FILE_ID_AT..FILE_ID_AT + 4].copy_from_slice(&FILE_ID.to_be_bytes());
    prefix[KIND_AT] = rules.kind.code();
    prefix[KIND_AT + 1..KIND_AT + 1 + KIND_TRAILER.len()].copy_from_slice(&KIND_TRAILER);
    for (at, value) in PER_NOTE_TABLES {
        prefix[at..at + NOTES].fill(value);
    }
    prefix[GAIN_AT] = rules.gain as u8;
    prefix[DAMPER_TOP_AT] = rules.damper_top;
    prefix[PARAMETER_TAIL_AT..PARAMETER_TAIL_AT + PARAMETER_TAIL.len()]
        .copy_from_slice(&PARAMETER_TAIL);
    prefix[BEFORE_DAMPER_CUT_AT..BEFORE_DAMPER_CUT_AT + BEFORE_DAMPER_CUT.len()]
        .copy_from_slice(&BEFORE_DAMPER_CUT);
    for note in 0..NOTES {
        prefix[DAMPER_CUT_AT + note] = damper_cut(note);
    }
    prefix
}

/// The damper cut's entry for `note`, at [`DAMPER_CUT_AT`] `+ note`.
///
/// A plateau over the lowest notes, a straight fall to the highest key an instrument
/// plays, and a fixed value past it. Confirmed on hardware: a curve of this shape takes
/// a held key down within tens of milliseconds, where a flat table of any level takes
/// about half a second. What axis the instrument reads the table on is open.
fn damper_cut(note: usize) -> u8 {
    /// The last note of the plateau, and the note the fall ends on.
    const FLAT_TO: usize = 24;
    const TOP: usize = 108;
    const PLATEAU: f64 = 79.0;
    const FALL: f64 = 59.0;
    /// What every note past [`TOP`] states, which no key reaches.
    const PAST_TOP: u8 = 30;

    if note < FLAT_TO {
        PLATEAU as u8
    } else if note <= TOP {
        (PLATEAU - FALL * (note - FLAT_TO) as f64 / (TOP - FLAT_TO) as f64).round_ties_even() as u8
    } else {
        PAST_TOP
    }
}

/// The record a stroke states where no template donates one: no length marks, a decay
/// ladder that applies nothing, the trim its layer implies, and its own place in the
/// directory as the identifier.
///
/// [`record`] lays the audio's own fields over this, and reads the absent marks as the
/// zeros they are.
fn rules_record(recording: &Recording, index: usize) -> [u8; RECORD] {
    let mut out = [0u8; RECORD];
    let (window, trim) = velocity_window(recording.bank, recording.layer);
    out[REC_WINDOW..REC_WINDOW + 2].copy_from_slice(&window.to_be_bytes());
    out[REC_TRIM..REC_TRIM + 2].copy_from_slice(&trim.to_be_bytes());
    for entry in 0..DECAYS {
        let at = REC_DECAYS + entry * 4;
        out[at..at + 4].copy_from_slice(&LADDER_UNITY.to_be_bytes());
    }
    out[REC_ID..REC_ID + 4].copy_from_slice(&(index as u32 + 1).to_be_bytes());
    out
}

/// The trim [`REC_TRIM`] states for a release stroke, in decibels.
const RELEASE_TRIM: u16 = 12;

/// The largest trim [`velocity_window`] states, the top of the range the layer values
/// are selected over ([`Stroke::layer`]).
const WIDEST_TRIM: u16 = 31;

/// The pair at [`REC_WINDOW`] and [`REC_TRIM`] a stroke of `bank` and `layer` states.
///
/// An attack or resonance stroke is trimmed three decibels past its layer value, so
/// that the softer layers of a root play softer than the loud ones by the amount their
/// values already say they are; a release stroke takes a fixed trim instead, its layer
/// value being no part of how it is selected.
fn velocity_window(bank: Bank, layer: u8) -> (u16, u16) {
    match bank {
        Bank::Release => (0, RELEASE_TRIM),
        Bank::Attack | Bank::Resonance => (
            u16::from(layer),
            u16::from(layer).saturating_add(3).min(WIDEST_TRIM),
        ),
    }
}

/// Code every stroke of `library` again from the frames it decodes to, each keeping
/// its own record and its own place in the directory.
///
/// A stroke whose decode saturates is refused rather than coded: the frames it would
/// be given are the clamped ones, so what came back would be a stroke holding audio
/// the file does not, and a stream that saturates this predictor is one the codec
/// does not describe.
pub fn rebuild(library: &Library<'_>) -> Result<Rebuilt, Error> {
    let block = library.block_bytes();
    let mut strokes = Vec::new();
    let mut report = Vec::new();
    for stroke in library.strokes() {
        let audio = codec::decode(stroke, library.channels())?;
        if audio.clipped > 0 {
            return Err(refuse(format!(
                "{stroke:?}: {} sample(s) left int16 in the decode; coding a stroke this                  codec does not describe would write the saturated frames as new audio",
                audio.clipped
            )));
        }
        let target = audio.frames();
        let mut source = audio.channels;
        for (channel, tail) in source.iter_mut().zip(&audio.tail) {
            channel.extend_from_slice(tail);
        }
        let seeds = stroke.seeds();
        let coded = code(&source, &seeds, target)?;
        report.push(compare(stroke.audio(), &coded.audio, block));
        strokes.push(Stroke {
            root: stroke.root,
            record: record(
                stroke.record(),
                &coded,
                stroke.bank_code(),
                stroke.layer(),
                &seeds,
                stroke.id(),
            )?,
            audio: Cow::Owned(coded.audio),
        });
    }
    Ok(Rebuilt {
        library: Library {
            header: library.header.clone(),
            prefix: library.prefix.clone(),
            channels: library.channels(),
            strokes,
        },
        strokes: report,
    })
}

/// What [`resample`] produced.
pub struct Resampled {
    /// One vector per channel at [`codec::RATE`].
    pub channels: Vec<Vec<i16>>,
    /// Samples a sum put outside `i16`, which saturate.
    pub clipped: usize,
}

/// 16-bit PCM at `rate`, interleaved by channel, resampled onto the stroke lattice.
///
/// The tap bank is [`nsmp`](crate::formats::nsmp::kernel)'s and the lattice is
/// `t(f) = rate·f / RATE`. Audio already at [`codec::RATE`] passes through untouched:
/// the bank interpolates rather than reproduces, so running it at a ratio of one
/// would filter the source for nothing.
///
/// The kernel's cutoff follows the rates: a source faster than [`codec::RATE`] is
/// band-limited to the lattice's own Nyquist before it lands on it, and a slower one
/// keeps its whole band.
pub fn resample(samples: &[i16], channels: usize, rate: u32) -> Result<Resampled, Error> {
    if channels == 0 || rate == 0 || !samples.len().is_multiple_of(channels) {
        return Err(ParseError::OutOfBounds {
            value: format!(
                "{} sample(s) of {channels} channel(s) at {rate} Hz",
                samples.len()
            ),
            bound: "whole frames of at least one channel at a positive rate".into(),
        }
        .into());
    }
    if rate == codec::RATE {
        let mut lanes = vec![Vec::new(); channels];
        for (i, &sample) in samples.iter().enumerate() {
            lanes[i % channels].push(sample);
        }
        return Ok(Resampled {
            channels: lanes,
            clipped: 0,
        });
    }

    let frames = samples.len() / channels;
    let fields = (frames as u128 * u128::from(codec::RATE) / u128::from(rate)) as usize;
    let kernel = kernel::Kernel::new(rate, codec::RATE);
    let mut clipped = 0;
    let mut lanes = Vec::with_capacity(channels);
    for channel in 0..channels {
        let lane: Vec<i16> = samples
            .iter()
            .skip(channel)
            .step_by(channels)
            .copied()
            .collect();
        lanes.push(
            (0..fields)
                .map(|f| {
                    let value = kernel.field(&lane, f);
                    let narrow = value.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
                    clipped += usize::from(i64::from(narrow) != value);
                    narrow
                })
                .collect(),
        );
    }
    Ok(Resampled {
        channels: lanes,
        clipped,
    })
}

/// The channel count the recordings agree on, or the first thing about them a
/// library cannot state.
fn check_recordings(recordings: &[Recording]) -> Result<u16, Error> {
    let Some(first) = recordings.first() else {
        return Err(refuse(
            "a library with no recordings at all has nothing to play",
        ));
    };
    let channels = first.channels.len();
    if !(1..=2).contains(&channels) {
        return Err(ParseError::OutOfBounds {
            value: format!("{channels} channels"),
            bound: "1 or 2, which is what a library states".into(),
        }
        .into());
    }

    let mut seen = BTreeSet::new();
    for recording in recordings {
        let what = describe(recording);
        midi_key("root", recording.root)?;
        if recording.channels.len() != channels {
            return Err(refuse(format!(
                "{what} has {} channel(s) where another has {channels}; one library plays \
                 one channel count",
                recording.channels.len()
            )));
        }
        let frames = recording.channels[0].len();
        if recording.channels.iter().any(|c| c.len() != frames) {
            return Err(refuse(format!(
                "{what} has channels of unequal length; a frame is one sample of each"
            )));
        }
        if frames == 0 {
            return Err(refuse(format!("{what} has no frames")));
        }
        if recording.layer > HIGHEST_PLAYED_LAYER {
            return Err(refuse(format!(
                "{what} states a layer value no velocity selects; {HIGHEST_PLAYED_LAYER} is                  the largest a key ever sounds"
            )));
        }
        if !seen.insert((recording.root, recording.bank.code(), recording.layer)) {
            return Err(refuse(format!(
                "{what} is recorded twice; a root's layers are numbered within one bank"
            )));
        }
    }
    Ok(channels as u16)
}

fn describe(recording: &Recording) -> String {
    format!(
        "root {} {} layer {}",
        recording.root, recording.bank, recording.layer
    )
}

fn refuse(what: impl Into<String>) -> Error {
    ParseError::AssertFail(what.into()).into()
}

/// The root each key plays: the lowest root the key sits no more than a semitone
/// above. A key more than a semitone above the highest root is left uncovered.
///
/// Inferred from the key maps of vendor libraries; not confirmed on hardware. Those
/// also stop short of the lowest keys, which is the acoustic instrument's range
/// rather than anything the map derives.
fn key_map(roots: &BTreeSet<u8>) -> [u8; NOTES] {
    let mut map = [UNCOVERED; NOTES];
    for (key, slot) in map.iter_mut().enumerate() {
        if let Some(&root) = roots.range((key as u8).saturating_sub(1)..).next() {
            *slot = root;
        }
    }
    map
}

/// The template stroke a recording inherits the fields no audio predicts from: the
/// same bank and nearest root, then the nearest layer.
///
/// A release stroke zeroes the marks and the decay coefficient at `+0x2e` it would
/// inherit, so any stroke can donate to one; anything else needs a donor that declares
/// marks of its own.
fn donor_record<'a>(
    template: &'a Library<'_>,
    recording: &Recording,
) -> Result<&'a [u8; RECORD], Error> {
    let release = Bank::Release.code();
    let wanted = recording.bank.code();
    let same: Vec<&Stroke<'_>> = template
        .strokes()
        .iter()
        .filter(|s| s.bank_code() == wanted)
        .collect();
    let pool: Vec<&Stroke<'_>> = match (same.is_empty(), recording.bank) {
        (false, _) => same,
        (true, Bank::Release) => template.strokes().iter().collect(),
        (true, _) => template
            .strokes()
            .iter()
            .filter(|s| s.bank_code() != release)
            .collect(),
    };
    pool.iter()
        .min_by_key(|s| {
            (
                s.root.abs_diff(recording.root),
                s.layer().abs_diff(recording.layer),
            )
        })
        .map(|s| s.record())
        .ok_or_else(|| {
            refuse(format!(
                "the template records no stroke to take {}'s length marks and decay \
                 coefficients from, and nothing in the audio predicts them",
                describe(recording)
            ))
        })
}

/// One stroke's blocks, and what its record has to say about them.
struct Coded {
    audio: Vec<u8>,
    /// Frames the blocks own between them.
    owned: usize,
    /// The frame each block starts at.
    starts: Vec<usize>,
}

/// Where one block sits and what its header will say.
#[derive(Debug, Clone, Copy)]
struct Placed {
    at: usize,
    order: u8,
    width: u8,
}

/// Code whole blocks over `target` frames of `source` — one vector per channel — and
/// report the frames they own between them, which is never fewer than `target`.
///
/// Each block is capped at what `target` has left to own, and when the blocks land on
/// `target` exactly that is the stroke. When instead they would stop short — the
/// remainder shorter than [`shortest_block`], so that no width's block fits it — the
/// stroke is laid out again with no cap on any block and ends at the first one to
/// reach `target`, the source read as silent past its end. What it then owns past
/// `target` is silence the stroke states rather than audio it drops.
fn code(source: &[Vec<i16>], seeds: &[[i16; SEEDS]; 2], target: usize) -> Result<Coded, Error> {
    let channels = source.len();
    let block = block_bytes(channels as u16);
    let counts = frame_counts(block, channels);
    let widest = counts[usize::from(MIN_WIDTH)];
    let total = target
        .checked_add(widest)
        .and_then(|total| total.checked_mul(channels).map(|_| total))
        .ok_or_else(|| refuse("a stroke longer than this platform can address"))?;
    let planes = planes(source, seeds, total)?;

    let mut placed = lay_capped(&planes, channels, &counts, target);
    if placed.is_empty() || owned_by(&placed, &counts) != target {
        placed = lay_uncapped(&planes, channels, &counts, target);
    }

    let mut audio = Vec::new();
    for block_at in &placed {
        let frames = counts[usize::from(block_at.width)];
        let span = block_at.at * channels..(block_at.at + frames) * channels;
        let peak = planes[0][span.clone()]
            .iter()
            .map(|&v| i64::from(v).abs())
            .max()
            .unwrap_or(0);
        pack(
            &mut audio,
            block_at.order,
            block_at.width,
            attenuation(peak),
            &planes[usize::from(block_at.order)][span],
            block,
        );
    }
    Ok(Coded {
        audio,
        owned: owned_by(&placed, &counts),
        starts: placed.iter().map(|b| b.at).collect(),
    })
}

/// Frames a layout owns between its blocks.
fn owned_by(placed: &[Placed], counts: &[usize; WIDTHS]) -> usize {
    placed
        .last()
        .map_or(0, |b| b.at + counts[usize::from(b.width)] - OVERLAP)
}

/// Blocks over the frames from zero, each capped at what `target` has left to own.
///
/// They land on `target` exactly or stop short of it by less than
/// [`shortest_block`], which is the length no width's block fits.
fn lay_capped(
    planes: &[Vec<i32>],
    channels: usize,
    counts: &[usize; WIDTHS],
    target: usize,
) -> Vec<Placed> {
    let shortest = shortest_block(counts);
    let mut out = Vec::new();
    let mut at = 0usize;
    while target - at >= shortest {
        let block = place(planes, at, channels, counts, Some(target - at));
        at += counts[usize::from(block.width)] - OVERLAP;
        out.push(block);
    }
    out
}

/// Blocks over the frames from zero with nothing capping their length, up to and
/// including the first one whose frames reach `target`.
///
/// A capped search over the frames these own admits every one of them — each is no
/// longer than what is left when it starts — so it lays out the same blocks and lands
/// on that count exactly, which is what makes the stroke this writes one that codes
/// again unchanged.
fn lay_uncapped(
    planes: &[Vec<i32>],
    channels: usize,
    counts: &[usize; WIDTHS],
    target: usize,
) -> Vec<Placed> {
    let mut out = Vec::new();
    let mut at = 0usize;
    loop {
        let block = place(planes, at, channels, counts, None);
        at += counts[usize::from(block.width)] - OVERLAP;
        out.push(block);
        if at >= target {
            return out;
        }
    }
}

/// The block starting at frame `at`, `room` being what the stroke has left to own.
fn place(
    planes: &[Vec<i32>],
    at: usize,
    channels: usize,
    counts: &[usize; WIDTHS],
    room: Option<usize>,
) -> Placed {
    let (order, width) = choose(planes, at, channels, counts, room)
        .expect("order zero states a sample outright, which always fits sixteen bits");
    Placed { at, order, width }
}

/// Frames the shortest block a header can declare owns: the widest field, and so the
/// fewest frames. It is the grain the stroke's own frame count comes in.
fn shortest_block(counts: &[usize; WIDTHS]) -> usize {
    counts[usize::from(MAX_WIDTH)] - OVERLAP
}

/// Frames a block of each candidate width carries, the overlap included.
fn frame_counts(block: usize, channels: usize) -> [usize; WIDTHS] {
    let mut out = [0; WIDTHS];
    for width in MIN_WIDTH..=MAX_WIDTH {
        out[usize::from(width)] = codec::block_frames(width, block, channels);
    }
    out
}

/// `Δ^order` of the covered frames for every order a header can declare, each
/// interleaved by channel the way a block emits them and read as silent past the
/// source's end.
fn planes(
    source: &[Vec<i16>],
    seeds: &[[i16; SEEDS]; 2],
    total: usize,
) -> Result<Vec<Vec<i32>>, Error> {
    let channels = source.len();
    let sample = |channel: usize, n: isize| -> i64 {
        match usize::try_from(n) {
            Ok(n) => i64::from(source[channel].get(n).copied().unwrap_or(0)),
            Err(_) => i64::from(seeds[channel][(SEEDS as isize + n) as usize]),
        }
    };
    let mut out = Vec::with_capacity(MAX_ORDER + 1);
    for order in 0..=MAX_ORDER {
        let mut plane = residuals(total * channels)?;
        for n in 0..total {
            for channel in 0..channels {
                let mut acc = 0i64;
                for j in 0..=order {
                    let term = codec::binomial(order, j) * sample(channel, n as isize - j as isize);
                    acc += if j.is_multiple_of(2) { term } else { -term };
                }
                plane[n * channels + channel] = acc as i32;
            }
        }
        out.push(plane);
    }
    Ok(out)
}

fn residuals(len: usize) -> Result<Vec<i32>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| ParseError::OutOfBounds {
            value: format!("{len} residual(s)"),
            bound: "an allocation that fits memory".into(),
        })?;
    out.resize(len, 0);
    Ok(out)
}

/// The `(order, width)` a block starting at frame `at` declares.
///
/// `owned_left` caps a block's owned frames at what the stroke has left to own. The
/// narrowest width is the longest block, so the cap rules out an opening range of
/// widths; at [`shortest_block`] it leaves only the widest, which order zero always
/// reaches, so a cap that large or larger always names a block. `None` lifts the cap,
/// which is what a last block reaching past the source is chosen without.
fn choose(
    planes: &[Vec<i32>],
    at: usize,
    channels: usize,
    counts: &[usize; WIDTHS],
    owned_left: Option<usize>,
) -> Option<(u8, u8)> {
    let mut best: Option<(u8, u8)> = None;
    for (order, plane) in planes.iter().enumerate() {
        let mut lo = 0i32;
        let mut hi = 0i32;
        let mut scanned = at;
        let mut narrowest = None;
        // A wider block is a shorter one, so walking widths down grows the window a
        // step at a time and the span the residuals need never narrows again.
        for width in (MIN_WIDTH..=MAX_WIDTH).rev() {
            let frames = counts[usize::from(width)];
            for &value in &plane[scanned * channels..(at + frames) * channels] {
                lo = lo.min(value);
                hi = hi.max(value);
            }
            scanned = at + frames;
            let bound = 1i32 << (width - 1);
            if lo < -bound || hi >= bound {
                break;
            }
            if owned_left.is_none_or(|left| frames - OVERLAP <= left) {
                narrowest = Some(width);
            }
        }
        if let Some(width) = narrowest {
            if best.is_none_or(|(_, reached)| width < reached) {
                best = Some((order as u8, width));
            }
        }
    }
    best
}

/// Append one block: the header word, then the residuals as `width`-bit two's
/// complement fields low-bit-first, then zero to the block's length.
fn pack(out: &mut Vec<u8>, order: u8, width: u8, stat: u8, residuals: &[i32], block: usize) {
    let start = out.len();
    let header = (u16::from(stat) << 8) | (u16::from(order) << 5) | u16::from(width);
    out.extend_from_slice(&header.to_be_bytes());
    let mask = (1u64 << width) - 1;
    let mut reservoir = 0u64;
    let mut held = 0u32;
    for &value in residuals {
        reservoir |= (i64::from(value) as u64 & mask) << held;
        held += u32::from(width);
        while held >= 16 {
            out.extend_from_slice(&((reservoir & 0xffff) as u16).to_be_bytes());
            reservoir >>= 16;
            held -= 16;
        }
    }
    if held > 0 {
        out.extend_from_slice(&((reservoir & 0xffff) as u16).to_be_bytes());
    }
    out.resize(start + block, 0);
}

/// The header's high byte: how far a block's loudest frame sits below [`FULL_SCALE`],
/// in dB, rounded to a whole one and clamped to `0..=100`.
///
/// A silent block declares 100 where one count declares 78. Inferred from specimens;
/// not confirmed on hardware.
fn attenuation(peak: i64) -> u8 {
    if peak == 0 {
        return 100;
    }
    let db = -20.0 * (peak as f64 / FULL_SCALE).log10();
    (db + 0.5).floor().clamp(0.0, 100.0) as u8
}

/// The four seeds a new recording declares, oldest first: a zero, then the recording's
/// own first three frames.
///
/// Vendor strokes carry the four frames before the recording, the oldest of them zero
/// on every stroke of every specimen read. A recording that starts in silence has no
/// such frames to carry and this states zeros, which is the same thing.
fn seeds_for(source: &[Vec<i16>]) -> [[i16; SEEDS]; 2] {
    let mut out = [[0i16; SEEDS]; 2];
    for (channel, group) in source.iter().zip(out.iter_mut()) {
        for (i, slot) in group.iter_mut().skip(1).enumerate() {
            *slot = channel.get(i).copied().unwrap_or(0);
        }
    }
    out
}

/// A donor record with everything the audio decides written over it.
///
/// The length marks scale with the stroke's length so that they stay inside it. A
/// release stroke declares no marks and zeroes the decay coefficient at `+0x2e`,
/// keeping the fourteen-entry ladder at `+0x36` exactly as the donor carries it, as a
/// stroke of any other bank does. The block index at `+0x2c` is derived: it is the
/// block holding the first mark.
fn record(
    donor: &[u8; RECORD],
    coded: &Coded,
    bank: u8,
    layer: u8,
    seeds: &[[i16; SEEDS]; 2],
    id: u32,
) -> Result<[u8; RECORD], Error> {
    let owned = u32::try_from(coded.owned).map_err(|_| ParseError::OutOfBounds {
        value: format!("{} frames", coded.owned),
        bound: "the u32 frame count a stroke record holds".into(),
    })?;
    let blocks = u16::try_from(coded.starts.len()).map_err(|_| ParseError::OutOfBounds {
        value: format!("{} blocks", coded.starts.len()),
        bound: "the u16 block count a stroke record holds".into(),
    })?;

    let mut out = *donor;
    out[REC_START..REC_START + 4].fill(0);
    out[REC_BANK] = bank;
    out[REC_LAYER] = layer;
    out[REC_FRAMES..REC_FRAMES + 4].copy_from_slice(&owned.to_be_bytes());
    out[REC_BLOCKS..REC_BLOCKS + 2].copy_from_slice(&blocks.to_be_bytes());
    for (channel, group) in seeds.iter().enumerate() {
        for (i, seed) in group.iter().enumerate() {
            let at = REC_SEEDS + (channel * SEEDS + i) * 2;
            out[at..at + 2].copy_from_slice(&seed.to_be_bytes());
        }
    }

    let silent = bank == Bank::Release.code();
    let donor_frames = be32(donor, REC_FRAMES);
    let mut first = 0u32;
    for mark in 0..MARKS {
        let at = REC_MARKS + mark * 4;
        let scaled = match (silent, donor_frames) {
            (true, _) | (_, 0) => 0,
            _ => rescale(be32(donor, at), owned, donor_frames),
        };
        out[at..at + 4].copy_from_slice(&scaled.to_be_bytes());
        if mark == 0 {
            first = scaled;
        }
    }
    let holding = coded
        .starts
        .iter()
        .rposition(|&start| start as u64 <= u64::from(first))
        .unwrap_or(0);
    out[REC_MARK_BLOCK..REC_MARK_BLOCK + 2].copy_from_slice(&(holding as u16).to_be_bytes());
    if silent {
        out[REC_DECAY..REC_DECAY + 4].fill(0);
    }
    out[REC_ID..REC_ID + 4].copy_from_slice(&id.to_be_bytes());
    Ok(out)
}

/// A mark at the same place in a stroke of `owned` frames, and inside it.
fn rescale(mark: u32, owned: u32, donor_frames: u32) -> u32 {
    let moved = u64::from(mark) * u64::from(owned) / u64::from(donor_frames);
    moved.min(u64::from(owned.saturating_sub(1))) as u32
}

/// How `coded` compares with the span it was coded from, block by block.
fn compare(before: &[u8], coded: &[u8], block: usize) -> Recoded {
    let mut out = Recoded {
        blocks: coded.len() / block,
        ..Recoded::default()
    };
    for (index, now) in coded.chunks_exact(block).enumerate() {
        let Some(was) = before.get(index * block..(index + 1) * block) else {
            continue;
        };
        if was == now {
            out.identical += 1;
        } else if was[1..] == now[1..] {
            out.restated += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbin::{Cbin, Header, RawBody};
    use crate::formats::npno::{be16, Piano, CNSP_MAGIC, DECAYS, FORMAT, REC_DECAYS};

    /// A one-stroke library the encoder can donate from: a real prefix and one real
    /// record, holding marks and a full ladder of decay coefficients a new stroke
    /// inherits.
    fn template(channels: u16) -> Piano {
        let block = block_bytes(channels);
        let directory_end = super::super::DIRECTORY_AT + RECORD;
        let first = super::super::first_audio_offset(directory_end, block).unwrap();
        let mut body = vec![0u8; first + block];
        body[..4].copy_from_slice(CNSP_MAGIC);
        body[0x04..0x06].copy_from_slice(&0x450u16.to_be_bytes());
        body[0x61c..0x61e].copy_from_slice(&0x450u16.to_be_bytes());
        body[0x61e..0x620].copy_from_slice(&channels.to_be_bytes());
        body[0x1c..0x1c + 9].copy_from_slice(b"Donor#Med");
        body[KEY_MAP_AT..KEY_MAP_AT + NOTES].fill(UNCOVERED);
        body[KEY_MAP_AT + 60] = 60;
        body[0x620..0x622].copy_from_slice(&1u16.to_be_bytes());
        body[0x622 + 60 * 2..0x622 + 60 * 2 + 2].copy_from_slice(&1u16.to_be_bytes());

        let rec = super::super::DIRECTORY_AT;
        body[rec..rec + 4].copy_from_slice(&(first as u32).to_be_bytes());
        body[rec + REC_FRAMES..rec + REC_FRAMES + 4].copy_from_slice(&1000u32.to_be_bytes());
        body[rec + REC_BLOCKS..rec + REC_BLOCKS + 2].copy_from_slice(&1u16.to_be_bytes());
        for mark in 0..MARKS {
            let at = rec + REC_MARKS + mark * 4;
            body[at..at + 4].copy_from_slice(&((mark as u32 + 6) * 100).to_be_bytes());
        }
        body[rec + REC_DECAY..rec + REC_DECAY + 4].copy_from_slice(&0x0000_2000u32.to_be_bytes());
        for coefficient in 0..DECAYS {
            let at = rec + REC_DECAYS + coefficient * 4;
            let value = 0x0000_1000u32 + coefficient as u32;
            body[at..at + 4].copy_from_slice(&value.to_be_bytes());
        }
        body[rec + REC_ID..rec + REC_ID + 4].copy_from_slice(&77u32.to_be_bytes());
        // One block of order-0 width-16 silence, so the stroke reads back.
        let audio = first;
        body[audio..audio + 2].copy_from_slice(&0x6410u16.to_be_bytes());
        let frames = codec::block_frames(16, block, usize::from(channels));
        let owned = (frames - OVERLAP) as u32;
        body[rec + REC_FRAMES..rec + REC_FRAMES + 4].copy_from_slice(&owned.to_be_bytes());

        Piano {
            file: Cbin {
                header: Header::new(FORMAT, (0, 0), 530),
                body: RawBody(body),
            },
        }
    }

    /// A decaying tone, which is the shape the coder's width search is built for.
    fn tone(frames: usize, hertz: f64, channels: usize) -> Vec<Vec<i16>> {
        (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|n| {
                        let t = n as f64 / f64::from(codec::RATE);
                        let envelope = (-3.0 * t).exp() * (1.0 - (-400.0 * t).exp());
                        let phase = std::f64::consts::TAU * hertz * (c as f64 * 0.01 + 1.0) * t;
                        (9000.0 * envelope * phase.sin()) as i16
                    })
                    .collect()
            })
            .collect()
    }

    fn one(root: u8, bank: Bank, layer: u8, channels: Vec<Vec<i16>>) -> Recording {
        Recording {
            root,
            bank,
            layer,
            channels,
        }
    }

    /// Build, write, read back, and decode: what the caller put in is what the
    /// instrument would be handed.
    fn round_trip(channels: u16, recordings: &[Recording]) -> Piano {
        let donor = template(channels);
        let built = build(
            &Donor::Template(&donor.library().unwrap()),
            &Options::new("Synth").variant("Test"),
            recordings,
        )
        .unwrap();
        let bytes = {
            let piano = built.to_piano().unwrap();
            let mut out = std::io::Cursor::new(Vec::new());
            piano.write_to(&mut out).unwrap();
            out.into_inner()
        };
        Piano::read_from(&mut std::io::Cursor::new(bytes)).unwrap()
    }

    /// A stroke holds whole blocks and every frame of its recording: what it states
    /// past the recording is silence, and the recording itself comes back sample for
    /// sample.
    #[test]
    fn a_built_library_decodes_back_to_the_frames_it_was_given() {
        let source = tone(20_000, 220.0, 2);
        let piano = round_trip(2, &[one(60, Bank::Attack, 0, source.clone())]);
        let library = piano.library().unwrap();
        assert_eq!(library.name(), ("Synth".into(), "Test".into()));
        assert_eq!(library.channels(), 2);

        let stroke = &library.strokes()[0];
        let audio = codec::decode(stroke, 2).unwrap();
        assert_eq!(audio.clipped, 0);
        let longest = codec::block_frames(MIN_WIDTH, library.block_bytes(), 2) - OVERLAP;
        let padding = audio
            .frames()
            .checked_sub(source[0].len())
            .unwrap_or_else(|| {
                panic!(
                    "the stroke states {} frames of a {} frame recording",
                    audio.frames(),
                    source[0].len()
                )
            });
        assert!(
            padding < longest,
            "the stroke states {padding} frames of silence, a whole block or more"
        );
        for (channel, given) in audio.channels.iter().zip(&source) {
            assert_eq!(&channel[..given.len()], &given[..]);
            assert!(channel[given.len()..].iter().all(|&s| s == 0));
        }
    }

    /// Nothing a recording holds is dropped for falling between block lengths: the
    /// coder states the silence that fills out the last block rather than fewer frames
    /// than it was given, including where the whole signal sits in the frames a
    /// truncating coder would leave off. The stroke it writes is one a rebuild leaves
    /// alone.
    #[test]
    fn a_recording_that_does_not_fill_its_last_block_keeps_every_frame() {
        let mut late = vec![vec![0i16; 892]; 2];
        for (channel, lane) in late.iter_mut().enumerate() {
            let signal = tone(64, 262.0, 2);
            lane[892 - 64..].copy_from_slice(&signal[channel]);
        }
        let cases: [(&str, Vec<Vec<i16>>); 3] = [
            ("a recording a block and a half long", tone(700, 262.0, 2)),
            (
                "a recording cut between block lengths",
                tone(9_133, 440.0, 2),
            ),
            ("a recording whose signal is all at the end", late),
        ];

        for (what, source) in cases {
            let frames = source[0].len();
            let signal: i64 = source[0].iter().map(|&s| i64::from(s).abs()).sum();
            assert!(signal > 0, "{what}: the case states no signal");
            let piano = round_trip(2, &[one(60, Bank::Attack, 0, source.clone())]);
            let library = piano.library().unwrap();
            let audio = codec::decode(&library.strokes()[0], 2).unwrap();
            assert!(
                audio.frames() >= frames,
                "{what}: the stroke states {} of {frames} frames",
                audio.frames()
            );
            for (channel, given) in audio.channels.iter().zip(&source) {
                assert_eq!(
                    &channel[..frames],
                    &given[..],
                    "{what}: frames came back changed"
                );
                assert!(
                    channel[frames..].iter().all(|&s| s == 0),
                    "{what}: the stroke states something other than silence past the recording"
                );
            }

            let again = rebuild(&library).unwrap();
            for recoded in &again.strokes {
                assert_eq!(
                    (recoded.identical, recoded.recoded()),
                    (recoded.blocks, 0),
                    "{what}: the rebuild laid the stroke out differently"
                );
            }
            assert_eq!(
                again.library.to_body().unwrap(),
                piano.file.body.0,
                "{what}: the rebuild is a different file"
            );
        }
    }

    /// The two claims above hold wherever a recording ends against the block grid, not
    /// only at the lengths a case picks: the stroke holds every frame, and coding it
    /// again reaches the same file.
    #[test]
    fn a_recording_of_any_length_codes_to_a_stroke_that_holds_it() {
        for frames in [
            1, 63, 64, 65, 445, 446, 447, 509, 891, 892, 893, 1_102, 2_658,
        ] {
            for channels in [1u16, 2] {
                let source = tone(frames, 262.0, usize::from(channels));
                let piano = round_trip(channels, &[one(60, Bank::Attack, 0, source.clone())]);
                let library = piano.library().unwrap();
                let audio = codec::decode(&library.strokes()[0], channels).unwrap();
                let what = format!("{frames} frame(s) over {channels} channel(s)");
                assert!(
                    audio.frames() >= frames,
                    "{what}: the stroke states {}",
                    audio.frames()
                );
                for (channel, given) in audio.channels.iter().zip(&source) {
                    assert_eq!(&channel[..frames], &given[..], "{what}: frames changed");
                    assert!(
                        channel[frames..].iter().all(|&s| s == 0),
                        "{what}: not silent"
                    );
                }
                assert_eq!(
                    rebuild(&library).unwrap().library.to_body().unwrap(),
                    piano.file.body.0,
                    "{what}: the rebuild is a different file"
                );
            }
        }
    }

    /// A stroke that saturates the decoder is refused rather than coded again: what a
    /// recode would write is the clamped reconstruction, which is audio the file it
    /// came from does not hold.
    #[test]
    fn a_stroke_whose_decode_saturates_is_not_coded_again() {
        let donor = template(1);
        let mut library = donor.library().unwrap();
        let block = library.block_bytes();
        let frames = codec::block_frames(MAX_WIDTH, block, 1);
        // Order one integrates its residuals, so a block of one large value runs the
        // reconstruction off the top of int16 within a few frames.
        let mut audio = Vec::new();
        pack(&mut audio, 1, MAX_WIDTH, 0, &vec![20_000i32; frames], block);
        library.strokes[0].audio = Cow::Owned(audio);

        let decoded = codec::decode(&library.strokes()[0], 1).unwrap();
        assert!(decoded.clipped > 0, "the case does not saturate");

        let error = match rebuild(&library) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("expected a refusal"),
        };
        assert!(error.contains("left int16"), "{error}");
    }

    #[test]
    fn a_mono_library_codes_and_decodes_on_its_own_block_size() {
        let source = tone(9_000, 440.0, 1);
        let piano = round_trip(1, &[one(48, Bank::Attack, 0, source.clone())]);
        let library = piano.library().unwrap();
        assert_eq!(library.channels(), 1);
        let audio = codec::decode(&library.strokes()[0], 1).unwrap();
        assert_eq!(audio.channels[0][..source[0].len()], source[0][..]);
        assert!(audio.channels[0][source[0].len()..].iter().all(|&s| s == 0));
    }

    #[test]
    fn the_directory_orders_strokes_by_root_then_bank_then_layer() {
        let short = tone(6_000, 300.0, 1);
        let piano = round_trip(
            1,
            &[
                one(72, Bank::Attack, 0, short.clone()),
                one(60, Bank::Release, 0, short.clone()),
                one(60, Bank::Attack, 4, short.clone()),
                one(60, Bank::Attack, 0, short.clone()),
            ],
        );
        let library = piano.library().unwrap();
        let seen: Vec<(u8, Option<Bank>, u8)> = library
            .strokes()
            .iter()
            .map(|s| (s.root, s.bank(), s.layer()))
            .collect();
        assert_eq!(
            seen,
            [
                (60, Some(Bank::Attack), 0),
                (60, Some(Bank::Attack), 4),
                (60, Some(Bank::Release), 0),
                (72, Some(Bank::Attack), 0),
            ]
        );
    }

    /// The default spread is what decides which velocities reach which layer, so the
    /// values it produces are the contract, not an implementation detail.
    #[test]
    fn the_default_layer_values_spread_a_root_over_the_selection_range() {
        let spread = |layers| {
            (0..layers)
                .map(|i| layer_value(i, layers))
                .collect::<Vec<_>>()
        };
        assert_eq!(spread(1), vec![0], "a lone layer plays at every velocity");
        assert_eq!(spread(2), vec![0, 27]);
        assert_eq!(spread(3), vec![0, 14, 27]);
        assert_eq!(spread(9), vec![0, 3, 7, 10, 14, 17, 20, 24, 27]);
        assert_eq!(layer_value(9, 9), 27, "an index past the last is the last");
        assert_eq!(layer_value(0, 0), 0);
    }

    /// A layer value is a field of the record, not a position in the directory: a
    /// library that states its own keeps them when its audio is coded again.
    #[test]
    fn a_rebuild_keeps_the_layer_value_every_stroke_states() {
        let short = tone(6_000, 300.0, 1);
        let piano = round_trip(
            1,
            &[
                one(60, Bank::Attack, 0, short.clone()),
                one(60, Bank::Attack, 6, short.clone()),
                one(60, Bank::Attack, 12, short),
            ],
        );
        let library = piano.library().unwrap();
        let again = rebuild(&library).unwrap();
        let values: Vec<u8> = again.library.strokes().iter().map(|s| s.layer()).collect();
        assert_eq!(values, [0, 6, 12]);
    }

    /// Only the coefficient at `+0x2e` answers to the bank: a release stroke built
    /// from a donor that carries all fifteen zeroes that one and keeps the donor's
    /// fourteen-entry ladder, as a stroke of any other bank does.
    #[test]
    fn a_release_stroke_declares_no_marks_and_zeroes_one_decay_coefficient() {
        let short = tone(6_000, 300.0, 1);
        let piano = round_trip(
            1,
            &[
                one(60, Bank::Attack, 0, short.clone()),
                one(60, Bank::Release, 0, short.clone()),
            ],
        );
        let library = piano.library().unwrap();
        let donated: Vec<u32> = (0..DECAYS).map(|c| 0x0000_1000u32 + c as u32).collect();
        for stroke in library.strokes() {
            let record = stroke.record();
            let marks: Vec<u32> = (0..MARKS)
                .map(|m| be32(record, REC_MARKS + m * 4))
                .collect();
            let ladder: Vec<u32> = (0..DECAYS)
                .map(|c| be32(record, REC_DECAYS + c * 4))
                .collect();
            assert_eq!(
                ladder, donated,
                "a stroke of any bank inherits the donor's ladder unchanged"
            );
            if stroke.bank() == Some(Bank::Release) {
                assert_eq!(marks, [0; MARKS], "a release stroke declares no marks");
                assert_eq!(
                    be32(record, REC_DECAY),
                    0,
                    "a release stroke zeroes the coefficient at +0x2e"
                );
            } else {
                assert!(marks.iter().all(|&m| m > 0 && m < stroke.frames()));
                assert_ne!(
                    be32(record, REC_DECAY),
                    0,
                    "a stroke of another bank inherits the donor's coefficient at +0x2e"
                );
            }
        }
    }

    /// Building without a template needs no library to donate anything, and what comes
    /// out reads back: the kind, the gain and the damper limit the rules state, a
    /// stroke trimmed by its own layer value with no decay applied over it, and
    /// identifiers counting the directory. A library that has been played holds these
    /// bytes; the corpus suite is where that comparison is made.
    #[test]
    fn a_library_built_from_rules_states_them_and_needs_no_template() {
        let short = tone(6_000, 300.0, 1);
        let rules = Rules {
            kind: Kind::Wurlitzer,
            gain: -20,
            damper_top: 97,
        };
        let built = build(
            &Donor::Rules(rules),
            &Options::new("Reeds"),
            &[
                one(60, Bank::Attack, 0, short.clone()),
                one(60, Bank::Attack, 17, short.clone()),
                one(60, Bank::Release, 0, short.clone()),
            ],
        )
        .unwrap();

        assert_eq!(built.prefix[KIND_AT], Kind::Wurlitzer.code());
        assert_eq!(built.prefix[GAIN_AT] as i8, -20);
        assert_eq!(built.prefix[DAMPER_TOP_AT], 97);
        assert_eq!(built.stream_version(), RULES_VERSION);
        assert_eq!(
            Rules::new(Kind::Wurlitzer).damper_top,
            rules.damper_top,
            "the kind names its own damper limit"
        );

        let stated: Vec<(u8, u16, u16, u32)> = built
            .strokes()
            .iter()
            .map(|s| {
                let record = s.record();
                (
                    s.layer(),
                    be16(record, REC_WINDOW),
                    be16(record, REC_TRIM),
                    be32(record, REC_ID),
                )
            })
            .collect();
        assert_eq!(stated, [(0, 0, 3, 1), (17, 17, 20, 2), (0, 0, 12, 3)]);
        for stroke in built.strokes() {
            let record = stroke.record();
            assert!((0..MARKS).all(|m| be32(record, REC_MARKS + m * 4) == 0));
            assert!((0..DECAYS).all(|c| be32(record, REC_DECAYS + c * 4) == LADDER_UNITY));
        }

        let again = rebuild(&built).unwrap();
        assert_eq!(
            again.library.to_body().unwrap(),
            built.to_body().unwrap(),
            "a rule-written library is not a fixed point of a recode"
        );
    }

    /// The damper cut is flat over the lowest notes, falls straight to the highest key
    /// an instrument plays, and states one value past it.
    #[test]
    fn the_damper_cut_is_a_plateau_then_a_straight_fall_to_the_top_key() {
        let curve: Vec<u8> = (0..NOTES).map(damper_cut).collect();
        assert_eq!(curve[..25], [79; 25]);
        assert_eq!((curve[66], curve[108], curve[109]), (50, 20, 30));
        assert!(
            curve[24..=108].windows(2).all(|w| w[0] >= w[1]),
            "the fall never rises"
        );
        assert!(curve[109..].iter().all(|&v| v == 30));
    }

    /// The bound is the selection rule at the softest note-on, so the value it names
    /// is the last one a key can reach and the spread stays inside it.
    #[test]
    fn the_highest_played_layer_is_the_rule_at_the_softest_velocity() {
        let selected = |velocity: u32| ((127 - velocity) * 31 / 127) as u8;
        assert_eq!(HIGHEST_PLAYED_LAYER, selected(1));
        assert!((1..=127).all(|v| selected(v) <= HIGHEST_PLAYED_LAYER));
        const { assert!(SOFTEST_LAYER <= HIGHEST_PLAYED_LAYER) };

        let donor = template(1);
        let library = donor.library().unwrap();
        let short = tone(6_000, 300.0, 1);
        build(
            &Donor::Template(&library),
            &Options::new("Synth"),
            &[one(60, Bank::Attack, HIGHEST_PLAYED_LAYER, short)],
        )
        .expect("the bound itself is a value a key sounds");
    }

    #[test]
    fn every_key_up_to_the_highest_roots_own_plays_the_root_above_it() {
        let roots: BTreeSet<u8> = [25, 30, 60].into_iter().collect();
        let map = key_map(&roots);
        assert_eq!(map[0], 25, "the lowest root takes everything under it");
        assert_eq!(map[26], 25, "a key one semitone above its root");
        assert_eq!(map[27], 30, "and the next one belongs to the root above");
        assert_eq!(map[31], 30, "a root reaches one semitone above itself");
        assert_eq!(map[32], 60, "and the key after that is the next root's");
        assert_eq!(map[61], 60, "the highest root reaches one semitone up");
        assert_eq!(map[62], UNCOVERED);
        assert_eq!(map[NOTES - 1], UNCOVERED);
    }

    #[test]
    fn the_attenuation_states_decibels_below_full_scale() {
        assert_eq!(attenuation(8192), 0);
        assert_eq!(
            attenuation(9000),
            0,
            "louder than full scale clamps at zero"
        );
        assert_eq!(attenuation(819), 20);
        assert_eq!(attenuation(82), 40);
        assert_eq!(attenuation(1), 78);
        assert_eq!(attenuation(0), 100, "silence is not 78 dB down");
    }

    #[test]
    fn a_recording_a_library_cannot_state_is_refused_by_name() {
        let donor = template(1);
        let library = donor.library().unwrap();
        let options = Options::new("Synth");
        let short = tone(6_000, 300.0, 1);
        let error = |recordings: &[Recording]| {
            build(&Donor::Template(&library), &options, recordings)
                .expect_err("expected a refusal")
                .to_string()
        };

        assert!(error(&[]).contains("no recordings"));
        assert!(error(&[one(60, Bank::Attack, 0, vec![])]).contains("1 or 2"));
        assert!(error(&[one(60, Bank::Attack, 0, vec![vec![]])]).contains("no frames"));
        assert!(
            error(&[one(60, Bank::Attack, 0, vec![short[0].clone(), vec![0; 3]])])
                .contains("unequal length")
        );
        assert!(error(&[
            one(60, Bank::Attack, 0, short.clone()),
            one(60, Bank::Attack, 0, short.clone()),
        ])
        .contains("recorded twice"));
        let unplayable = error(&[one(
            60,
            Bank::Attack,
            HIGHEST_PLAYED_LAYER + 1,
            short.clone(),
        )]);
        assert!(unplayable.contains("no velocity selects"), "{unplayable}");
        assert!(unplayable.contains("30"), "{unplayable}");
        assert!(error(&[
            one(60, Bank::Attack, 0, short.clone()),
            one(
                60,
                Bank::Attack,
                1,
                vec![short[0].clone(), short[0].clone()]
            ),
        ])
        .contains("one channel count"));
    }

    #[test]
    fn a_template_with_only_release_strokes_cannot_donate_to_an_attack_stroke() {
        let donor = template(1);
        let mut library = donor.library().unwrap();
        library.strokes[0].record[REC_BANK] = Bank::Release.code();
        let short = tone(6_000, 300.0, 1);
        let error = build(
            &Donor::Template(&library),
            &Options::new("Synth"),
            &[one(60, Bank::Attack, 0, short)],
        )
        .expect_err("expected a refusal")
        .to_string();
        assert!(
            error.contains("length marks and decay coefficients"),
            "{error}"
        );
    }

    /// A recording followed by digital silence, which is what a take trimmed to a
    /// fixed length holds and what a release stroke is mostly made of.
    fn trailing_silence(frames: usize, silence: usize, channels: usize) -> Vec<Vec<i16>> {
        let mut source = tone(frames, 262.0, channels);
        for channel in &mut source {
            channel.resize(frames + silence, 0);
        }
        source
    }

    /// A library this module writes is one [`rebuild`] leaves alone: every block comes
    /// back byte for byte and so does the container around it. A stroke states the
    /// frames its blocks own and nothing past them, which is what leaves the width
    /// search the same room the second time.
    ///
    /// The lengths put the recording's end in each place it falls against the block
    /// grid: inside a single block, part-way down a decay, and after the recording has
    /// already reached digital silence.
    #[test]
    fn rebuilding_a_library_this_module_wrote_reproduces_every_block() {
        let cases: [(&str, u16, Vec<Recording>); 4] = [
            (
                "a source shorter than one block",
                2,
                vec![one(60, Bank::Attack, 0, tone(509, 262.0, 2))],
            ),
            (
                "two stereo strokes cut mid-decay",
                2,
                vec![
                    one(60, Bank::Attack, 0, tone(12_000, 262.0, 2)),
                    one(72, Bank::Attack, 0, tone(9_000, 523.0, 2)),
                ],
            ),
            (
                "a mono stroke cut mid-decay",
                1,
                vec![one(48, Bank::Attack, 0, tone(9_133, 440.0, 1))],
            ),
            (
                "a stroke that reaches silence before its source ends",
                2,
                vec![one(60, Bank::Attack, 0, trailing_silence(6_000, 5_000, 2))],
            ),
        ];

        for (what, channels, recordings) in cases {
            let piano = round_trip(channels, &recordings);
            let library = piano.library().unwrap();
            let again = rebuild(&library).unwrap();
            assert_eq!(again.strokes.len(), recordings.len());
            for (index, recoded) in again.strokes.iter().enumerate() {
                assert_eq!(
                    (recoded.identical, recoded.recoded()),
                    (recoded.blocks, 0),
                    "{what}: stroke {index} came back with different blocks"
                );
            }
            assert_eq!(
                again.library.to_body().unwrap(),
                piano.file.body.0,
                "{what}: the library came back a different file"
            );
        }
    }

    #[test]
    fn the_resampler_leaves_audio_already_on_the_lattice_alone() {
        let interleaved: Vec<i16> = (0..64).map(|n| (n * 100 - 3000) as i16).collect();
        let out = resample(&interleaved, 2, codec::RATE).unwrap();
        assert_eq!(out.clipped, 0);
        assert_eq!(out.channels[0][..3], [-3000, -2800, -2600]);
        assert_eq!(out.channels[1][..3], [-2900, -2700, -2500]);

        assert!(resample(&[1, 2, 3], 2, codec::RATE).is_err());
        assert!(resample(&[1, 2], 1, 0).is_err());
    }

    /// A source faster than the lattice is band-limited to the lattice's own Nyquist
    /// on the way down: a tone above it comes through as near silence rather than
    /// folded back into the band as a tone the recording never held, and one well
    /// inside the band comes through at its level.
    #[test]
    fn resampling_a_faster_source_drops_what_the_lattice_cannot_hold() {
        let rate = 96_000;
        let tone_at = |hertz: f64| -> Vec<i16> {
            (0..rate as usize / 4)
                .map(|n| {
                    let t = n as f64 / f64::from(rate);
                    (8000.0 * (std::f64::consts::TAU * hertz * t).sin()) as i16
                })
                .collect()
        };
        // The kernel rings in and out at the ends, so the level is read off the middle.
        let peak = |lane: &[i16]| {
            lane[400..lane.len() - 400]
                .iter()
                .map(|&s| i32::from(s).abs())
                .max()
                .unwrap_or(0)
        };

        let above = resample(&tone_at(24_000.0), 1, rate).unwrap();
        assert_eq!(above.clipped, 0);
        let level = peak(&above.channels[0]);
        assert!(level < 400, "a 24 kHz tone came through at {level} of 8000");

        let inside = resample(&tone_at(1_000.0), 1, rate).unwrap();
        let level = peak(&inside.channels[0]);
        assert!(
            level > 7_900,
            "a 1 kHz tone came through at {level} of 8000"
        );
    }

    #[test]
    fn resampling_a_slower_source_stretches_it_onto_the_lattice() {
        let rate = 22_050;
        let frames = 4_000;
        let source: Vec<i16> = (0..frames)
            .map(|n| {
                let t = n as f64 / f64::from(rate);
                (8000.0 * (std::f64::consts::TAU * 100.0 * t).sin()) as i16
            })
            .collect();
        let out = resample(&source, 1, rate).unwrap();
        assert_eq!(
            out.channels[0].len(),
            frames * codec::RATE as usize / rate as usize
        );
        // A 100 Hz sine keeps its zero crossings, so the resampled lattice holds the
        // same count of them over the same span of time.
        let crossings = |signal: &[i16]| {
            signal
                .windows(2)
                .filter(|w| (w[0] < 0) != (w[1] < 0))
                .count()
        };
        assert_eq!(
            crossings(&out.channels[0][100..]),
            crossings(&source[100..])
        );
    }
}
