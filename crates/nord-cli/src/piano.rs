//! `nord piano` — the verbs that only mean anything for a piano library.
//!
//! A library is a directory of strokes: one recording per root note, bank and
//! velocity layer, each owning a span of encoded audio. `inspect` reports that
//! directory, `decode` turns one stroke back into a WAV, and `edit`, `trim` and
//! `split` are the transforms `nord_format`'s model can express — renaming,
//! retuning and rerouting keys, dropping banks and layers, narrowing the key range,
//! and cutting a library in two. Nothing here re-encodes audio: a stroke that
//! survives a transform moves byte for byte.
//!
//! `build` and `rebuild` are the two verbs that do write audio: one lays a library out
//! from a directory of WAVs against a template, the other codes a library's own
//! strokes again and reports how each one came back.
//!
//! A library written here loads on the instrument and plays: confirmed on hardware
//! for `trim`, both for a dropped bank and for dropped velocity layers, and for what
//! `build` and `rebuild` code — mono and stereo, every bank, every root of a
//! full-keyboard library. What `edit` changes — a name, a key's tuning, the root a
//! key plays — and the narrowed key range `trim --range` and `split` leave behind are
//! inferred from specimens; not confirmed on hardware. The fields a build cannot
//! derive from audio — the length marks, the decay coefficients, the per-note tables,
//! the word at the body's start — go in as the template donated them: the instrument
//! accepts them, and what it makes of them beyond accepting is not known.
//!
//! These verbs take a file. A library is tens of megabytes, so moving one to or
//! from the instrument is `nord piano get` and `nord piano put`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use nord_format::formats::npno::{self, codec, encode, Bank, Change, Layers, Library, UNCOVERED};
use nord_format::Entity;

use crate::edit::write_file;
use crate::note;
use crate::ui::Ui;

/// The banks a trim can drop by name. The attack bank is every library's reason to
/// exist and dropping it would leave silence, so it is not offered.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DroppableBank {
    /// The pedal-down resonance set, which the larger libraries add.
    Resonance,
    /// The note-off sample.
    Release,
}

impl From<DroppableBank> for Bank {
    fn from(bank: DroppableBank) -> Bank {
        match bank {
            DroppableBank::Resonance => Bank::Resonance,
            DroppableBank::Release => Bank::Release,
        }
    }
}

#[derive(Args)]
pub struct InspectArgs {
    /// The piano libraries to report on.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,

    /// List every stroke rather than one line per root.
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

    /// The stroke by its index in the directory. The alternative to `--key`.
    #[arg(long, value_name = "N", conflicts_with = "key")]
    pub stroke: Option<usize>,

    /// A key the library covers: a name (`C4`, `F#3`) or 0-127. Narrow the strokes
    /// it selects with `--layer` and `--bank`.
    #[arg(long, value_name = "KEY")]
    pub key: Option<String>,

    /// Velocity layer, 0 being the loudest recording of the root.
    #[arg(long, value_name = "N", requires = "key")]
    pub layer: Option<u8>,

    #[arg(long, value_enum, requires = "key")]
    pub bank: Option<BankName>,

    /// Where to write the WAV. Frames come out at the rate the instrument plays
    /// them, with no gain applied.
    #[arg(short, long, value_name = "WAV")]
    pub out: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum BankName {
    Attack,
    Resonance,
    Release,
}

impl From<BankName> for Bank {
    fn from(bank: BankName) -> Bank {
        match bank {
            BankName::Attack => Bank::Attack,
            BankName::Resonance => Bank::Resonance,
            BankName::Release => Bank::Release,
        }
    }
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

    /// Replace the variant — the text after the `#`, where the vendor records the
    /// voicing and the library's size. It cannot itself hold a `#`.
    #[arg(long)]
    pub variant: Option<String>,

    /// Replace the voicing, a field of its own that only the newer stream carries.
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
    #[arg(long, value_enum, value_name = "BANK")]
    pub drop_bank: Vec<DroppableBank>,

    /// `N` keeps the N loudest layers of each root and bank; `=0,3,7` keeps exactly
    /// those layer values. Surviving layers keep the numbers they had.
    #[arg(long, value_name = "N|=LIST")]
    pub layers: Option<String>,

    /// `LO..HI`, inclusive: uncover every key outside it and drop the roots nothing
    /// plays any more. Both ends take a note name or a number.
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
    /// 0 attack, 1 pedal resonance, 2 release; `l00`, `l01`, … count from the loudest
    /// and are spread over the layer values 0..27 the instrument selects by. Write
    /// `v12` in place of `l00` to state a layer's value outright; one root's bank
    /// names all its layers the same way. Files that are not WAVs are skipped, and a
    /// WAV named some other way is refused.
    ///
    /// A stroke holds whole blocks and every frame of its WAV, so it states the
    /// silence that fills out the block the WAV ends in; the `frames` column below is
    /// the WAV's, not the stroke's.
    #[arg(value_name = "DIR")]
    pub dir: PathBuf,

    /// The library to take everything the audio does not decide from: the length
    /// marks, the decay coefficients, the per-note tables, the stream version and the
    /// word at the body's start. Each new stroke inherits from the template stroke of
    /// its own bank and nearest root.
    #[arg(long, value_name = "FILE")]
    pub template: PathBuf,

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

#[derive(Args)]
pub struct RebuildArgs {
    /// The piano library to code again from its own audio.
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
fn read(path: &Path) -> Result<(Vec<u8>, npno::Piano), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    match entity {
        Entity::Piano(piano) => Ok((bytes, piano)),
        other => Err(format!(
            "{}: this is a {}, not a piano library (.npno)",
            path.display(),
            entity_kind(&other)
        )),
    }
}

fn entity_kind(entity: &Entity) -> &'static str {
    match entity {
        Entity::Sample(_) => "sample instrument",
        Entity::SampleProject(_) => "Sample Editor project",
        Entity::Program(_) => "program",
        Entity::Live(_) => "live slot",
        Entity::Settings(_) => "settings file",
        _ => "file of another format",
    }
}

/// A trim or a split writes a new library; overwriting the one it reads would
/// leave nothing to compare against, and no flag says that was meant.
fn refuse_in_place(input: &Path, output: &Path) -> Result<(), String> {
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    if same(input, output) {
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
    let mut failed = 0usize;
    for (i, path) in args.files.iter().enumerate() {
        if i > 0 {
            ui.out("");
        }
        ui.out(ui.bold(path.display()));
        match inspect_one(ui, path, &args) {
            Ok(()) => {}
            Err(e) => {
                failed += 1;
                ui.note(format!("  {} {e}", ui.danger("error")));
            }
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} of {} file(s) did not read", args.files.len())),
    }
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
        "  {title}  stream {:#06x}  {} channel(s)  {} bytes",
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
                .filter(|(_, s)| {
                    args.bank
                        .is_none_or(|b| s.bank_code() == Bank::from(b).code())
                })
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
    if let Some(parent) = args.out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    write_file(ui, &args.out, &wav)?;

    let peak = audio
        .channels
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
    match args.out {
        Some(out) => write_file(ui, &out, &edited),
        None => {
            ui.note(format!(
                "about to {} {} in place",
                ui.danger("overwrite"),
                args.file.display()
            ));
            ui.confirm(args.yes)?;
            write_file(ui, &args.file, &edited)?;
            ui.note(format!("{} bytes in, {} out", original.len(), edited.len()));
            Ok(())
        }
    }
}

/// `-4` is fine-tune units; `+2.1c` is cents, rounded to the nearest unit.
fn parse_tune(value: &str) -> Result<i8, String> {
    if let Some(cents) = value.strip_suffix(['c', 'C']) {
        let cents: f32 = cents
            .parse()
            .map_err(|_| format!("{value:?} is not a number of cents"))?;
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
    for bank in &args.drop_bank {
        let bank = Bank::from(*bank);
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
            "{} key(s) are left playing nothing, and {} root(s) dropped out entirely",
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

/// `N` keeps the N loudest; `=0,3,7` keeps exactly those layer values.
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
    let mut failed = 0usize;
    let mut strokes = 0usize;
    let mut frames = 0usize;
    let mut overlap = 0usize;
    for path in &args.files {
        match verify_one(path, args.deep) {
            Ok(counted) => {
                strokes += counted.strokes;
                frames += counted.frames;
                overlap += counted.overlap;
                ui.out(format!(
                    "ok     {} ({})",
                    path.display(),
                    counted.line(args.deep)
                ));
            }
            Err(line) => {
                failed += 1;
                ui.out(format!(
                    "{} {} ({line})",
                    ui.danger("FAILED"),
                    path.display()
                ));
            }
        }
    }
    if args.deep {
        ui.note(format!(
            "{strokes} stroke(s), {frames} frame(s) decoded, {overlap} repeated sample(s) \
             matched the block before"
        ));
    }
    match failed {
        0 => Ok(()),
        n => Err(format!(
            "{n} of {} file(s) did not check out",
            args.files.len()
        )),
    }
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
        let at = rebuilt
            .iter()
            .zip(&original)
            .position(|(a, b)| a != b)
            .map(|i| format!("{i:#x}"))
            .unwrap_or_else(|| "the length".to_string());
        return Err(format!(
            "the rebuild differs at {at}; in {} bytes, out {}",
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
                    "{stroke:?}: {} sample(s) left int16",
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

/// What a WAV's third name component says about its velocity layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LayerName {
    /// `l02`: the third-loudest layer of its root and bank, taking whatever value the
    /// spread over that root's layers gives it.
    Index(u8),
    /// `v12`: the layer value itself, written to the record as it stands.
    Value(u8),
}

/// One WAV a build reads, and what its name says the stroke is.
struct StrokeFile {
    path: PathBuf,
    root: u8,
    bank: Bank,
    layer: LayerName,
}

/// `<root>-b<bank>-l<layer>` or `<root>-b<bank>-v<value>`, as in `060-b0-l00`.
fn parse_stroke_name(stem: &str) -> Option<(u8, Bank, LayerName)> {
    let mut parts = stem.split('-');
    let root = parts.next()?.parse().ok()?;
    let bank = Bank::from_code(parts.next()?.strip_prefix('b')?.parse().ok()?)?;
    let third = parts.next()?;
    let layer = if let Some(index) = third.strip_prefix('l') {
        LayerName::Index(index.parse().ok()?)
    } else {
        LayerName::Value(third.strip_prefix('v')?.parse().ok()?)
    };
    parts.next().is_none().then_some((root, bank, layer))
}

/// The WAVs in a directory, with what their names say each one is, in stroke order.
fn stroke_files(dir: &Path) -> Result<Vec<StrokeFile>, String> {
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
        let (root, bank, layer) = parse_stroke_name(&stem).ok_or_else(|| {
            format!(
                "{}: a WAV here is named <root>-b<bank>-l<layer>.wav, as in \
                 060-b0-l00.wav — MIDI note 60, bank 0 (attack), layer 0; \
                 <root>-b<bank>-v<value>.wav states the layer value instead",
                path.display()
            )
        })?;
        out.push(StrokeFile {
            path,
            root,
            bank,
            layer,
        });
    }
    if out.is_empty() {
        return Err(format!("{}: no WAV to build a library from", dir.display()));
    }
    out.sort_by(|a, b| {
        (a.root, a.bank.code(), a.layer, &a.path).cmp(&(b.root, b.bank.code(), b.layer, &b.path))
    });
    Ok(out)
}

/// The layer value each file's stroke states, in the order the files came in.
///
/// A `v` name is that value; an `l` name is spread across its root and bank's own
/// layers, loudest first. The two forms would each mean something different about how
/// many layers a spread is over, so one root's bank names its layers one way.
///
/// A `v` name past [`encode::HIGHEST_PLAYED_LAYER`] is a stroke no velocity would
/// reach, and is refused by the file that names it.
fn layer_values(files: &[StrokeFile]) -> Result<Vec<u8>, String> {
    let mut groups: BTreeMap<(u8, u8), Vec<usize>> = BTreeMap::new();
    for (index, file) in files.iter().enumerate() {
        groups
            .entry((file.root, file.bank.code()))
            .or_default()
            .push(index);
    }

    let mut values = vec![0u8; files.len()];
    for ((root, bank), members) in groups {
        let states = members
            .iter()
            .filter(|&&i| matches!(files[i].layer, LayerName::Value(_)))
            .count();
        let what = format!("root {} {}", note::name(root), bank_label(bank));
        if states != 0 && states != members.len() {
            return Err(format!(
                "{what} names some of its layers by index (l..) and some by value \
                 (v..); one root's bank names them one way"
            ));
        }
        if members
            .iter()
            .map(|&i| files[i].layer)
            .collect::<BTreeSet<_>>()
            .len()
            != members.len()
        {
            return Err(format!("{what} names one of its layers twice"));
        }
        for (rank, &index) in members.iter().enumerate() {
            values[index] = match files[index].layer {
                LayerName::Value(value) => value,
                LayerName::Index(_) => encode::layer_value(rank, members.len()),
            };
            if values[index] > encode::HIGHEST_PLAYED_LAYER {
                return Err(format!(
                    "{}: no velocity selects layer value {}; {} is the largest a key ever                      sounds",
                    files[index].path.display(),
                    values[index],
                    encode::HIGHEST_PLAYED_LAYER
                ));
            }
        }
    }
    Ok(values)
}

pub fn build(ui: &Ui, args: BuildArgs) -> Result<(), String> {
    refuse_in_place(&args.template, &args.out)?;
    let (_, donor) = read(&args.template)?;
    let template = donor.library().map_err(|e| e.to_string())?;

    ui.out(ui.dim(format!(
        "  {:>26}  {:>6} {:>10} {:>5} {:>9} {:>8}",
        "wav", "root", "bank", "value", "frames", "seconds"
    )));
    let files = stroke_files(&args.dir)?;
    let values = layer_values(&files)?;
    let mut recordings = Vec::new();
    let mut clipped = 0usize;
    let mut resampled = 0usize;
    for (file, layer) in files.iter().zip(values) {
        let path = &file.path;
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let pcm =
            nord_format::wav::read_pcm16(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let audio = encode::resample(&pcm.samples, usize::from(pcm.channels), pcm.rate)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        clipped += audio.clipped;
        resampled += usize::from(pcm.rate != codec::RATE);
        let frames = audio.channels.first().map_or(0, Vec::len);
        ui.out(format!(
            "  {:>26}  {:>6} {:>10} {:>5} {:>9} {:>8.3}{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            note::name(file.root),
            file.bank.name(),
            layer,
            frames,
            frames as f64 / f64::from(codec::RATE),
            if pcm.rate == codec::RATE {
                String::new()
            } else {
                format!("  from {} Hz", pcm.rate)
            }
        ));
        recordings.push(encode::Recording {
            root: file.root,
            bank: file.bank,
            layer,
            channels: audio.channels,
        });
    }

    let options = encode::Options::new(&args.name).variant(&args.variant);
    let library = encode::build(&template, &options, &recordings).map_err(|e| e.to_string())?;
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
            "{resampled} WAV(s) were resampled onto the {} Hz lattice the instrument \
             plays at",
            codec::RATE
        ));
    }
    if clipped > 0 {
        ui.note(format!(
            "{clipped} resampled sample(s) saturated at int16; the source is loud \
             enough that the kernel overshoots it"
        ));
    }
    ui.note(format!(
        "{} states the length marks, the decay coefficients, the per-note tables and \
         the word at the body's start as the template donated them; the instrument \
         accepts them, and what it makes of them beyond accepting is not known",
        args.out.display()
    ));
    Ok(())
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
            "{restated} block(s) declare a different attenuation. It is a statistic \
             the file's own encoder measured, not a function of the frames it stored, \
             and the decode never reads it"
        ));
    }
    if recoded > 0 {
        ui.note(format!(
            "{} {recoded} block(s) came back with different residuals, a different \
             width or a different order — this library was not laid out the way the \
             coder lays one out",
            ui.danger("warning:")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stroke_wav_names_its_root_bank_and_layer() {
        use LayerName::{Index, Value};
        assert_eq!(
            parse_stroke_name("060-b0-l00"),
            Some((60, Bank::Attack, Index(0)))
        );
        assert_eq!(
            parse_stroke_name("36-b2-l7"),
            Some((36, Bank::Release, Index(7)))
        );
        assert_eq!(
            parse_stroke_name("101-b1-v12"),
            Some((101, Bank::Resonance, Value(12)))
        );
        assert_eq!(parse_stroke_name("060-b3-l00"), None, "no such bank");
        assert_eq!(parse_stroke_name("300-b0-l00"), None, "no such note");
        assert_eq!(parse_stroke_name("060-0-l00"), None);
        assert_eq!(parse_stroke_name("060-b0-x2"), None, "no such layer form");
        assert_eq!(parse_stroke_name("060-b0"), None);
        assert_eq!(parse_stroke_name("060-b0-l00-take2"), None);
        assert_eq!(
            parse_stroke_name("C4-b0-l00"),
            None,
            "notes are numbers here"
        );
    }

    fn wav(root: u8, bank: Bank, layer: LayerName) -> StrokeFile {
        StrokeFile {
            path: PathBuf::new(),
            root,
            bank,
            layer,
        }
    }

    /// The velocity a layer answers to is its value, so the directory of WAVs decides
    /// which part of the range each recording plays over.
    #[test]
    fn indexed_layers_spread_over_their_own_root_and_bank() {
        let files = [
            wav(60, Bank::Attack, LayerName::Index(0)),
            wav(60, Bank::Attack, LayerName::Index(1)),
            wav(60, Bank::Attack, LayerName::Index(2)),
            wav(60, Bank::Release, LayerName::Index(0)),
            wav(72, Bank::Attack, LayerName::Index(0)),
            wav(72, Bank::Attack, LayerName::Index(1)),
        ];
        assert_eq!(layer_values(&files).unwrap(), [0, 14, 27, 0, 0, 27]);
    }

    #[test]
    fn a_named_layer_value_is_written_as_it_stands() {
        let files = [
            wav(60, Bank::Attack, LayerName::Value(0)),
            wav(60, Bank::Attack, LayerName::Value(6)),
            wav(60, Bank::Attack, LayerName::Value(12)),
        ];
        assert_eq!(layer_values(&files).unwrap(), [0, 6, 12]);
    }

    /// `v255` parses and is a layer no key would ever sound, so the name is refused
    /// rather than built into a library as a stroke nothing plays.
    #[test]
    fn a_named_layer_value_no_velocity_selects_is_refused() {
        let highest = encode::HIGHEST_PLAYED_LAYER;
        assert_eq!(
            layer_values(&[wav(60, Bank::Attack, LayerName::Value(highest))]).unwrap(),
            [highest]
        );
        let refused = layer_values(&[wav(60, Bank::Attack, LayerName::Value(255))]).unwrap_err();
        assert!(refused.contains("no velocity selects"), "{refused}");
        assert!(refused.contains(&highest.to_string()), "{refused}");
    }

    #[test]
    fn one_root_and_bank_names_its_layers_one_way() {
        let mixed = [
            wav(60, Bank::Attack, LayerName::Index(0)),
            wav(60, Bank::Attack, LayerName::Value(12)),
        ];
        let refused = layer_values(&mixed).unwrap_err();
        assert!(refused.contains("C4 attack"), "{refused}");

        let twice = [
            wav(60, Bank::Attack, LayerName::Index(0)),
            wav(60, Bank::Attack, LayerName::Index(0)),
        ];
        assert!(layer_values(&twice).is_err(), "a layer named twice");
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
}
