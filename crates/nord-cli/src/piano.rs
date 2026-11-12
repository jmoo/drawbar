//! `nord piano`: the verbs that apply only to a piano library file.
//!
//! `edit`, `trim` and `split` copy every stroke they keep byte for byte. Only `build`
//! and `rebuild` encode audio.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::builder::{PossibleValue, PossibleValuesParser, TypedValueParser};
use clap::{Args, ValueEnum};
use nord_format::formats::npno::encode::{parse_stroke_name, Clash, LayerTag, Stem};
use nord_format::formats::npno::{
    self, codec, encode, Bank, Change, Layers, Library, Version, UNCOVERED,
};
use nord_format::note;
use nord_format::take::clean::{self, Noise};
use nord_format::take::layers::TAKE_PEAK_DB;
use nord_format::wav;
use nord_format::Entity;

use crate::edit::{write_edit, write_file};
use crate::ui::Ui;

/// A bank named as [`Bank::name`] spells it, from the names `offered`.
fn bank_parser(
    offered: impl IntoIterator<Item = PossibleValue>,
) -> impl TypedValueParser<Value = Bank> {
    PossibleValuesParser::new(offered).try_map(|name| {
        Bank::ALL
            .into_iter()
            .find(|bank| bank.name() == name)
            .ok_or_else(|| format!("{name} is not a bank"))
    })
}

fn any_bank() -> impl TypedValueParser<Value = Bank> {
    bank_parser(Bank::ALL.map(|bank| PossibleValue::new(bank.name())))
}

/// The banks a trim can drop. Dropping the attack bank would leave silence, so it is
/// not offered.
fn droppable_bank() -> impl TypedValueParser<Value = Bank> {
    bank_parser([
        PossibleValue::new(Bank::Resonance.name())
            .help("The pedal-down resonance set, which the larger libraries add"),
        PossibleValue::new(Bank::Release.name()).help("The note-off sample"),
    ])
}

#[derive(Args)]
pub struct InspectArgs {
    /// The piano libraries to report on.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,

    /// List every stroke instead of one line per root.
    #[arg(long)]
    pub strokes: bool,

    /// List every covered key with the root it plays and its fine tune.
    #[arg(long)]
    pub keys: bool,
}

#[derive(Args)]
pub struct DecodeArgs {
    /// The piano library to read.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,

    /// The stroke's index in the directory. An alternative to `--key`.
    #[arg(long, value_name = "N", conflicts_with = "key")]
    pub stroke: Option<usize>,

    /// A key the library covers: a name (`C4`, `F#3`) or 0-127. Narrow the strokes
    /// it selects with `--layer` and `--bank`.
    #[arg(long, value_name = "KEY")]
    pub key: Option<String>,

    /// Velocity layer, 0 being the loudest recording of the root.
    #[arg(long, value_name = "N", requires = "key")]
    pub layer: Option<u8>,

    #[arg(long, value_name = "BANK", value_parser = any_bank(), requires = "key")]
    pub bank: Option<Bank>,

    /// Where to write the WAV. Frames come out at the rate the instrument plays
    /// them, with no gain applied.
    #[arg(short, long, value_name = "WAV")]
    pub out: PathBuf,
}

#[derive(Args)]
pub struct EditArgs {
    /// The piano library to edit.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,

    /// Rename the library. The name shares a fixed-width field with the variant and
    /// is split from it on a `#`, so a name holding one is refused.
    #[arg(long)]
    pub name: Option<String>,

    /// Replace the variant: the text after the `#`, where the vendor records the
    /// voicing and the library's size. It cannot contain a `#`.
    #[arg(long)]
    pub variant: Option<String>,

    /// Replace the voicing, a separate field that only the newer stream version has.
    #[arg(long)]
    pub voicing: Option<String>,

    /// `KEY=UNITS`, repeatable: retune one key. A bare number is fine-tune units;
    /// a number suffixed with `c` is cents, rounded to the nearest unit.
    #[arg(long = "tune", value_name = "KEY=UNITS")]
    pub tune: Vec<String>,

    /// `KEY=ROOT`, repeatable: play KEY with ROOT's strokes. `KEY=-` uncovers the
    /// key. Both sides take a note name or a number.
    #[arg(long = "map", value_name = "KEY=ROOT")]
    pub map: Vec<String>,

    /// Write the edit here instead of over the input file.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Confirm the write. Editing a file in place needs it.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args)]
pub struct TrimArgs {
    /// The piano library to trim.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,

    /// Drop every stroke of one bank. Dropping the resonance set is what turns a
    /// large library into a small one.
    #[arg(long, value_name = "BANK", value_parser = droppable_bank())]
    pub drop_bank: Vec<Bank>,

    /// `N` keeps the N loudest layers of each root and bank; `=0,3,7` keeps only
    /// those layer values. The layers kept keep their numbers.
    #[arg(long, value_name = "N|=LIST")]
    pub layers: Option<String>,

    /// `LO..HI`, inclusive: uncover every key outside it and drop the roots nothing
    /// plays anymore. Both ends take a note name or a number.
    #[arg(long, value_name = "LO..HI")]
    pub range: Option<String>,

    /// Where to write the trimmed library.
    #[arg(short, long, value_name = "FILE")]
    pub out: PathBuf,
}

#[derive(Args)]
pub struct BuildArgs {
    /// A directory of WAVs, one per stroke, named `<root>-b<bank>-l<layer>.wav`:
    /// `060-b0-l00.wav` is MIDI note 60, the attack bank, the loudest layer. Banks are
    /// 0 attack, 1 pedal resonance and 2 release. `l00`, `l01`, … count from the
    /// loudest. Write `v12` in place of `l00` to set a layer's value directly; every
    /// layer of one root and bank must use the same form. Files that are not WAVs are
    /// skipped, and a WAV named any other way is refused.
    ///
    /// Or one WAV, a take, whose regions are the strokes: each region labeled as a
    /// stroke's WAV is named (`060-b0-l00`), with the `smpl` loop it holds, if any, as
    /// its loop. The take may be PCM of 8 to 32 bits or float; each region is rounded to
    /// 16 bits, as a 16-bit WAV of it would be. In a format 6 library without a
    /// template, a softer region is first raised to its root's loudest layer's peak,
    /// which keeps its detail and does not change how loud it plays.
    ///
    /// The `frames` column in the output counts the WAV's frames. A stroke pads them
    /// out to whole blocks, or ends with the block holding its loop's end.
    ///
    /// Several takes build one library, such as one take per velocity layer, each
    /// holding every note.
    #[arg(value_name = "DIR|TAKE", required = true, num_args = 1..)]
    pub input: Vec<PathBuf>,

    /// A library to take everything the audio does not decide from: the decay
    /// coefficients, per-note tables, playback parameters, format and the word at the
    /// start of the body. Each new stroke inherits from the template's stroke of the
    /// same bank and nearest root, and is trimmed by its own layer value.
    #[arg(long, value_name = "FILE")]
    pub template: Option<PathBuf>,

    /// The piano format to write. Format 6 states velocity on a finer scale, and format
    /// 5 is for instruments that do not play format 6. A template's format decides
    /// instead.
    #[arg(long, value_enum, default_value_t = FormatName::Six, conflicts_with = "template")]
    pub format: FormatName,

    /// The kind of instrument the library holds. It sets the damper limit and how the
    /// instrument files the library. Defaults to the kind the takes state, as drawbar's
    /// take editor saves it, and otherwise to grand.
    #[arg(long, value_enum, conflicts_with = "template")]
    pub kind: Option<KindName>,

    /// A gain over the whole library, in decibels. Stored in tenths, so the range is
    /// -12.8 to 12.7.
    #[arg(
        long,
        value_name = "DB",
        default_value_t = 5.0,
        conflicts_with = "template"
    )]
    pub gain: f64,

    /// The highest key the instrument damps at note-off; keys above it ring on. A note
    /// name or 0-127, defaulting to what `--kind` implies. Anything above the top key
    /// the instrument plays damps every key.
    #[arg(long, value_name = "KEY", conflicts_with = "template")]
    pub damper_top: Option<String>,

    /// How fast a held loop at middle C fades, in decibels per second, from the loop's
    /// start until the key is released. The rate rises up the keyboard as it does on
    /// the instrument's own pianos of `--kind`, which also sets the default. 0 holds
    /// every loop at its level.
    #[arg(long, value_name = "DB_PER_S", conflicts_with = "template")]
    pub loop_fade: Option<f64>,

    /// The library's name, the half of its name field before the `#`.
    #[arg(long)]
    pub name: String,

    /// The half after the `#`, where the vendor records the voicing and the size.
    #[arg(long, default_value = "")]
    pub variant: String,

    /// Where to write the library.
    #[arg(short, long, value_name = "FILE")]
    pub out: PathBuf,
}

/// The piano formats a build writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FormatName {
    #[value(name = "5")]
    Five,
    #[value(name = "6")]
    Six,
}

impl From<FormatName> for Version {
    fn from(format: FormatName) -> Version {
        match format {
            FormatName::Five => Version::V5,
            FormatName::Six => Version::V6,
        }
    }
}

/// The instruments a library can state it holds, at body `0x18`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum KindName {
    Grand,
    Upright,
    ElectricGrand,
    /// The tine electric pianos.
    ElectricPiano,
    /// The reed electric pianos.
    Wurlitzer,
    Clavinet,
    Harpsichord,
    Digital,
    /// The hybrid and ballad electric pianos.
    Hybrid,
    Mallet,
}

impl From<KindName> for encode::Kind {
    fn from(kind: KindName) -> encode::Kind {
        match kind {
            KindName::Grand => encode::Kind::Grand,
            KindName::Upright => encode::Kind::Upright,
            KindName::ElectricGrand => encode::Kind::ElectricGrand,
            KindName::ElectricPiano => encode::Kind::ElectricPiano,
            KindName::Wurlitzer => encode::Kind::Wurlitzer,
            KindName::Clavinet => encode::Kind::Clavinet,
            KindName::Harpsichord => encode::Kind::Harpsichord,
            KindName::Digital => encode::Kind::DigitalPiano,
            KindName::Hybrid => encode::Kind::Hybrid,
            KindName::Mallet => encode::Kind::Mallet,
        }
    }
}

#[derive(Args)]
pub struct RebuildArgs {
    /// The piano library to re-encode from its own audio.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,

    /// Where to write the result.
    #[arg(short, long, value_name = "FILE")]
    pub out: PathBuf,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// The piano libraries to check.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,

    /// Also decode every stroke, checking each block against the one before it.
    #[arg(long)]
    pub deep: bool,
}

#[derive(Args)]
pub struct SplitArgs {
    /// The piano library to split.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,

    /// The first key of the upper half: a note name or 0-127.
    #[arg(long, value_name = "KEY")]
    pub at: String,

    /// Directory for the two halves, named after the input.
    #[arg(short, long, value_name = "DIR")]
    pub out: PathBuf,
}

/// Read a file and parse it as a piano library, naming the format it turned out to
/// be when it is not one.
pub(crate) fn read(path: &Path) -> Result<(Vec<u8>, npno::Piano), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    match entity {
        Entity::Piano(piano) => Ok((bytes, piano)),
        other => Err(format!(
            "{}: a {} file, not a piano library ({})",
            path.display(),
            other.identity().format,
            npno::FORMAT,
        )),
    }
}

/// A trim or a split writes a new library. Overwriting its input would leave
/// nothing to compare against, and there is no flag to ask for that.
fn refuse_in_place(input: &Path, output: &Path) -> Result<(), String> {
    if crate::edit::same_file(input, output) {
        return Err(format!(
            "{} is the input; give -o another path",
            output.display()
        ));
    }
    Ok(())
}

fn to_bytes(library: &Library<'_>, path: &Path) -> Result<Vec<u8>, String> {
    library
        .to_piano()
        .and_then(|piano| nord_format::to_bytes(&Entity::Piano(piano)))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `KEY=VALUE`, with the key as a note.
fn split_pair<'a>(spec: &'a str, flag: &str) -> Result<(u8, &'a str), String> {
    let (key, value) = spec
        .split_once('=')
        .ok_or_else(|| format!("--{flag} takes KEY=VALUE, got {spec:?}"))?;
    Ok((note::parse(key)?, value.trim()))
}

pub fn inspect(ui: &Ui, args: InspectArgs) -> Result<(), String> {
    let heading = |path: &PathBuf| path.display().to_string();
    crate::file::report_each(ui, &args.files, "file(s) did not read", heading, |path| {
        inspect_one(ui, path, &args)
    })
}

fn inspect_one(ui: &Ui, path: &Path, args: &InspectArgs) -> Result<(), String> {
    let (bytes, piano) = read(path)?;
    let library = piano.library().map_err(|e| e.to_string())?;
    let (name, variant) = library.name();

    let covered = covered_keys(&library);
    let coverage = match (covered.first(), covered.last()) {
        (Some(&lo), Some(&hi)) => format!(
            "{}..{} ({} keys)",
            note::name(lo),
            note::name(hi),
            covered.len()
        ),
        _ => "none".to_string(),
    };
    let title = if variant.is_empty() {
        name
    } else {
        format!("{name} ({variant})")
    };
    ui.out(format!(
        "  {title}  format {} (stream {:#06x})  {} channel(s)  {} bytes",
        library.version().number(),
        library.stream_version(),
        library.channels(),
        bytes.len(),
    ));
    if let (Some(long), Some(voicing)) = (library.long_name(), library.voicing()) {
        ui.out(ui.dim(format!("  long name {long:?}, voicing {voicing:?}")));
    }
    ui.out(format!(
        "  {} stroke(s) over {} root(s); keys {coverage}",
        library.strokes().len(),
        library.roots().len(),
    ));

    let mut by_bank: BTreeMap<u8, usize> = BTreeMap::new();
    for stroke in library.strokes() {
        *by_bank.entry(stroke.bank_code()).or_default() += 1;
    }
    let banks: Vec<String> = by_bank
        .iter()
        .map(|(&code, n)| format!("{} {n}", bank_label(code)))
        .collect();
    ui.out(format!("  banks: {}", banks.join(", ")));

    if args.strokes {
        ui.out(ui.dim(format!(
            "  {:>5} {:>6} {:>9} {:>5} {:>9} {:>8}  keys",
            "index", "root", "bank", "layer", "frames", "seconds"
        )));
        for (index, stroke) in library.strokes().iter().enumerate() {
            ui.out(format!(
                "  {index:>5} {:>6} {:>9} {:>5} {:>9} {:>8.3}  {}",
                note::name(stroke.root),
                bank_label(stroke.bank_code()),
                stroke.layer(),
                stroke.frames(),
                f64::from(stroke.frames()) / f64::from(codec::RATE),
                key_span(&library.keys_for(stroke.root)),
            ));
        }
        return Ok(());
    }

    ui.out(ui.dim(format!(
        "  {:>6} {:>7} {:>7}  layers per bank",
        "root", "strokes", "keys"
    )));
    for root in library.roots() {
        let mine: Vec<_> = library
            .strokes()
            .iter()
            .filter(|s| s.root == root)
            .collect();
        let mut layers: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
        for stroke in &mine {
            layers
                .entry(stroke.bank_code())
                .or_default()
                .push(stroke.layer());
        }
        let detail: Vec<String> = layers
            .iter()
            .map(|(&code, values)| {
                format!(
                    "{} {}",
                    bank_label(code),
                    match (values.first(), values.last()) {
                        (Some(lo), Some(hi)) if lo != hi =>
                            format!("{lo}..{hi} ({})", values.len()),
                        (Some(lo), _) => format!("{lo}"),
                        _ => "none".into(),
                    }
                )
            })
            .collect();
        let keys = library.keys_for(root);
        ui.out(format!(
            "  {:>6} {:>7} {:>7}  {}",
            note::name(root),
            mine.len(),
            keys.len(),
            detail.join(", "),
        ));
    }
    let tuned: Vec<i8> = covered
        .iter()
        .map(|&k| library.fine_tune(k))
        .collect::<Result<Vec<i8>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|&units| units != 0)
        .collect();
    match (tuned.iter().min(), tuned.iter().max()) {
        (Some(&lo), Some(&hi)) => ui.out(format!(
            "  fine tune: {} of {} key(s), {lo:+} to {hi:+} units ({:+.1} to {:+.1} c)",
            tuned.len(),
            covered.len(),
            f32::from(lo) * npno::FINE_TUNE_CENTS_PER_UNIT,
            f32::from(hi) * npno::FINE_TUNE_CENTS_PER_UNIT,
        )),
        _ => ui.out("  fine tune: none"),
    }

    if args.keys {
        ui.out(ui.dim(format!(
            "  {:>6} {:>6} {:>7}  {}",
            "key", "root", "tune", "cents"
        )));
        for key in covered {
            let units = library.fine_tune(key).map_err(|e| e.to_string())?;
            let root = library
                .key_root(key)
                .map_err(|e| e.to_string())?
                .expect("a covered key names a root");
            ui.out(format!(
                "  {:>6} {:>6} {:>7} {:>+7.1}",
                note::name(key),
                note::name(root),
                format!("{units:+}"),
                f32::from(units) * npno::FINE_TUNE_CENTS_PER_UNIT,
            ));
        }
    }
    Ok(())
}

/// The keys the map routes somewhere, ascending.
fn covered_keys(library: &Library<'_>) -> Vec<u8> {
    library
        .key_map()
        .iter()
        .enumerate()
        .filter(|&(_, &root)| root != UNCOVERED)
        .map(|(key, _)| key as u8)
        .collect()
}

fn bank_label(code: u8) -> String {
    match Bank::from_code(code) {
        Some(bank) => bank.name().to_owned(),
        None => format!("bank{code}"),
    }
}

fn key_span(keys: &[u8]) -> String {
    match (keys.first(), keys.last()) {
        (Some(lo), Some(hi)) if lo != hi => format!("{}..{}", note::name(*lo), note::name(*hi)),
        (Some(lo), _) => note::name(*lo),
        _ => "-".into(),
    }
}

pub fn decode(ui: &Ui, args: DecodeArgs) -> Result<(), String> {
    let (_, piano) = read(&args.file)?;
    let library = piano.library().map_err(|e| e.to_string())?;

    let chosen = match (args.stroke, &args.key) {
        (Some(index), _) => {
            let stroke = library.strokes().get(index).ok_or_else(|| {
                format!(
                    "stroke {index} is outside 0..{}",
                    library.strokes().len().saturating_sub(1)
                )
            })?;
            (index, stroke)
        }
        (None, Some(spec)) => {
            let key = note::parse(spec)?;
            let root = library
                .key_root(key)
                .map_err(|e| format!("--key {spec}: {e}"))?
                .ok_or_else(|| format!("{} is not a key this library covers", note::name(key)))?;
            let matching: Vec<(usize, _)> = library
                .strokes()
                .iter()
                .enumerate()
                .filter(|(_, s)| s.root == root)
                .filter(|(_, s)| args.layer.is_none_or(|l| s.layer() == l))
                .filter(|(_, s)| args.bank.is_none_or(|b| s.bank_code() == b.code()))
                .collect();
            match matching.len() {
                0 => {
                    return Err(format!(
                        "no stroke plays {} with that bank and layer",
                        note::name(key)
                    ))
                }
                1 => matching[0],
                _ => {
                    let listing: Vec<String> = matching
                        .iter()
                        .map(|(i, s)| {
                            format!("{i}: {} layer {}", bank_label(s.bank_code()), s.layer())
                        })
                        .collect();
                    return Err(format!(
                        "{} selects {} strokes; narrow it with --bank/--layer, or name one \
                         with --stroke:\n  {}",
                        note::name(key),
                        matching.len(),
                        listing.join("\n  ")
                    ));
                }
            }
        }
        (None, None) => return Err("give --stroke N or --key KEY".into()),
    };

    let (index, stroke) = chosen;
    let audio = codec::decode(stroke, library.channels()).map_err(|e| e.to_string())?;
    let wav = nord_format::wav::pcm16(&audio.interleaved(), codec::RATE, library.channels())
        .map_err(|e| e.to_string())?;
    write_file(ui, &args.out, &wav)?;

    let peak = audio
        .lanes
        .iter()
        .flatten()
        .map(|s| i32::from(*s).abs())
        .max()
        .unwrap_or(0);
    ui.out(format!(
        "  stroke {index}  root {}  {}  layer {}  keys {}",
        note::name(stroke.root),
        bank_label(stroke.bank_code()),
        stroke.layer(),
        key_span(&library.keys_for(stroke.root)),
    ));
    ui.out(format!(
        "  {} frames, {:.3} s, {} channel(s) at {} Hz; peak {peak}, no gain applied",
        audio.frames(),
        audio.seconds(),
        library.channels(),
        codec::RATE,
    ));
    ui.out(ui.dim(format!(
        "  {} repeated sample(s) checked against the block before, {} block(s)",
        audio.overlap_checked,
        stroke.blocks(),
    )));
    Ok(())
}

pub fn edit(ui: &Ui, args: EditArgs) -> Result<(), String> {
    let (original, piano) = read(&args.file)?;
    let mut library = piano.library().map_err(|e| e.to_string())?;
    let mut changed = 0usize;

    if let Some(name) = &args.name {
        library.set_name(name).map_err(|e| format!("--name: {e}"))?;
        changed += 1;
    }
    if let Some(variant) = &args.variant {
        library
            .set_variant(variant)
            .map_err(|e| format!("--variant: {e}"))?;
        changed += 1;
    }
    if let Some(voicing) = &args.voicing {
        library
            .set_voicing(voicing)
            .map_err(|e| format!("--voicing: {e}"))?;
        changed += 1;
    }
    for spec in &args.tune {
        let (key, value) = split_pair(spec, "tune")?;
        let units = parse_tune(value)?;
        library
            .set_fine_tune(key, units)
            .map_err(|e| format!("--tune {spec}: {e}"))?;
        ui.out(format!(
            "  {} fine tune {units:+} ({:+.1} c)",
            note::name(key),
            f32::from(units) * npno::FINE_TUNE_CENTS_PER_UNIT
        ));
        changed += 1;
    }
    for spec in &args.map {
        let (key, value) = split_pair(spec, "map")?;
        let root = match value {
            "-" | "" => None,
            other => Some(note::parse(other)?),
        };
        library
            .set_key_root(key, root)
            .map_err(|e| format!("--map {spec}: {e}"))?;
        ui.out(format!(
            "  {} plays {}",
            note::name(key),
            root.map_or_else(|| "nothing".to_string(), note::name)
        ));
        changed += 1;
    }

    if changed == 0 {
        ui.note("no field changed; writing nothing");
        return Ok(());
    }

    let edited = to_bytes(&library, &args.file)?;
    write_edit(ui, &args.file, args.out, args.yes, &edited)?;
    ui.note(format!("{} bytes in, {} out", original.len(), edited.len()));
    Ok(())
}

/// `-4` is fine-tune units; `+2.1c` is cents, rounded to the nearest unit.
fn parse_tune(value: &str) -> Result<i8, String> {
    if let Some(cents) = value.strip_suffix(['c', 'C']) {
        // `nan` and `inf` parse, and rounding either gives 0, a tuning nobody asked
        // for. Refuse them instead.
        let cents: f32 = cents
            .parse()
            .ok()
            .filter(|c: &f32| c.is_finite())
            .ok_or_else(|| format!("{value:?} is not a number of cents"))?;
        let units = (cents / npno::FINE_TUNE_CENTS_PER_UNIT).round();
        return i8::try_from(units as i32)
            .map_err(|_| format!("{cents} c is more than the per-key fine tune reaches"));
    }
    value.parse().map_err(|_| {
        format!(
            "{value:?} is not a fine-tune unit count (-128 to 127), or cents \
                              with a `c` suffix"
        )
    })
}

pub fn trim(ui: &Ui, args: TrimArgs) -> Result<(), String> {
    refuse_in_place(&args.file, &args.out)?;
    let (original, piano) = read(&args.file)?;
    let mut library = piano.library().map_err(|e| e.to_string())?;
    if args.drop_bank.is_empty() && args.layers.is_none() && args.range.is_none() {
        return Err("nothing to trim; give --drop-bank, --layers or --range".into());
    }

    let mut total = Change::default();
    for &bank in &args.drop_bank {
        let present = library.strokes().iter().any(|s| s.bank() == Some(bank));
        if !present {
            return Err(format!("this library has no {bank} strokes to drop"));
        }
        let change = library.drop_bank(bank);
        ui.out(format!(
            "  dropped the {bank} bank: {} stroke(s)",
            change.strokes_removed
        ));
        total = add(total, change);
    }
    if let Some(spec) = &args.layers {
        let layers = parse_layers(spec)?;
        let change = library.keep_layers(&layers);
        ui.out(format!(
            "  kept {}: {} stroke(s) dropped",
            describe_layers(&layers),
            change.strokes_removed
        ));
        total = add(total, change);
    }
    if let Some(spec) = &args.range {
        let (lo, hi) = parse_range(spec)?;
        let change = library
            .cut_range(lo..=hi)
            .map_err(|e| format!("--range {spec}: {e}"))?;
        ui.out(format!(
            "  cut to {}..{}: {} key(s) uncovered, {} stroke(s) dropped",
            note::name(lo),
            note::name(hi),
            change.keys_uncovered,
            change.strokes_removed
        ));
        total = add(total, change);
    }

    if library.strokes().is_empty() {
        return Err("that would leave the library with no strokes at all".into());
    }
    if total.strokes_removed == 0 {
        ui.note("nothing matched; writing the library unchanged");
    }

    let trimmed = to_bytes(&library, &args.file)?;
    write_file(ui, &args.out, &trimmed)?;
    report_size(ui, original.len(), trimmed.len());
    if total.keys_uncovered > 0 {
        ui.note(format!(
            "{} key(s) are left silent, and {} root(s) were dropped",
            total.keys_uncovered, total.roots_removed
        ));
    }
    Ok(())
}

fn add(a: Change, b: Change) -> Change {
    Change {
        strokes_removed: a.strokes_removed + b.strokes_removed,
        roots_removed: a.roots_removed + b.roots_removed,
        keys_uncovered: a.keys_uncovered + b.keys_uncovered,
    }
}

fn report_size(ui: &Ui, before: usize, after: usize) {
    let percent = 100.0 * after as f64 / before.max(1) as f64;
    ui.note(format!(
        "{before} bytes in, {after} out ({percent:.1}% of the original)"
    ));
}

/// `N` keeps the N loudest; `=0,3,7` keeps only those layer values.
fn parse_layers(spec: &str) -> Result<Layers, String> {
    let spec = spec.trim();
    if let Some(list) = spec.strip_prefix('=') {
        let values: Result<BTreeSet<u8>, String> = list
            .split(',')
            .map(|v| {
                v.trim()
                    .parse::<u8>()
                    .map_err(|_| format!("{v:?} is not a layer number (0-255)"))
            })
            .collect();
        let values = values?;
        if values.is_empty() {
            return Err("--layers =LIST needs at least one layer".into());
        }
        return Ok(Layers::Only(values));
    }
    let n: usize = spec.parse().map_err(|_| {
        format!("--layers takes a count (`2`) or an explicit list (`=0,3,7`), got {spec:?}")
    })?;
    if n == 0 {
        return Err("--layers 0 would keep no stroke at all".into());
    }
    Ok(Layers::Loudest(n))
}

fn describe_layers(layers: &Layers) -> String {
    match layers {
        Layers::Loudest(n) => format!("the {n} loudest layer(s) of each root and bank"),
        Layers::Only(values) => format!(
            "layer(s) {}",
            values
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn parse_range(spec: &str) -> Result<(u8, u8), String> {
    let (lo, hi) = spec
        .split_once("..")
        .ok_or_else(|| format!("--range takes LO..HI, got {spec:?}"))?;
    let (lo, hi) = (note::parse(lo)?, note::parse(hi.trim_start_matches('='))?);
    if lo > hi {
        return Err(format!(
            "--range {spec}: {} is above {}",
            note::name(lo),
            note::name(hi)
        ));
    }
    Ok((lo, hi))
}

/// Rebuild each library from its model and compare, and with `--deep` decode every
/// stroke it holds.
pub fn verify(ui: &Ui, args: VerifyArgs) -> Result<(), String> {
    let mut total = Counted::default();
    let checked = crate::file::check_each(ui, &args.files, "file(s) did not check out", |path| {
        match verify_one(path, args.deep) {
            Ok(counted) => {
                total.strokes += counted.strokes;
                total.frames += counted.frames;
                total.overlap += counted.overlap;
                Ok(format!(
                    "ok     {} ({})",
                    path.display(),
                    counted.line(args.deep)
                ))
            }
            Err(line) => Err(format!(
                "{} {} ({line})",
                ui.danger("FAILED"),
                path.display()
            )),
        }
    });
    if args.deep {
        ui.note(format!(
            "{} stroke(s), {} frame(s) decoded, {} repeated sample(s) matched the block \
             before",
            total.strokes, total.frames, total.overlap
        ));
    }
    checked
}

#[derive(Default)]
struct Counted {
    strokes: usize,
    frames: usize,
    overlap: usize,
    bytes: usize,
}

impl Counted {
    fn line(&self, deep: bool) -> String {
        if deep {
            format!(
                "{} bytes, {} stroke(s), {} frame(s), {} repeated sample(s) matched",
                self.bytes, self.strokes, self.frames, self.overlap
            )
        } else {
            format!("{} bytes, {} stroke(s)", self.bytes, self.strokes)
        }
    }
}

fn verify_one(path: &Path, deep: bool) -> Result<Counted, String> {
    let (original, piano) = read(path)?;
    let library = piano.library().map_err(|e| e.to_string())?;
    let rebuilt = to_bytes(&library, path)?;
    if rebuilt != original {
        return Err(format!(
            "the rebuild differs at {}; in {} bytes, out {}",
            crate::file::first_difference(&rebuilt, &original),
            original.len(),
            rebuilt.len()
        ));
    }
    let mut counted = Counted {
        strokes: library.strokes().len(),
        bytes: original.len(),
        ..Counted::default()
    };
    if deep {
        for stroke in library.strokes() {
            let audio = codec::decode(stroke, library.channels())
                .map_err(|e| format!("{stroke:?}: {e}"))?;
            if audio.clipped > 0 {
                return Err(format!(
                    "{stroke:?}: {} sample(s) fell outside the 16-bit range",
                    audio.clipped
                ));
            }
            counted.frames += audio.frames();
            counted.overlap += audio.overlap_checked;
        }
    }
    Ok(counted)
}

pub fn split(ui: &Ui, args: SplitArgs) -> Result<(), String> {
    let (original, piano) = read(&args.file)?;
    let library = piano.library().map_err(|e| e.to_string())?;
    let at = note::parse(&args.at)?;
    let (low, high) = library
        .split_at(at)
        .map_err(|e| format!("--at {}: {e}", args.at))?;
    for (label, half) in [("low", &low), ("high", &high)] {
        if half.strokes().is_empty() {
            return Err(format!(
                "splitting at {} leaves the {label} half with no strokes",
                note::name(at)
            ));
        }
    }

    std::fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let stem = args
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "piano".to_string());
    for (label, half) in [("low", &low), ("high", &high)] {
        let path = args.out.join(format!("{stem} {label}.npno"));
        refuse_in_place(&args.file, &path)?;
        let bytes = to_bytes(half, &args.file)?;
        write_file(ui, &path, &bytes)?;
        let covered = covered_keys(half);
        ui.out(format!(
            "  {label}: {} stroke(s) over {} root(s), keys {}",
            half.strokes().len(),
            half.roots().len(),
            key_span(&covered),
        ));
        report_size(ui, original.len(), bytes.len());
    }
    ui.note("both halves keep the library's name; `nord piano edit --name` changes it");
    Ok(())
}

/// Where a stroke's audio is.
enum Source<'a> {
    /// A WAV of its own.
    File(PathBuf),
    /// A region of a take, with its frames as the build cleans them, the loop it holds
    /// and the frame its fade starts at, both counted from the region's start.
    Region {
        path: &'a Path,
        take: &'a wav::Audio,
        region: &'a wav::Region,
        audio: Vec<f32>,
        sample_loop: Option<wav::Loop>,
        fade: Option<u32>,
        /// Semitones the region is pitched by, to play a root the take does not hold.
        shift: i32,
    },
}

/// One stroke a build reads, and what its name says the stroke is.
struct StrokeFile<'a> {
    source: Source<'a>,
    root: u8,
    bank: Bank,
    layer: LayerTag,
}

impl StrokeFile<'_> {
    fn name(&self) -> String {
        match &self.source {
            Source::File(path) => path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            Source::Region { region, shift, .. } => match shift {
                0 => region.label.clone(),
                _ => format!("{} {shift:+}", region.label),
            },
        }
    }

    /// The stroke as an error names it.
    fn whose(&self) -> String {
        match &self.source {
            Source::File(path) => path.display().to_string(),
            Source::Region { path, region, .. } => {
                format!("{}: region {:?}", path.display(), region.label)
            }
        }
    }

    /// The stroke's audio, a region's raised by `gain` before it is dithered to 16 bits.
    fn pcm(&self, gain: f32) -> Result<wav::Pcm16, String> {
        let seed = dither_seed(self.root, self.bank, self.layer);
        match &self.source {
            Source::File(path) => crate::wav::pcm16(path),
            Source::Region {
                take,
                audio,
                sample_loop,
                fade,
                ..
            } => Ok(crate::wav::cut(
                take,
                audio,
                self.bank,
                *sample_loop,
                *fade,
                gain,
                seed,
            )),
        }
    }

    /// How loud a region is, in decibels over a steady sample of one unit, measured as
    /// the encoder measures a stroke: the most power any 10 ms of its first half second
    /// averages, on its louder channel. None for a WAV, and for a silent region.
    fn level(&self) -> Option<f64> {
        let Source::Region { take, audio, .. } = &self.source else {
            return None;
        };
        nord_format::take::layers::level(audio, usize::from(take.channels), take.rate)
    }

    /// The loudest sample of a region, and none for a WAV, which is already 16 bits.
    fn peak(&self) -> Option<f32> {
        match &self.source {
            Source::File(_) => None,
            Source::Region { audio, .. } => {
                Some(audio.iter().fold(0.0, |most, x| f32::max(most, x.abs())))
            }
        }
    }
}

/// The dither noise of a stroke's audio, its own: its root, bank and layer pick it.
fn dither_seed(root: u8, bank: Bank, layer: LayerTag) -> u64 {
    let layer = match layer {
        LayerTag::Index(index) => index,
        LayerTag::Value(value) => value,
    };
    u64::from(root) << 16 | u64::from(bank.code()) << 8 | u64::from(layer)
}

/// The gain each stroke's audio is raised or lowered by before it is rounded to 16
/// bits, in the order `files` came in.
///
/// A format 6 library built from rules plays each stroke at the level of its root's
/// loudest layer, whatever the stroke's own level, and each root at its loudest layer's
/// audio level. So every region of a take is brought to one peak: a soft layer keeps
/// its detail above the 16-bit steps, and a root recorded quieter plays as loud as the
/// others. Kept at their recorded levels, the treble roots of the interface take played
/// 2 to 12 dB under the prototype's, which brought every hit to one peak, and the top
/// sounded dead (heard on a Stage 4). In any other library a stroke plays at its own
/// level, so nothing changes.
fn take_gains(files: &[StrokeFile], leveled: bool) -> Vec<f32> {
    let target = 10f32.powf(TAKE_PEAK_DB as f32 / 20.0);
    files
        .iter()
        .map(|file| match (leveled, file.peak()) {
            (true, Some(peak)) if peak > 0.0 => target / peak,
            _ => 1.0,
        })
        .collect()
}

/// The lowest and highest keys a take's roots are pitched out towards: the instrument's
/// 88.
const KEYS: (u8, u8) = (21, 108);

/// The furthest a root is pitched to play keys its take does not reach, in semitones.
const MOST_PITCHED: i32 = 12;

/// Extra roots that reach the keys a take's lowest and highest roots do not, each the
/// strokes of that root pitched, as the prototype made them, for a format 6 library
/// built from rules.
///
/// A root plays from 12 semitones under it to one over, and a key past them is silent.
/// The lowest root's strokes are pitched down to the root that reaches [`KEYS`]' lowest,
/// and the highest's up to the one that reaches its highest, by at most
/// [`MOST_PITCHED`] semitones.
fn pitched_roots<'a>(files: &[StrokeFile<'a>]) -> Result<Vec<StrokeFile<'a>>, String> {
    let roots: BTreeSet<u8> = files.iter().map(|file| file.root).collect();
    let (Some(&lowest), Some(&highest)) = (roots.first(), roots.last()) else {
        return Ok(Vec::new());
    };
    let mut wanted = Vec::new();
    if lowest > KEYS.0 + 12 {
        let to = (KEYS.0 + 12).max(lowest.saturating_sub(MOST_PITCHED as u8));
        wanted.push((lowest, to));
    }
    if highest < KEYS.1 - 1 {
        let to = (KEYS.1 - 1).min(highest + MOST_PITCHED as u8);
        wanted.push((highest, to));
    }
    let mut out = Vec::new();
    for (from, to) in wanted {
        let shift = i32::from(to) - i32::from(from);
        for file in files.iter().filter(|file| file.root == from) {
            let Source::Region {
                path,
                take,
                region,
                audio,
                sample_loop,
                fade,
                ..
            } = &file.source
            else {
                continue;
            };
            let named = |e: &dyn std::fmt::Display| format!("{}: {e}", file.whose());
            let pitched = nord_format::take::resample::pitched(
                &wav::Audio {
                    rate: take.rate,
                    channels: take.channels,
                    samples: audio.clone(),
                },
                shift,
            )
            .map_err(|e| named(&e))?;
            let frames = pitched.frames() as u32;
            let scale = |at: u32| {
                ((f64::from(at) * frames as f64 / (audio.len() / usize::from(take.channels)) as f64)
                    .round() as u32)
                    .min(frames)
            };
            out.push(StrokeFile {
                source: Source::Region {
                    path,
                    take,
                    region,
                    audio: pitched.samples,
                    // A loop's end is its last frame, so it stays on one.
                    sample_loop: sample_loop.map(|found| wav::Loop {
                        start: scale(found.start).min(frames.saturating_sub(1)),
                        end: scale(found.end).min(frames.saturating_sub(1)),
                        ..found
                    }),
                    fade: fade.map(scale),
                    shift,
                },
                root: to,
                bank: file.bank,
                layer: file.layer,
            });
        }
    }
    Ok(out)
}

/// How far under its own attack each release region was recorded, in the order `files`
/// came in, with `values` their layer values: under the attack region of its root at its
/// layer value. None for every other stroke, for a release with no such attack, and
/// unless `leveled`, as [`take_gains`] says.
///
/// One recording holds both strokes, so their balance is the instrument's: a format 6
/// library from rules plays the release at it, lifted as quiet key-offs need. A
/// directory's WAVs may each have been levelled on their own, so theirs is not used.
fn release_balance(files: &[StrokeFile], values: &[u8], leveled: bool) -> Vec<Option<f64>> {
    let attacks: BTreeMap<(u8, u8), f64> = files
        .iter()
        .zip(values)
        .filter(|(file, _)| file.bank == Bank::Attack)
        .filter_map(|(file, &value)| Some(((file.root, value), file.level()?)))
        .collect();
    files
        .iter()
        .zip(values)
        .map(|(file, &value)| match (leveled, file.bank) {
            (true, Bank::Release) => Some(attacks.get(&(file.root, value))? - file.level()?),
            _ => None,
        })
        .collect()
}

/// The end of a region's label that makes it the fade of the stroke the rest names.
const FADE_SUFFIX: &str = "-fade";

fn in_stroke_order(strokes: &mut [StrokeFile]) {
    strokes.sort_by_key(|s| (s.root, s.bank.code(), s.layer, s.name()));
}

/// The WAVs in a directory, with what their names say each one is, in stroke order.
fn stroke_files(dir: &Path) -> Result<Vec<StrokeFile<'static>>, String> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
        {
            continue;
        }
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let (root, bank, layer) = parse_stroke_name(&stem, Stem::None).ok_or_else(|| {
            format!(
                "{}: a WAV here must be named <root>-b<bank>-l<layer>.wav, as in \
                 060-b0-l00.wav for MIDI note 60, bank 0 (attack), layer 0, or \
                 <root>-b<bank>-v<value>.wav to give the layer value directly",
                path.display()
            )
        })?;
        out.push(StrokeFile {
            source: Source::File(path),
            root,
            bank,
            layer,
        });
    }
    if out.is_empty() {
        return Err(format!("{}: no WAV to build a library from", dir.display()));
    }
    in_stroke_order(&mut out);
    Ok(out)
}

/// The regions of the take at `path`, with what their labels say each stroke is, in
/// stroke order, each cleaned of the take's `noise` by its root's pitch, or as recorded
/// where there is none.
fn region_strokes<'a>(
    path: &'a Path,
    take: &'a wav::Audio,
    marks: &'a wav::Marks,
    noise: Option<&Noise>,
) -> Result<Vec<StrokeFile<'a>>, String> {
    let named = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let mut out = Vec::new();
    let mut fades = Vec::new();
    for (region, sample_loop) in marks.regions_with_loops().map_err(|e| named(&e))? {
        if let Some(stroke) = region.label.strip_suffix(FADE_SUFFIX) {
            fades.push((stroke, region));
            continue;
        }
        let (root, bank, layer) = parse_stroke_name(&region.label, Stem::None)
            .filter(|_| region.frames > 0)
            .ok_or_else(|| {
                named(&format!(
                    "region {:?} at frame {} is no stroke: a region here must span frames \
                     and be labelled as a stroke's WAV is named, as in 060-b0-l00",
                    region.label, region.start
                ))
            })?;
        let channels = usize::from(take.channels.max(1));
        let audio = match noise {
            Some(noise) => noise.stroke(
                &take.samples,
                channels,
                region.start as usize,
                region.frames as usize,
                clean::f0(root),
            ),
            None => {
                let at = |frame: u32| (frame as usize * channels).min(take.samples.len());
                take.samples[at(region.start)..at(region.start + region.frames)].to_vec()
            }
        };
        out.push(StrokeFile {
            source: Source::Region {
                path,
                take,
                region,
                audio,
                sample_loop,
                fade: None,
                shift: 0,
            },
            root,
            bank,
            layer,
        });
    }
    for (stroke, fade) in fades {
        let refuse = |why: &str| named(&format!("region {:?} {why}", fade.label));
        let owner = out
            .iter_mut()
            .find(|file| file.name() == stroke)
            .ok_or_else(|| {
                refuse(&format!(
                    "fades no stroke: no region is labelled {stroke:?}"
                ))
            })?;
        let Source::Region {
            region,
            sample_loop,
            fade: at,
            ..
        } = &mut owner.source
        else {
            unreachable!("a take's strokes are its regions");
        };
        let end = |r: &wav::Region| r.start + r.frames;
        if fade.start < region.start || end(fade) != end(region) {
            return Err(refuse(&format!(
                "must lie within {stroke:?} and end with it, where the fade reaches silence"
            )));
        }
        if owner.bank == Bank::Release || sample_loop.is_some() || at.is_some() {
            return Err(refuse(&format!(
                "fades {stroke:?}, which is a release, loops, or fades already; only an \
                 attack or resonance that does not loop fades"
            )));
        }
        *at = Some(fade.start - region.start);
    }
    if out.is_empty() {
        return Err(named(&"no region to build a library from"));
    }
    in_stroke_order(&mut out);
    Ok(out)
}

/// The layer value each file's stroke states in a `version` library, in the order the
/// files came in, as [`encode::layer_values`] reads their names.
///
/// A `v` name past [`Version::highest_played_layer`] is a stroke no velocity would
/// reach, and is refused by the file that names it.
fn layer_values(files: &[StrokeFile], version: Version) -> Result<Vec<u8>, String> {
    let named: Vec<(u8, Bank, LayerTag)> = files
        .iter()
        .map(|file| (file.root, file.bank, file.layer))
        .collect();
    let values = encode::layer_values(&named, version).map_err(|clash| {
        let what = format!("root {} {}", note::name(clash.root), clash.bank.name());
        match clash.how {
            Clash::BothForms => format!(
                "{what} names some of its layers by index (l..) and some by value \
                 (v..); all layers of one root and bank must use the same form"
            ),
            Clash::Twice => format!("{what} names one of its layers twice"),
        }
    })?;
    let highest = version.highest_played_layer();
    for (file, &value) in files.iter().zip(&values) {
        if value > highest {
            return Err(format!(
                "{}: no velocity selects layer value {value}; {highest} is the largest a \
                 key of a format {} library ever sounds",
                file.whose(),
                version.number()
            ));
        }
    }
    Ok(values)
}

/// The rules a build follows without a template, from the flags that set them, and
/// `stated`, the kind the takes state, where `--kind` says none.
fn build_rules(args: &BuildArgs, stated: Option<encode::Kind>) -> Result<encode::Rules, String> {
    let kind = args
        .kind
        .map(encode::Kind::from)
        .or(stated)
        .unwrap_or_default();
    let tenths = (args.gain * 10.0).round();
    let gain = (tenths.is_finite() && (f64::from(i8::MIN)..=f64::from(i8::MAX)).contains(&tenths))
        .then_some(tenths as i8)
        .ok_or_else(|| {
            format!(
                "--gain {} dB is outside the -12.8 to 12.7 the field holds",
                args.gain
            )
        })?;
    let damper_top = match &args.damper_top {
        Some(key) => note::parse(key)?,
        None => kind.damper_top(),
    };
    let loop_fade = match args.loop_fade {
        Some(rate) => {
            let tenths = (rate * 10.0).round();
            (tenths.is_finite() && (0.0..=f64::from(u16::MAX)).contains(&tenths))
                .then_some(tenths as u16)
                .ok_or_else(|| {
                    format!("--loop-fade {rate} dB/s is outside the 0 to 6553.5 the rules hold")
                })?
        }
        None => kind.loop_fade(),
    };
    Ok(encode::Rules {
        version: args.format.into(),
        kind,
        gain,
        damper_top,
        loop_fade,
    })
}

/// The kind `takes` state, at `paths`: the one they agree on, none where none states one.
fn stated_kind(
    paths: &[PathBuf],
    takes: &[(wav::Audio, wav::Marks, wav::Settings)],
) -> Result<Option<encode::Kind>, String> {
    let mut stated: Option<(encode::Kind, &Path)> = None;
    for (path, (_, _, settings)) in paths.iter().zip(takes) {
        let Some(word) = &settings.kind else {
            continue;
        };
        let kind = encode::Kind::from_word(word).ok_or_else(|| {
            format!(
                "{}: states kind {word:?}, which is no kind of instrument",
                path.display()
            )
        })?;
        match stated {
            Some((first, at)) if first != kind => {
                return Err(format!(
                    "{} states kind {}, but {} states {}; pass --kind to choose",
                    at.display(),
                    first.word(),
                    path.display(),
                    kind.word()
                ));
            }
            _ => stated = Some((kind, path)),
        }
    }
    Ok(stated.map(|(kind, _)| kind))
}

pub fn build(ui: &Ui, args: BuildArgs) -> Result<(), String> {
    let takes = match args.input.as_slice() {
        [dir] if dir.is_dir() => Vec::new(),
        paths => paths
            .iter()
            .map(|path| match path.is_dir() {
                true => Err(format!(
                    "{}: a directory of WAVs is built on its own, not with takes",
                    path.display()
                )),
                false => crate::wav::take(path),
            })
            .collect::<Result<Vec<_>, String>>()?,
    };
    let rules = build_rules(&args, stated_kind(&args.input, &takes)?)?;
    let template = match &args.template {
        Some(path) => {
            refuse_in_place(path, &args.out)?;
            Some(read(path)?.1)
        }
        None => None,
    };
    let template = match &template {
        Some(piano) => Some(piano.library().map_err(|e| e.to_string())?),
        None => None,
    };
    let version = template.as_ref().map_or(rules.version, |t| t.version());
    let donor = match &template {
        Some(library) => encode::Donor::Template(library),
        None => encode::Donor::Rules(rules),
    };

    ui.out(ui.dim(format!(
        "  {:>26}  {:>6} {:>10} {:>5} {:>9} {:>8}",
        "wav", "root", "bank", "value", "frames", "seconds"
    )));
    let files = match takes.is_empty() {
        true => stroke_files(&args.input[0])?,
        false => {
            let mut files = Vec::new();
            for (path, (audio, marks, settings)) in args.input.iter().zip(&takes) {
                // A take its editor set to raw is built as recorded.
                let noise = match settings.clean {
                    Some(false) => {
                        ui.note(format!(
                            "{}: built as recorded, not cleaned",
                            path.display()
                        ));
                        None
                    }
                    _ => Some(Noise::of(&nord_format::take::plan::mono(audio), audio.rate)),
                };
                files.extend(region_strokes(path, audio, marks, noise.as_ref())?);
            }
            let pitched = match (&donor, version) {
                (encode::Donor::Rules(_), Version::V6) => pitched_roots(&files)?,
                _ => Vec::new(),
            };
            for file in &pitched {
                let Source::Region { shift, .. } = file.source else {
                    unreachable!("pitched roots are regions");
                };
                ui.note(format!(
                    "{}: pitched {shift:+} semitones to a root of {}, to reach keys the take does not",
                    file.whose(),
                    note::name(file.root)
                ));
            }
            files.extend(pitched);
            in_stroke_order(&mut files);
            files
        }
    };
    let values = layer_values(&files, version)?;
    let gains = take_gains(
        &files,
        matches!((&donor, version), (encode::Donor::Rules(_), Version::V6)),
    );
    let balances = release_balance(
        &files,
        &values,
        matches!((&donor, version), (encode::Donor::Rules(_), Version::V6)),
    );
    let balanced = balances.iter().flatten().count();
    let leveled = gains.iter().filter(|&&gain| gain != 1.0).count();
    let (least, most) = gains
        .iter()
        .fold((1.0f32, 1.0f32), |(l, m), &gain| (l.min(gain), m.max(gain)));
    let mut recordings = Vec::new();
    let mut clipped = 0usize;
    let mut resampled = 0usize;
    let mut looped = 0usize;
    for (((file, layer), gain), under_attack) in files.iter().zip(values).zip(gains).zip(balances) {
        let pcm = file.pcm(gain)?;
        let audio = encode::resample(&pcm.samples, usize::from(pcm.channels), pcm.rate)
            .map_err(|e| format!("{}: {e}", file.whose()))?;
        let sustain = pcm
            .sample_loop
            .map(|found| encode::sustain(found, pcm.rate))
            .transpose()
            .map_err(|e| format!("{}: {e}", file.whose()))?;
        looped += usize::from(sustain.is_some());
        clipped += audio.clipped;
        resampled += usize::from(pcm.rate != codec::RATE);
        let frames = audio.channels.first().map_or(0, Vec::len);
        ui.out(format!(
            "  {:>26}  {:>6} {:>10} {:>5} {:>9} {:>8.3}{}{}",
            file.name(),
            note::name(file.root),
            file.bank.name(),
            layer,
            frames,
            frames as f64 / f64::from(codec::RATE),
            if pcm.rate == codec::RATE {
                String::new()
            } else {
                format!("  from {} Hz", pcm.rate)
            },
            match sustain {
                Some(sustain) => format!(
                    "  loops {:.3} s from {:.3} s",
                    f64::from(sustain.length()) / f64::from(codec::RATE),
                    f64::from(sustain.start()) / f64::from(codec::RATE)
                ),
                None => String::new(),
            }
        ));
        recordings.push(encode::Recording {
            root: file.root,
            bank: file.bank,
            layer,
            channels: audio.channels,
            sustain,
            under_attack,
        });
    }

    let options = encode::Options::new(&args.name).variant(&args.variant);
    let library = encode::build(&donor, &options, &recordings).map_err(|e| e.to_string())?;
    let bytes = to_bytes(&library, &args.out)?;
    write_file(ui, &args.out, &bytes)?;

    let covered = covered_keys(&library);
    ui.out(format!(
        "  {} stroke(s) over {} root(s), {} channel(s); keys {}",
        library.strokes().len(),
        library.roots().len(),
        library.channels(),
        key_span(&covered),
    ));
    if resampled > 0 {
        ui.note(format!(
            "{resampled} WAV(s) were resampled to the {} Hz rate the instrument plays at",
            codec::RATE
        ));
    }
    if balanced > 0 {
        ui.note(format!(
            "{balanced} release(s) play at their recorded level under their own attack, \
             lifted by at most 12 dB towards 14 dB under it"
        ));
    }
    if leveled > 0 {
        ui.note(format!(
            "{leveled} region(s) were brought to a {TAKE_PEAK_DB} dBFS peak, by {:+.1} to \
             {:+.1} dB, before rounding to 16 bits, so every root plays as loud as the others",
            20.0 * least.log10(),
            20.0 * most.log10()
        ));
    }
    if looped > 0 {
        ui.note(format!(
            "{looped} stroke(s) loop where their WAV's smpl chunk says while the key is held"
        ));
        if args.template.is_none() {
            ui.note(match rules.loop_fade {
                0 => "a held loop keeps its level until the key is released".to_string(),
                tenths => format!(
                    "a held loop fades {:.1} dB/s at C4, faster up the keyboard",
                    f64::from(tenths) / 10.0
                ),
            });
        }
    }
    if clipped > 0 {
        ui.note(format!(
            "{clipped} resampled sample(s) clipped at the 16-bit limit; the source is \
             loud enough that the resampling filter overshoots"
        ));
    }
    ui.note(match &args.template {
        Some(_) => format!(
            "{} takes the decay coefficients, per-note tables, playback parameters and \
             the word at the start of the body from the template",
            args.out.display()
        ),
        None => format!(
            "{} uses playback rules in place of a template: each stroke decaying as the \
             instrument's own pianos of its kind do, {}, a fade at key-up, {:+.1} dB of \
             library gain, and the damper reaching {}",
            args.out.display(),
            match version {
                Version::V5 => "each stroke trimmed by its own layer value",
                Version::V6 => "each layer trimmed to its root's loudest, softer layers lifted as its kind's are",
            },
            f64::from(rules.gain) / 10.0,
            damper_reach(rules.damper_top),
        ),
    });
    Ok(())
}

/// What a `damper_top` reaches, for the line a build prints.
fn damper_reach(top: u8) -> String {
    match top >= encode::ALL_KEYS_DAMPED {
        true => "every key".to_string(),
        false => format!("{} and below", note::name(top)),
    }
}

pub fn rebuild(ui: &Ui, args: RebuildArgs) -> Result<(), String> {
    refuse_in_place(&args.file, &args.out)?;
    let (original, piano) = read(&args.file)?;
    let library = piano.library().map_err(|e| e.to_string())?;
    let again = encode::rebuild(&library).map_err(|e| e.to_string())?;

    ui.out(ui.dim(format!(
        "  {:>5} {:>6} {:>10} {:>5} {:>7}  blocks",
        "index", "root", "bank", "layer", "blocks"
    )));
    let mut exact = 0usize;
    let mut restated = 0usize;
    let mut recoded = 0usize;
    for (index, (stroke, coded)) in library.strokes().iter().zip(&again.strokes).enumerate() {
        restated += coded.restated;
        recoded += coded.recoded();
        exact += usize::from(coded.identical == coded.blocks);
        ui.out(format!(
            "  {index:>5} {:>6} {:>10} {:>5} {:>7}  {}",
            note::name(stroke.root),
            bank_label(stroke.bank_code()),
            stroke.layer(),
            coded.blocks,
            match (coded.restated, coded.recoded()) {
                (0, 0) => "exact".to_string(),
                (n, 0) => format!("{n} restating the attenuation"),
                (0, n) => format!("{} {n} coded differently", ui.danger("!")),
                (n, m) => format!(
                    "{} {n} restating the attenuation, {m} coded differently",
                    ui.danger("!")
                ),
            }
        ));
    }

    let bytes = to_bytes(&again.library, &args.file)?;
    write_file(ui, &args.out, &bytes)?;
    ui.out(format!(
        "  {exact} of {} stroke(s) came back byte for byte",
        again.strokes.len()
    ));
    report_size(ui, original.len(), bytes.len());
    if restated > 0 {
        ui.note(format!(
            "{restated} block(s) declare a different attenuation. The original encoder \
             measured that value independently of the stored frames, and decoding never \
             reads it"
        ));
    }
    if recoded > 0 {
        ui.note(format!(
            "{} {recoded} block(s) came back with different residuals, width or order; \
             this library was not encoded the way this tool encodes one",
            ui.danger("warning:")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn piano(argv: &[&str]) -> Result<crate::PianoAction, clap::error::ErrorKind> {
        match crate::Cli::try_parse_from([&["nord", "piano"], argv].concat()) {
            Ok(crate::Cli {
                command: crate::Command::Piano { action },
                ..
            }) => Ok(action),
            Ok(_) => panic!("{argv:?} parsed as another noun"),
            Err(e) => Err(e.kind()),
        }
    }

    /// A root's attack region, `frames` frames of a tone, with a loop ending on its last
    /// frame and a fade from its middle.
    fn region_stroke<'a>(
        path: &'a Path,
        take: &'a wav::Audio,
        region: &'a wav::Region,
        root: u8,
    ) -> StrokeFile<'a> {
        let frames = region.frames;
        StrokeFile {
            source: Source::Region {
                path,
                take,
                region,
                audio: take.samples.clone(),
                sample_loop: Some(wav::Loop::forward(frames / 2, frames - 1)),
                fade: Some(frames / 2),
                shift: 0,
            },
            root,
            bank: Bank::Attack,
            layer: LayerTag::Index(0),
        }
    }

    #[test]
    fn a_pitched_root_keeps_its_loop_inside_its_frames_and_reaches_the_keys() {
        let path = Path::new("take.wav");
        for frames in [99u32, 100, 101, 1001] {
            let take = wav::Audio {
                rate: 35_002,
                channels: 1,
                samples: (0..frames).map(|i| (i as f32 / 20.0).sin() / 2.0).collect(),
            };
            let region = wav::Region {
                start: 0,
                frames,
                label: "060-b0-l00".into(),
            };
            let pitched = pitched_roots(&[region_stroke(path, &take, &region, 60)]).unwrap();
            let roots: Vec<(u8, i32)> = pitched
                .iter()
                .map(|file| match file.source {
                    Source::Region { shift, .. } => (file.root, shift),
                    Source::File(_) => panic!("a pitched root is a region"),
                })
                .collect();
            assert_eq!(roots, [(48, -12), (72, 12)], "{frames} frames");
            for file in &pitched {
                let Source::Region {
                    audio,
                    sample_loop: Some(found),
                    fade: Some(fade),
                    ..
                } = &file.source
                else {
                    panic!("the loop and fade are kept");
                };
                let len = audio.len() as u32;
                assert!(
                    found.start <= found.end && found.end < len && *fade <= len,
                    "{frames} frames pitched to {}: loop {found:?}, fade {fade}, {len} frames",
                    file.root
                );
            }
        }
    }

    #[test]
    fn no_root_is_pitched_where_the_take_reaches_the_keys_or_holds_none() {
        assert!(pitched_roots(&[]).unwrap().is_empty());
        let path = Path::new("take.wav");
        let take = wav::Audio {
            rate: 35_002,
            channels: 1,
            samples: vec![0.0; 100],
        };
        let low = wav::Region {
            start: 0,
            frames: 100,
            label: "033-b0-l00".into(),
        };
        let high = wav::Region {
            label: "107-b0-l00".into(),
            ..low.clone()
        };
        let files = [
            region_stroke(path, &take, &low, 33),
            region_stroke(path, &take, &high, 107),
        ];
        assert!(pitched_roots(&files).unwrap().is_empty());
    }

    #[test]
    fn a_bank_is_named_as_the_format_spells_it_and_attack_is_never_dropped() {
        let trim = |bank| match piano(&["trim", "in.npno", "-o", "out.npno", "--drop-bank", bank]) {
            Ok(crate::PianoAction::Trim(args)) => Ok(args.drop_bank),
            Ok(_) => panic!("--drop-bank {bank} parsed as another verb"),
            Err(kind) => Err(kind),
        };
        assert_eq!(trim("resonance"), Ok(vec![Bank::Resonance]));
        assert_eq!(trim("release"), Ok(vec![Bank::Release]));
        assert_eq!(trim("attack"), Err(clap::error::ErrorKind::InvalidValue));

        for bank in Bank::ALL {
            let decode = ["decode", "in.npno", "--key", "C4", "-o", "a.wav"];
            match piano(&[&decode[..], &["--bank", bank.name()]].concat()) {
                Ok(crate::PianoAction::Decode(args)) => assert_eq!(args.bank, Some(bank)),
                _ => panic!("--bank {bank} did not parse"),
            }
        }
    }

    fn wav(root: u8, bank: Bank, layer: LayerTag) -> StrokeFile<'static> {
        StrokeFile {
            source: Source::File(PathBuf::new()),
            root,
            bank,
            layer,
        }
    }

    /// `v255` parses, but no key would ever sound it, so the name is refused instead of
    /// building a stroke nothing plays.
    #[test]
    fn a_named_layer_value_no_velocity_selects_is_refused() {
        for version in Version::ALL {
            let highest = version.highest_played_layer();
            let named =
                |value| layer_values(&[wav(60, Bank::Attack, LayerTag::Value(value))], version);
            assert_eq!(named(highest).unwrap(), [highest]);
            let refused = named(highest + 1).unwrap_err();
            assert!(refused.contains("no velocity selects"), "{refused}");
            assert!(refused.contains(&highest.to_string()), "{refused}");
        }
    }

    #[test]
    fn one_root_and_bank_names_its_layers_one_way() {
        let mixed = [
            wav(60, Bank::Attack, LayerTag::Index(0)),
            wav(60, Bank::Attack, LayerTag::Value(12)),
        ];
        let refused = layer_values(&mixed, Version::V6).unwrap_err();
        assert!(refused.contains("C4 attack"), "{refused}");

        let twice = [
            wav(60, Bank::Attack, LayerTag::Index(0)),
            wav(60, Bank::Attack, LayerTag::Index(0)),
        ];
        assert!(
            layer_values(&twice, Version::V6).is_err(),
            "a layer named twice"
        );
    }

    #[test]
    fn a_layer_count_and_an_explicit_list_are_told_apart() {
        assert_eq!(parse_layers("2").unwrap(), Layers::Loudest(2));
        assert_eq!(
            parse_layers("=0,3,7").unwrap(),
            Layers::Only([0, 3, 7].into_iter().collect())
        );
        assert!(parse_layers("0").is_err(), "keeping no layer is a mistake");
        assert!(parse_layers("=").is_err());
        assert!(parse_layers("two").is_err());
    }

    #[test]
    fn a_range_takes_note_names_at_either_end() {
        assert_eq!(parse_range("C2..C6").unwrap(), (36, 84));
        assert_eq!(parse_range("36..84").unwrap(), (36, 84));
        assert!(
            parse_range("C6..C2").is_err(),
            "an inverted range is refused"
        );
        assert!(parse_range("C2").is_err());
    }

    #[test]
    fn a_tune_value_reads_units_by_default_and_cents_on_request() {
        assert_eq!(parse_tune("-4").unwrap(), -4);
        assert_eq!(parse_tune("+3").unwrap(), 3);
        // 2.1 cents at 0.7 cents a unit.
        assert_eq!(parse_tune("2.1c").unwrap(), 3);
        assert!(parse_tune("400c").is_err());
        assert!(parse_tune("loud").is_err());
    }

    /// A cent count that is not a number cannot round to one: `nan` and `inf` parse as
    /// floats, and rounding them would silently tune the key to 0.
    #[test]
    fn a_cent_count_that_is_not_finite_is_refused_rather_than_rounded() {
        for bad in ["nanc", "NaNc", "infc", "-infc"] {
            assert!(parse_tune(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_pair_splits_on_the_first_equals_and_reads_its_key_as_a_note() {
        assert_eq!(split_pair("C4=60", "map").unwrap(), (60, "60"));
        assert_eq!(split_pair("60=-", "map").unwrap(), (60, "-"));
        assert!(split_pair("C4", "map").is_err());
    }

    /// Every verb that takes a key reads it here, so a key the tables cannot hold is
    /// refused before it reaches a transform.
    #[test]
    fn a_key_past_the_midi_range_is_refused_at_every_flag_that_takes_one() {
        assert!(split_pair("127=3", "tune").is_ok());
        assert!(split_pair("128=3", "tune").is_err());
        assert!(split_pair("128=C4", "map").is_err());
        assert!(note::parse("128").is_err(), "--at and --map's root");
        assert_eq!(parse_range("0..127").unwrap(), (0, 127));
        assert!(parse_range("0..128").is_err());
    }

    /// A decaying stereo tone of `frames` at `hz`, at a rate the instrument plays.
    fn tone(hz: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let t = i as f64 / f64::from(codec::RATE);
                let x = (std::f64::consts::TAU * hz * t).sin() * (-3.0 * t).exp() * 0.4;
                [x as f32, (x * 0.7) as f32]
            })
            .collect()
    }

    fn build_from(input: &Path, out: &Path) -> Result<Vec<u8>, String> {
        build_as(input, out, "6")
    }

    fn build_as(input: &Path, out: &Path, format: &str) -> Result<Vec<u8>, String> {
        let (input, out) = (input.to_str().unwrap(), out.to_str().unwrap());
        let Ok(crate::PianoAction::Build(args)) = piano(&[
            "build",
            input,
            "--name",
            "Take",
            "--kind",
            "wurlitzer",
            "--format",
            format,
            "-o",
            out,
        ]) else {
            panic!("build did not parse");
        };
        build(&Ui::piped(), args)?;
        Ok(std::fs::read(out).unwrap())
    }

    /// A take at `path`, 24-bit, of `audio` with `marks`, as a build reads it back.
    fn write_take(path: &Path, audio: &wav::Audio, marks: &wav::Marks) -> wav::Audio {
        let deep = wav::write(audio, wav::Encoding::Int { bits: 24 }).unwrap();
        std::fs::write(path, wav::with_marks(&deep, marks).unwrap()).unwrap();
        wav::read(&deep).unwrap()
    }

    /// The regions of `take` written into `dir` as 16-bit WAVs of their own, unraised,
    /// cleaned and their loops shaped as a take's build does it.
    fn write_cuts(dir: &Path, take: &wav::Audio, marks: &wav::Marks) {
        std::fs::create_dir(dir).unwrap();
        let noise = Noise::of(&nord_format::take::plan::mono(take), take.rate);
        for (found, sample_loop) in marks.regions_with_loops().unwrap() {
            let (root, bank, layer) = parse_stroke_name(&found.label, Stem::None).unwrap();
            let frames = noise.stroke(
                &take.samples,
                usize::from(take.channels),
                found.start as usize,
                found.frames as usize,
                clean::f0(root),
            );
            let pcm = crate::wav::cut(
                take,
                &frames,
                bank,
                sample_loop,
                None,
                1.0,
                dither_seed(root, bank, layer),
            );
            let file = wav::pcm16(&pcm.samples, pcm.rate, pcm.channels).unwrap();
            let loops = wav::Marks {
                regions: vec![],
                loops: sample_loop.into_iter().collect(),
            };
            let file = wav::with_marks(&file, &loops).unwrap();
            std::fs::write(dir.join(format!("{}.wav", found.label)), file).unwrap();
        }
    }

    fn region(start: u32, frames: u32, label: &str) -> wav::Region {
        wav::Region {
            start,
            frames,
            label: label.into(),
        }
    }

    /// The RFC's equivalence: a take's regions build the strokes their audio, cut into
    /// WAVs of their own, builds. A format 5 library plays each stroke at its own
    /// level, so nothing is raised.
    #[test]
    fn a_takes_regions_build_what_the_same_cuts_as_wavs_build() {
        let dir = crate::edit::tests::scratch("piano-take");
        let mut samples = tone(261.6, 9_000);
        samples.extend(tone(329.6, 8_000));
        let audio = wav::Audio {
            rate: codec::RATE,
            channels: 2,
            samples,
        };
        let marks = wav::Marks {
            regions: vec![
                region(100, 7_900, "060-b0-l00"),
                region(8_000, 1_000, "060-b2-l00"),
                region(9_000, 8_000, "064-b0-l00"),
            ],
            loops: vec![wav::Loop::forward(5_000, 6_999)],
        };
        let path = dir.join("take.wav");
        let take = write_take(&path, &audio, &marks);
        write_cuts(&dir.join("cuts"), &take, &marks);

        let from_take = build_as(&path, &dir.join("take.npno"), "5").unwrap();
        let from_cuts = build_as(&dir.join("cuts"), &dir.join("cuts.npno"), "5").unwrap();
        assert!(
            from_take == from_cuts,
            "the take built another library than its cuts"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A take states its kind and whether it is cleaned, as drawbar's take editor saves
    /// them: a build files the library as that kind where `--kind` says none, builds a
    /// take set to raw from its regions as recorded, and refuses takes whose kinds
    /// disagree.
    #[test]
    fn a_take_s_own_settings_choose_its_kind_and_whether_it_is_cleaned() {
        let dir = crate::edit::tests::scratch("piano-take-settings");
        let audio = wav::Audio {
            rate: codec::RATE,
            channels: 2,
            samples: tone(261.6, 9_000),
        };
        let marks = wav::Marks {
            regions: vec![region(100, 7_900, "060-b0-l00")],
            loops: Vec::new(),
        };
        let path = dir.join("take.wav");
        let take = write_take(&path, &audio, &marks);
        let settings = |kind: &str, clean: bool| wav::Settings {
            kind: Some(kind.into()),
            clean: Some(clean),
        };
        let stated = |path: &Path, settings: &wav::Settings| {
            let bytes = std::fs::read(path).unwrap();
            std::fs::write(path, wav::with_settings(&bytes, settings).unwrap()).unwrap();
        };
        stated(&path, &settings("electric-piano", false));
        let built = |inputs: &[&Path], out: &Path| {
            let mut words = vec!["build".to_string()];
            words.extend(inputs.iter().map(|path| path.to_str().unwrap().to_string()));
            words.extend(
                [
                    "--name",
                    "Take",
                    "--format",
                    "5",
                    "-o",
                    out.to_str().unwrap(),
                ]
                .map(String::from),
            );
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            let Ok(crate::PianoAction::Build(args)) = piano(&words) else {
                panic!("build did not parse");
            };
            build(&Ui::piped(), args).map(|()| std::fs::read(out).unwrap())
        };
        let raw = built(&[&path], &dir.join("raw.npno")).unwrap();
        let (_, piano_lib) = read(&dir.join("raw.npno")).unwrap();
        assert_eq!(
            piano_lib.library().unwrap().kind_code(),
            encode::Kind::ElectricPiano.code(),
            "filed as the kind the take states"
        );

        let cuts = dir.join("cuts");
        std::fs::create_dir(&cuts).unwrap();
        let (found, _) = marks.regions_with_loops().unwrap()[0];
        let channels = usize::from(take.channels);
        let frames = &take.samples
            [found.start as usize * channels..(found.start + found.frames) as usize * channels];
        let pcm = crate::wav::cut(
            &take,
            frames,
            Bank::Attack,
            None,
            None,
            1.0,
            dither_seed(60, Bank::Attack, LayerTag::Index(0)),
        );
        let file = wav::pcm16(&pcm.samples, pcm.rate, pcm.channels).unwrap();
        std::fs::write(cuts.join("060-b0-l00.wav"), file).unwrap();
        let mut words = vec![
            "build",
            cuts.to_str().unwrap(),
            "--name",
            "Take",
            "--kind",
            "electric-piano",
        ];
        let out = dir.join("cuts.npno");
        words.extend(["--format", "5", "-o", out.to_str().unwrap()]);
        let Ok(crate::PianoAction::Build(args)) = piano(&words) else {
            panic!("build did not parse");
        };
        build(&Ui::piped(), args).unwrap();
        assert!(
            raw == std::fs::read(&out).unwrap(),
            "a raw take builds its regions as recorded"
        );

        let other = dir.join("other.wav");
        write_take(&other, &audio, &marks);
        stated(&other, &settings("grand", true));
        let refused = built(&[&path, &other], &dir.join("both.npno")).unwrap_err();
        assert!(
            refused.contains("electric-piano") && refused.contains("grand"),
            "{refused}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Each stroke of the library at `path`: its peak, and the decibels it plays at, the
    /// most power any 10 ms of it holds less its trim, as the level rule measures it.
    fn stroke_levels(path: &Path) -> Vec<(i16, f64)> {
        let (_, piano) = read(path).unwrap();
        let library = piano.library().unwrap();
        library
            .strokes()
            .iter()
            .map(|stroke| {
                let audio = codec::decode(stroke, library.channels()).unwrap();
                let lane = &audio.lanes[0];
                let peak = lane.iter().map(|v| v.saturating_abs()).max().unwrap();
                let power = lane
                    .windows(codec::RATE as usize / 100)
                    .map(|window| window.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>())
                    .fold(0.0, f64::max);
                let level = 10.0 * power.log10();
                (peak, level - f64::from(stroke.trim()) / 10.0)
            })
            .collect()
    }

    /// A soft layer and a release, 32 dB under their root's loudest layer, are raised
    /// to its peak before rounding. The soft layer still plays as loud against it as
    /// unraised, and the release, which the WAVs' build plays 18 dB under the loudest
    /// attack, plays at its recorded balance lifted by 12 dB: 20 dB under its attack.
    #[test]
    fn a_takes_soft_regions_are_raised_without_changing_how_loud_they_play() {
        let dir = crate::edit::tests::scratch("piano-take-raise");
        let soft = |hz, frames| {
            tone(hz, frames)
                .iter()
                .map(|x| x / 40.0)
                .collect::<Vec<_>>()
        };
        let gap = || vec![0.0f32; 2 * 9_000];
        let mut samples = tone(261.6, 9_000);
        samples.extend(gap());
        samples.extend(soft(261.6, 9_000));
        samples.extend(gap());
        samples.extend(soft(261.6, 2_000));
        let audio = wav::Audio {
            rate: codec::RATE,
            channels: 2,
            samples,
        };
        let marks = wav::Marks {
            regions: vec![
                region(0, 9_000, "060-b0-l00"),
                region(18_000, 9_000, "060-b0-l01"),
                region(36_000, 2_000, "060-b2-l00"),
            ],
            loops: vec![],
        };
        let path = dir.join("take.wav");
        let take = write_take(&path, &audio, &marks);
        write_cuts(&dir.join("cuts"), &take, &marks);
        build_from(&path, &dir.join("take.npno")).unwrap();
        build_from(&dir.join("cuts"), &dir.join("cuts.npno")).unwrap();

        let raised = stroke_levels(&dir.join("take.npno"));
        let unraised = stroke_levels(&dir.join("cuts.npno"));
        for stroke in 1..raised.len() {
            let under = 20.0 * (f64::from(raised[0].0) / f64::from(raised[stroke].0)).log10();
            assert!(
                under < 0.5,
                "stroke {stroke} peaks {under:.1} dB under its root's"
            );
        }
        let against = |levels: &[(i16, f64)], stroke: usize| levels[stroke].1 - levels[0].1;
        assert!(
            (against(&raised, 1) - against(&unraised, 1)).abs() < 0.15,
            "the soft layer plays {:.2} dB against its root raised, {:.2} unraised",
            against(&raised, 1),
            against(&unraised, 1)
        );
        let recorded = 20.0 * 40f64.log10();
        assert!(
            (against(&raised, 2) + recorded - 12.0).abs() < 0.15,
            "the release plays {:.2} dB against its attack",
            against(&raised, 2)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A fade region must end with the attack it names, and only an attack or
    /// resonance that does not loop fades.
    #[test]
    fn a_fade_that_is_not_an_attacks_end_is_refused() {
        let dir = crate::edit::tests::scratch("piano-take-fade");
        let audio = wav::Audio {
            rate: codec::RATE,
            channels: 2,
            samples: tone(261.6, 20_000),
        };
        let path = dir.join("take.wav");
        let cases = [
            (region(4_000, 5_000, "060-b0-l00-fade"), "end with it"),
            (region(15_000, 5_000, "060-b2-l00-fade"), "is a release"),
            (region(4_000, 6_000, "062-b0-l00-fade"), "fades no stroke"),
        ];
        for (fade, reason) in cases {
            let marks = wav::Marks {
                regions: vec![
                    region(0, 10_000, "060-b0-l00"),
                    region(10_000, 10_000, "060-b2-l00"),
                    fade,
                ],
                loops: vec![],
            };
            write_take(&path, &audio, &marks);
            let err = build_from(&path, &dir.join("out.npno")).unwrap_err();
            assert!(err.contains(reason), "want {reason:?}: {err}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_region_that_names_no_stroke_is_refused_by_label() {
        let dir = crate::edit::tests::scratch("piano-take-label");
        let take = wav::Audio {
            rate: codec::RATE,
            channels: 1,
            samples: vec![0.1; 2_000],
        };
        let path = dir.join("take.wav");
        let plain = wav::write(&take, wav::Encoding::Int { bits: 24 }).unwrap();
        for (frames, label) in [(500, "intro"), (0, "060-b0-l00")] {
            let marks = wav::Marks {
                regions: vec![wav::Region {
                    start: 10,
                    frames,
                    label: label.into(),
                }],
                loops: vec![],
            };
            std::fs::write(&path, wav::with_marks(&plain, &marks).unwrap()).unwrap();
            let err = build_from(&path, &dir.join("out.npno")).unwrap_err();
            assert!(err.contains("take.wav"), "{err}");
            assert!(err.contains(&format!("region {label:?}")), "{err}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
