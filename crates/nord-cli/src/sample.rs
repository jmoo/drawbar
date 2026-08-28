//! `nord sample` — the verbs that only mean anything for a sample instrument.
//!
//! `edit` mirrors `nord program edit`, but the fields come from
//! [`editors::SampleEditor`]'s accessors rather than a declarative panel: a
//! sample is mostly encoded audio, and only what the format crate can patch in
//! place is settable — the name, and each zone's root key and top note.
//! `decode` turns the audio back into WAV, and `verify --deep` walks the
//! encoded stream rather than only the container.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::Args;
use nord_format::cbin::Cbin;
use nord_format::formats::nsmp::{codec, encode, Sample};
use nord_format::Entity;
use nord_usb::ObjectClass;

use crate::edit::{print_byte_diff, write_file};
use crate::editors::{self, SampleEditor};
use crate::note;
use crate::slot::Target;
use crate::ui::Ui;
use crate::wav;

#[derive(Args)]
pub struct EditArgs {
    /// A `.nsmp` file, or a slot on the instrument (`1:14`). A slot makes this a
    /// read-modify-write over USB, so it is a mutation and obeys `--yes`.
    #[arg(value_name = "FILE|BANK:SLOT")]
    pub target: String,

    /// `path=value`, repeatable: `name=NAME`, `zone1.root_key=NOTE`,
    /// `zone1.top_note=NOTE`. Notes are names (`C4`, `F#3`) or numbers (0-127).
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub set: Vec<String>,

    /// Report what would change — including which bytes — and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// List every settable field with its current value, then exit.
    #[arg(long)]
    pub fields: bool,

    /// Write the edited sample here instead of over the input file.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Confirm the write. Editing a slot, or a file in place, needs it.
    #[arg(long)]
    pub yes: bool,
}

pub fn run(ui: &Ui, args: EditArgs) -> Result<(), String> {
    let target = crate::slot::target(&args.target)?;

    let original = match &target {
        Target::File(path) => {
            std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Target::Slot(at) => crate::device::fetch(*at, ObjectClass::Sample)?,
    };

    let mut entity = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .map_err(|e| e.to_string())?;
    let sample = match &mut entity {
        Entity::Sample(nord_format::Sample::V2(sample)) => sample,
        // Editing needs the v2 zone/stroke accessors; the v3/v4 chain is read-only.
        Entity::Sample(nord_format::Sample::V3(_)) => {
            return Err(
                "this instrument is nsmp3/nsmp4 content; only v2 (.nsmp) can be edited".into(),
            )
        }
        Entity::SampleProject(_) => {
            return Err(
                "this is a Sample Editor project, not a sample instrument — try `nord edit`".into(),
            )
        }
        _ => return Err("sample edit only understands sample instruments (.nsmp)".into()),
    };

    let Some(changed) = editors::stage(ui, args.fields, &args.set, &mut SampleEditor(sample))?
    else {
        // `--fields` has listed them and is done.
        return Ok(());
    };
    if changed == 0 {
        ui.note("no field changed; writing nothing");
        return Ok(());
    }

    let edited = nord_format::to_bytes(&entity).map_err(|e| e.to_string())?;
    print_byte_diff(ui, &original, &edited);

    if args.dry_run {
        ui.note("--dry-run: nothing written");
        return Ok(());
    }

    match (target, args.out) {
        // An explicit destination is the unambiguous case, whatever the source was.
        (_, Some(out)) => write_file(ui, &out, &edited),
        (Target::File(path), None) => {
            ui.note(format!(
                "about to {} {} in place",
                ui.danger("overwrite"),
                path.display()
            ));
            ui.confirm(args.yes)?;
            write_file(ui, &path, &edited)
        }
        (Target::Slot(at), None) => crate::device::send(
            ui,
            &edited,
            at,
            ObjectClass::Sample,
            args.yes,
            "the edited sample",
            None,
            None,
        ),
    }
}

#[derive(Args)]
pub struct DecodeArgs {
    /// The `.nsmp` files to decode.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,

    /// Write one WAV per zone into this directory. Without it, nothing is written
    /// and the run is a coverage report.
    #[arg(short, long, value_name = "DIR")]
    pub out: Option<PathBuf>,
}

#[derive(Args)]
pub struct EncodeArgs {
    /// A 44.1 kHz mono 16-bit PCM WAV.
    #[arg(value_name = "WAV")]
    pub wav: PathBuf,

    /// Where to write the instrument. Defaults to the WAV's name with `.nsmp`.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Instrument name, up to 14 bytes. Defaults to the WAV's file stem.
    #[arg(long)]
    pub name: Option<String>,

    /// The note the sample plays untransposed at: a name (`C4`, `F#3`) or 0-127.
    #[arg(long, value_name = "NOTE", default_value = "C4")]
    pub root_key: String,

    /// The highest note the zone covers. Defaults to two octaves above the root.
    #[arg(long, value_name = "NOTE")]
    pub top_note: Option<String>,

    /// Difference content records down to the narrowest predictor order, the way the
    /// instrument's own encoder does.
    ///
    /// ⚠️ Smaller, and `nord sample decode` reads the result back only approximately:
    /// where a differenced run resumes from is not recorded in the stream and the rule
    /// for recovering it is unsolved.
    #[arg(long)]
    pub predict: bool,

    /// Acknowledge that this codec tier is unproven on hardware. Required.
    #[arg(long)]
    pub experimental: bool,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// The `.nsmp` files to check.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,

    /// Also walk each stroke's encoded stream, and check the walk against the
    /// stroke header's own word directory.
    #[arg(long)]
    pub deep: bool,
}

/// How a run of zones came out: decoded, or refused with a reason worth counting.
#[derive(Default)]
struct Coverage {
    files: usize,
    zones: usize,
    decoded: usize,
    fields: usize,
    differenced: usize,
    reasons: BTreeMap<&'static str, usize>,
}

impl Coverage {
    fn refuse(&mut self, reason: &'static str) {
        *self.reasons.entry(reason).or_default() += 1;
    }

    fn line(&self) -> String {
        let unsupported: usize = self.reasons.values().sum();
        let mut line = format!(
            "{} file(s), {} zone(s): {} decoded, {unsupported} unsupported",
            self.files, self.zones, self.decoded
        );
        if self.fields > 0 {
            line.push_str(&format!(
                "; {:.1}% of decoded fields came through the predictor",
                100.0 * self.differenced as f64 / self.fields as f64,
            ));
        }
        if !self.reasons.is_empty() {
            let detail: Vec<String> = self
                .reasons
                .iter()
                .map(|(reason, n)| format!("{reason} {n}"))
                .collect();
            line.push_str(&format!(" ({})", detail.join(", ")));
        }
        line
    }
}

/// The v2 body of a file, or why this command cannot reach one.
fn v2(path: &Path) -> Result<Cbin<Sample>, String> {
    let entity = nord_format::from_path(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match entity {
        Entity::Sample(nord_format::Sample::V2(sample)) => Ok(sample),
        Entity::Sample(nord_format::Sample::V3(_)) => Err(format!(
            "{}: nsmp3/nsmp4 content — the codec constants for those generations are \
             not ported, so the audio stays encoded",
            path.display()
        )),
        other => Err(format!(
            "{}: a {} file, not a sample instrument",
            path.display(),
            other.identity().format
        )),
    }
}

/// `nord sample decode`: the encoded audio back to WAV, and what did not decode.
pub fn decode(ui: &Ui, args: DecodeArgs) -> Result<(), String> {
    if let Some(dir) = &args.out {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut coverage = Coverage::default();
    let mut failed = 0usize;
    for path in &args.files {
        ui.out(ui.bold(path.display().to_string()));
        match decode_file(ui, path, args.out.as_deref(), &mut coverage) {
            Ok(()) => coverage.files += 1,
            Err(e) => {
                failed += 1;
                // A whole file that will not open is not a codec gap, so it is
                // reported rather than counted against coverage.
                ui.out(format!("  {} {e}", ui.danger("error")));
            }
        }
    }
    ui.note(coverage.line());
    if failed == args.files.len() {
        return Err("nothing decoded".into());
    }
    Ok(())
}

fn decode_file(
    ui: &Ui,
    path: &Path,
    out: Option<&Path>,
    coverage: &mut Coverage,
) -> Result<(), String> {
    let sample = v2(path)?;
    let zones = sample.zones().map_err(|e| e.to_string())?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sample".into());

    let strokes = sample.strokes().map_err(|e| e.to_string())?;

    for (index, (zone, meta)) in zones.iter().zip(&strokes).enumerate() {
        coverage.zones += 1;
        let n = index + 1;
        let (at, stroke) = sample.zone_stream(index).map_err(|e| e.to_string())?;
        let head = format!(
            "  zone{n:<2} root {:<4} top {:<4}",
            note::name(meta.root_key),
            note::name(zone.top_note),
        );
        match codec::decode(stroke, at) {
            Ok(audio) => {
                coverage.decoded += 1;
                coverage.fields += audio.samples.len();
                coverage.differenced += audio.differenced;
                let mut notes = Vec::new();
                if audio.differenced > 0 {
                    notes.push(format!(
                        "{}% predicted",
                        100 * audio.differenced / audio.samples.len().max(1)
                    ));
                }
                if audio.clipped > 0 {
                    notes.push(format!("{} clipped", audio.clipped));
                }
                let mut row = format!(
                    "{head} {:>9} fields  {:>7.3} s  {}",
                    audio.samples.len(),
                    audio.seconds(),
                    ui.dim(notes.join(", ")),
                );
                if let Some(dir) = out {
                    let file = dir.join(format!("{stem}-zone{n}.wav"));
                    std::fs::write(&file, wav::mono_pcm16(&audio.samples, codec::FIELD_RATE))
                        .map_err(|e| format!("{}: {e}", file.display()))?;
                    row.push_str(&format!("  -> {}", file.display()));
                }
                ui.out(row);
            }
            Err(why) => {
                coverage.refuse(why.reason());
                ui.out(format!(
                    "{head} {} {}",
                    ui.danger("unsupported"),
                    ui.dim(why.to_string())
                ));
            }
        }
    }
    Ok(())
}

/// `nord sample encode`: a WAV into a one-zone v2 instrument.
pub fn encode(ui: &Ui, args: EncodeArgs) -> Result<(), String> {
    if !args.experimental {
        return Err(
            "encoding is experimental: the file it writes is structurally sound and \
             decodes back exactly, but it is not byte-identical to the editor's output \
             and no instrument has been shown to play one. Pass --experimental to write \
             it anyway."
                .into(),
        );
    }

    let bytes = std::fs::read(&args.wav).map_err(|e| format!("{}: {e}", args.wav.display()))?;
    let source = wav::read_pcm16(&bytes).map_err(|e| format!("{}: {e}", args.wav.display()))?;
    if source.rate != codec::SOURCE_RATE {
        return Err(format!(
            "{}: {} Hz — the field lattice is defined against {} Hz, and the instrument's \
             own resampler is not decoded, so resample the WAV first",
            args.wav.display(),
            source.rate,
            codec::SOURCE_RATE,
        ));
    }
    if source.channels != 1 {
        return Err(format!(
            "{}: {} channels — only mono is encoded; a stroke pair under one header is \
             how the format carries stereo and that layout is not written yet",
            args.wav.display(),
            source.channels,
        ));
    }

    let stem = args
        .wav
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sample".into());
    let name = args.name.unwrap_or_else(|| stem.clone());
    let mut options = encode::Options::new(&name)
        .root_key(note::parse(&args.root_key)?)
        .predictor(if args.predict {
            encode::Predictor::Minimising
        } else {
            encode::Predictor::Plain
        });
    if let Some(top) = &args.top_note {
        options = options.top_note(note::parse(top)?);
    }

    let instrument = encode::instrument(&source.samples, &options).map_err(|e| e.to_string())?;
    let out = instrument.to_bytes().map_err(|e| e.to_string())?;

    let (at, stroke) = instrument.stroke_streams()[0];
    let stream = codec::walk(stroke, at).map_err(|e| e.to_string())?;
    let audio = codec::decode(stroke, at).map_err(|e| e.to_string())?;
    ui.out(format!(
        "{} frames -> {} fields ({:.3} s), shift {}, peak {}, {} record(s)",
        source.frames(),
        stream.fields,
        audio.seconds(),
        codec::shift(stroke).unwrap_or_default(),
        codec::peak(stroke).unwrap_or_default(),
        stream.records.len(),
    ));
    ui.out(if audio.exact() {
        ui.dim("the stream reads back exactly: no record differences its fields")
    } else {
        ui.dim(format!(
            "{}% of fields sit in a differenced run, which decodes shape-correct and \
             level-approximate",
            100 * audio.differenced / audio.samples.len().max(1),
        ))
    });

    let path = args
        .out
        .unwrap_or_else(|| args.wav.with_file_name(format!("{stem}.nsmp")));
    write_file(ui, &path, &out)
}

/// `nord sample verify`: the container round trip, and with `--deep` the stream.
pub fn verify(ui: &Ui, args: VerifyArgs) -> Result<(), String> {
    let mut failed = 0usize;
    for path in &args.files {
        let original = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                failed += 1;
                ui.out(format!("error  {} ({e})", path.display()));
                continue;
            }
        };
        let round_trip = nord_format::from_path(path)
            .and_then(|entity| nord_format::to_bytes(&entity))
            .map_err(|e| e.to_string());
        match round_trip {
            Ok(bytes) if bytes == original => {}
            Ok(_) => {
                failed += 1;
                ui.out(format!(
                    "DIFFER {} (re-encode is not byte-identical)",
                    path.display()
                ));
                continue;
            }
            Err(e) => {
                failed += 1;
                ui.out(format!("error  {} ({e})", path.display()));
                continue;
            }
        }
        if !args.deep {
            ui.out(format!(
                "ok     {} ({} bytes)",
                path.display(),
                original.len()
            ));
            continue;
        }
        match deep(path) {
            Ok(note) => ui.out(format!("ok     {} ({note})", path.display())),
            Err(e) => {
                failed += 1;
                ui.out(format!("STREAM {} ({e})", path.display()));
            }
        }
    }
    if failed > 0 {
        return Err(format!(
            "{failed} of {} did not check out",
            args.files.len()
        ));
    }
    Ok(())
}

/// Walks every stroke's stream and cross-checks it against the header's directory.
///
/// The directory is written by the encoder and read by nothing else here, so
/// agreement between it and an independent walk is a real check on both.
fn deep(path: &Path) -> Result<String, String> {
    let sample = v2(path)?;
    let streams = sample.stroke_streams();
    let mut records = 0usize;
    for (index, (at, stroke)) in streams.iter().enumerate() {
        let stream = codec::walk(stroke, *at).map_err(|e| format!("stroke {index}: {e}"))?;
        records += stream.records.len();
        // The walk already had to land exactly on the record the directory names,
        // so what is left to check is the one pointer it does not consume.
        let directory = codec::Directory::read(stroke)
            .ok_or_else(|| format!("stroke {index} is too short for its word directory"))?;
        let resync = codec::Directory::resolve(directory.resync, *at);
        if !stream.records.iter().any(|r| r.at == resync) {
            return Err(format!(
                "stroke {index}: the resync pointer lands at word {resync}, which is not a record"
            ));
        }
    }
    Ok(format!(
        "{} stroke(s), {records} record(s), directory agrees",
        streams.len()
    ))
}
