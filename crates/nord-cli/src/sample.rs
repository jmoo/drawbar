//! `nord sample` — the verbs that only mean anything for a sample instrument.
//!
//! `edit` mirrors `nord program edit`, but the fields come from
//! [`editors::SampleEditor`]'s accessors rather than a declarative panel: a
//! sample is mostly encoded audio, and only what the format crate can patch in
//! place is settable — the name, and each zone's root key and top note.
//! `decode` turns the audio back into WAV, and `verify --deep` walks the
//! encoded stream rather than only the container.
//!
//! `encode` builds a one-zone instrument from a WAV; `build` builds a whole one
//! from a Sample Editor project, which is where the zones, root keys, top notes
//! and trim points come from instead of the command line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::Args;
use nord_format::formats::nsmp::{codec, encode};
use nord_format::formats::nsmpproj::Project;
use nord_format::Entity;
use nord_usb::ObjectClass;

use crate::edit::{print_byte_diff, write_file};
use crate::editors::{self, SampleEditor};
use crate::note;
use crate::slot::Target;
use crate::ui::Ui;

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
    /// The sample instruments to decode: `.nsmp`, `.nsmp3` or `.nsmp4`.
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

    /// Use the narrowest predictor order per cell. Smaller, and decoded exactly.
    #[arg(long)]
    pub predict: bool,

    /// Acknowledge that this codec tier is unproven on hardware. Required.
    #[arg(long)]
    pub experimental: bool,
}

#[derive(Args)]
pub struct BuildArgs {
    /// A Nord Sample Editor project (`.nsmpproj`). The audio paths inside it
    /// resolve from the project's own directory.
    #[arg(value_name = "PROJECT")]
    pub project: PathBuf,

    /// Where to write the instrument. Defaults to the project's path with `.nsmp`.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Instrument name, up to 14 bytes. Defaults to the project's own.
    #[arg(long)]
    pub name: Option<String>,

    /// Use the narrowest predictor order per cell. Smaller, and decoded exactly.
    #[arg(long)]
    pub predict: bool,

    /// Acknowledge that this codec tier is unproven on hardware. Required.
    #[arg(long)]
    pub experimental: bool,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// The sample instruments to check: `.nsmp`, `.nsmp3` or `.nsmp4`.
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

/// The sample body of a file, or why this command cannot reach one.
fn body(path: &Path) -> Result<nord_format::Sample, String> {
    let entity = nord_format::from_path(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match entity {
        Entity::Sample(sample) => Ok(sample),
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
    let body = body(path)?;
    let layout = body.layout();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sample".into());

    for (index, zone) in body.zones().map_err(|e| e.to_string())?.iter().enumerate() {
        coverage.zones += 1;
        let n = index + 1;
        let head = format!(
            "  zone{n:<2} root {:<4} top {:<4}",
            note::name(zone.root_key),
            note::name(zone.top_note),
        );
        match codec::decode(zone.stream, zone.at, layout) {
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
                    let wav =
                        nord_format::wav::pcm16(&audio.samples, codec::FIELD_RATE, audio.channels)
                            .map_err(|e| format!("{}: {e}", file.display()))?;
                    std::fs::write(&file, wav).map_err(|e| format!("{}: {e}", file.display()))?;
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

/// The gate every writing verb in this module sits behind.
fn experimental(acknowledged: bool) -> Result<(), String> {
    if acknowledged {
        return Ok(());
    }
    Err(
        "encoding is experimental: the file it writes is structurally sound and \
         decodes back exactly, but it is not byte-identical to the editor's output \
         and no instrument has been shown to play one. Pass --experimental to write \
         it anyway."
            .into(),
    )
}

/// One WAV as the encoder needs it: mono 16-bit at [`codec::SOURCE_RATE`].
fn mono_source(path: &Path) -> Result<nord_format::wav::Pcm16, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let source =
        nord_format::wav::read_pcm16(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if source.rate != codec::SOURCE_RATE {
        return Err(format!(
            "{}: {} Hz — the field lattice is defined against {} Hz, and the instrument's \
             own resampler is not decoded, so resample the WAV first",
            path.display(),
            source.rate,
            codec::SOURCE_RATE,
        ));
    }
    if source.channels != 1 {
        return Err(format!(
            "{}: {} channels — only mono is encoded; a stroke pair under one header is \
             how the format carries stereo and that layout is not written yet",
            path.display(),
            source.channels,
        ));
    }
    Ok(source)
}

fn predictor(minimising: bool) -> encode::Predictor {
    if minimising {
        encode::Predictor::Minimising
    } else {
        encode::Predictor::Plain
    }
}

/// What one encoded stroke came out as, for the report.
fn stroke_line(stream: &[u8], at: usize) -> Result<String, String> {
    let layout = codec::Layout::V2;
    let walk = codec::walk(stream, at, layout).map_err(|e| e.to_string())?;
    let audio = codec::decode(stream, at, layout).map_err(|e| e.to_string())?;
    Ok(format!(
        "{:>8} fields  {:>7.3} s  shift {}, peak {}, {} record(s), {}% predicted",
        walk.fields,
        audio.seconds(),
        codec::shift(stream, layout).unwrap_or_default(),
        codec::peak(stream, layout).unwrap_or_default(),
        walk.records.len(),
        100 * audio.differenced / audio.samples.len().max(1),
    ))
}

/// `nord sample encode`: a WAV into a one-zone v2 instrument.
pub fn encode(ui: &Ui, args: EncodeArgs) -> Result<(), String> {
    experimental(args.experimental)?;
    let source = mono_source(&args.wav)?;

    let stem = args
        .wav
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sample".into());
    let name = args.name.unwrap_or_else(|| stem.clone());
    let mut options = encode::Options::new(&name)
        .root_key(note::parse(&args.root_key)?)
        .predictor(predictor(args.predict));
    if let Some(top) = &args.top_note {
        options = options.top_note(note::parse(top)?);
    }

    let instrument = encode::instrument(&source.samples, &options).map_err(|e| e.to_string())?;
    let out = instrument.to_bytes().map_err(|e| e.to_string())?;

    let (at, stroke) = instrument.stroke_streams()[0];
    ui.out(format!(
        "{} frames -> {}",
        source.frames(),
        stroke_line(stroke, at)?
    ));

    let path = args
        .out
        .unwrap_or_else(|| args.wav.with_file_name(format!("{stem}.nsmp")));
    write_file(ui, &path, &out)
}

/// One zone of a project, resolved: the audio region it plays and where on the
/// keyboard it plays it.
struct ProjectZone {
    global_id: u32,
    root_key: u8,
    top_note: u8,
    samples: Vec<i16>,
    source: PathBuf,
}

/// `nord sample build`: a Sample Editor project into the instrument it describes.
pub fn build(ui: &Ui, args: BuildArgs) -> Result<(), String> {
    experimental(args.experimental)?;

    let project = match nord_format::from_path(&args.project)
        .map_err(|e| format!("{}: {e}", args.project.display()))?
    {
        Entity::SampleProject(project) => project,
        other => {
            return Err(format!(
                "{}: a {} file, not a Sample Editor project",
                args.project.display(),
                other.identity().format
            ))
        }
    };

    let dir = args.project.parent().unwrap_or_else(|| Path::new("."));
    let resolved = project_zones(&project, dir)?;
    let name = match args.name {
        Some(name) => name,
        None => project.name().map_err(|e| e.to_string())?,
    };

    let zones: Vec<encode::NewZone> = resolved
        .iter()
        .map(|z| encode::NewZone {
            source: &z.samples,
            root_key: z.root_key,
            top_note: z.top_note,
            global_id: z.global_id,
        })
        .collect();
    let instrument =
        encode::multi_zone(&zones, &name, predictor(args.predict)).map_err(|e| e.to_string())?;
    let out = instrument.to_bytes().map_err(|e| e.to_string())?;

    ui.out(format!("{} — {} zone(s)", ui.bold(&name), zones.len()));
    for (index, zone) in resolved.iter().enumerate() {
        let (at, stream) = instrument.zone_stream(index).map_err(|e| e.to_string())?;
        ui.out(format!(
            "  zone{:<2} root {:<4} top {:<4} {}",
            index + 1,
            note::name(zone.root_key),
            note::name(zone.top_note),
            stroke_line(stream, at)?,
        ));
        ui.out(ui.dim(format!(
            "         stroke {} from {}",
            zone.global_id,
            zone.source.display()
        )));
    }

    let path = args
        .out
        .unwrap_or_else(|| args.project.with_extension("nsmp"));
    write_file(ui, &path, &out)
}

/// Resolve a project's zones, highest first, into audio and keyboard placement.
///
/// Everything the editor can express that this writer does not lay out is refused by
/// name here rather than dropped: the file it would otherwise produce would be a
/// silent reinterpretation of the project.
fn project_zones(project: &Project, dir: &Path) -> Result<Vec<ProjectZone>, String> {
    let say = |e: nord_format::error::ParseError| e.to_string();
    let files = project.audio_files().map_err(say)?;
    let strokes = project.strokes().map_err(say)?;
    let zones = project.zones().map_err(say)?;

    zones
        .iter()
        .enumerate()
        .map(|(index, zone)| {
            let at = format!("zone{}", index + 1);
            if !zone.enabled {
                return Err(format!(
                    "{at} is switched off in the project; turn it on or remove it — an \
                     instrument has no way to carry a zone that does not sound"
                ));
            }
            let [layer] = zone.strokes.as_slice() else {
                return Err(format!(
                    "{at} plays {} strokes, which is a velocity split or a round robin; \
                     one stroke per zone is the layout this writer lays down",
                    zone.strokes.len()
                ));
            };
            if !layer.enabled {
                return Err(format!("{at}'s only stroke is switched off"));
            }
            if layer.gain != 1.0 || layer.detune != 0 || layer.velocity != (0, 127) {
                return Err(format!(
                    "{at} sets gain {}, detune {} and velocity {}..={} on its stroke; \
                     where the instrument applies those is not decoded, so nothing here \
                     reproduces them",
                    layer.gain, layer.detune, layer.velocity.0, layer.velocity.1
                ));
            }
            let stroke = strokes
                .iter()
                .find(|s| s.global_id == layer.global_id)
                .ok_or_else(|| {
                    format!(
                        "{at} names stroke {}, which the project does not hold",
                        layer.global_id
                    )
                })?;
            if stroke.loop_enabled {
                return Err(format!(
                    "{at}'s stroke loops; a looping zone carries loop fields in its zone \
                     record that are not decoded, so it cannot be written"
                ));
            }
            let file = files
                .iter()
                .find(|f| f.id == stroke.file_id)
                .ok_or_else(|| {
                    format!(
                        "{at} plays audio file {}, which the project does not hold",
                        stroke.file_id
                    )
                })?;

            let path = dir.join(&file.path);
            let source = mono_source(&path)?;
            let frames = source.frames();
            let start = frame(&at, "start", stroke.start, frames)?;
            let stop = frame(&at, "stop", stroke.stop, frames)?;
            if start >= stop {
                return Err(format!(
                    "{at} plays frames {start}..{stop} of {}, which is nothing",
                    path.display()
                ));
            }
            Ok(ProjectZone {
                global_id: layer.global_id,
                root_key: zone.root_key,
                top_note: zone.top_note,
                samples: source.samples[start..stop].to_vec(),
                source: path,
            })
        })
        .collect()
}

/// A project's frame position as an index into the file it points at.
///
/// Positions are `%f` decimals counted at 44 100 Hz whatever the file's own rate says.
/// Inferred from the editor's field counts: a zone encodes `start..stop`, not the
/// whole `begin..end` extent.
fn frame(zone: &str, label: &str, value: f64, frames: usize) -> Result<usize, String> {
    let rounded = value.round();
    if !(0.0..=frames as f64).contains(&rounded) {
        return Err(format!(
            "{zone}'s {label} is at frame {value}, outside the {frames} frames its audio holds"
        ));
    }
    Ok(rounded as usize)
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

/// Walks every stroke and verifies all four directory landmarks.
fn deep(path: &Path) -> Result<String, String> {
    let body = body(path)?;
    let layout = body.layout();
    let streams = body.stroke_streams();
    let mut records = 0usize;
    let mut marked = 0usize;
    for (index, (at, stroke)) in streams.iter().enumerate() {
        let stream =
            codec::walk(stroke, *at, layout).map_err(|e| format!("stroke {index}: {e}"))?;
        records += stream.records.len();
        marked += stream.records.iter().filter(|r| r.mark).count();
        let directory = codec::Directory::read(stroke)
            .ok_or_else(|| format!("stroke {index} is too short for its word directory"))?;
        let words = (stroke.len() - layout.header_len()) / layout.word();
        let first = codec::Directory::resolve(directory.first_record, *at, layout);
        let terminator = codec::Directory::resolve_end(directory.terminator, *at, layout, words);
        if first != stream.first_record || terminator != stream.terminator {
            return Err(format!(
                "stroke {index}: directory says {first}..{terminator}, walk found {}..{}",
                stream.first_record, stream.terminator
            ));
        }
        let names = |pointer: u16, word: usize| {
            word % codec::WRAP == codec::Directory::resolve(pointer, *at, layout) % codec::WRAP
        };
        if !names(directory.resync, stream.terminator)
            && !stream.records.iter().any(|r| names(directory.resync, r.at))
        {
            return Err(format!("stroke {index}: resync does not name a record"));
        }
        if !names(directory.mark, stream.terminator)
            && !stream.records.iter().any(|r| names(directory.mark, r.at))
        {
            return Err(format!("stroke {index}: mark does not name a record"));
        }
        let actual: Vec<_> = stream.records.iter().filter(|r| r.mark).collect();
        if actual.len() > 1 || actual.first().is_some_and(|r| !names(directory.mark, r.at)) {
            return Err(format!(
                "stroke {index}: marked record disagrees with the directory"
            ));
        }
    }
    let mut note = format!(
        "{}, {} stroke(s), {records} record(s), directory agrees",
        body.generation(),
        streams.len()
    );
    if marked > 0 {
        note.push_str(&format!(", {marked} marked"));
    }
    Ok(note)
}
