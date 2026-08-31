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
use nord_format::formats::nsmpproj::{Project, Stroke, Zone, LOWEST_NOTE};
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

    /// Loop over `START:END`, in frames of the WAV. The audio after END is not
    /// encoded, and the loop's crossfade is applied to the samples themselves.
    #[arg(long = "loop", value_name = "START:END")]
    pub loop_points: Option<String>,

    /// Frames of the loop's tail to fade into the frames before its start.
    #[arg(
        long,
        value_name = "FRAMES",
        default_value_t = 0,
        requires = "loop_points"
    )]
    pub loop_crossfade: usize,

    /// Use the narrowest predictor order per cell. Smaller, and decoded exactly.
    #[arg(long)]
    pub predict: bool,

    /// Acknowledge that this is not a vendor-identical encode. Required.
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
         decodes back exactly, and single-zone unlooped output plays on an Electro 5, \
         but it is not byte-identical to the editor's output. Pass --experimental to \
         write it anyway."
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
    let mut line = format!(
        "{:>8} fields  {:>7.3} s  shift {}, peak {}, {} record(s), {}% predicted",
        walk.fields,
        audio.seconds(),
        codec::shift(stream, layout).unwrap_or_default(),
        codec::peak(stream, layout).unwrap_or_default(),
        walk.records.len(),
        100 * audio.differenced / audio.samples.len().max(1),
    );
    if let Some(record) = walk.records.iter().find(|r| r.mark) {
        let fields = walk.fields - record.first_field;
        line.push_str(&format!(
            ", loops the last {fields} field(s) ({:.3} s)",
            fields as f64 / f64::from(codec::FIELD_RATE),
        ));
    }
    Ok(line)
}

/// `START:END` in source frames.
fn loop_points(text: &str, crossfade: usize) -> Result<encode::Loop, String> {
    let number = |part: &str, label: &str| {
        part.trim()
            .parse::<usize>()
            .map_err(|_| format!("--loop wants START:END in frames; its {label} reads {part:?}"))
    };
    let (start, end) = text
        .split_once(':')
        .ok_or_else(|| format!("--loop wants START:END in frames, not {text:?}"))?;
    Ok(encode::Loop::new(number(start, "start")?, number(end, "end")?).crossfade(crossfade))
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
    if let Some(points) = &args.loop_points {
        options = options.loops(loop_points(points, args.loop_crossfade)?);
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
    loops: Option<encode::Loop>,
    /// Loop settings the project carries that the instrument has no field for, named
    /// so a build says what it dropped rather than dropping it quietly.
    dropped: Vec<String>,
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
            loops: z.loops,
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
        if !zone.dropped.is_empty() {
            ui.warn(format!(
                "zone{} sets {}, which the instrument has nowhere to hold",
                index + 1,
                zone.dropped.join(", ")
            ));
        }
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
    let instrument_decay = project.loop_decay_enabled().map_err(say)?;
    validate_key_ranges(&zones)?;

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
            let (loops, mut dropped) = zone_loop(&at, stroke, start, stop)?;
            if loops.is_some() && instrument_decay {
                dropped.push("the instrument's own m_loopDecayEnabled".into());
            }
            Ok(ProjectZone {
                global_id: layer.global_id,
                root_key: zone.root_key,
                top_note: zone.top_note,
                samples: source.samples[start..stop].to_vec(),
                source: path,
                loops,
                dropped,
            })
        })
        .collect()
}

/// One stroke's loop as the encoder states it, and the loop settings that reach no
/// instrument.
///
/// The project's loop points count in the audio file's own frames, so they move with
/// the trim. Whichever loop is switched on maps onto the one loop the container holds:
/// a short loop is the same start with the short length, and nothing in the file says
/// which of the two it was.
fn zone_loop(
    at: &str,
    stroke: &Stroke,
    start: usize,
    stop: usize,
) -> Result<(Option<encode::Loop>, Vec<String>), String> {
    if !stroke.loop_enabled {
        return Ok((None, Vec::new()));
    }
    let short = stroke.short_loop_enabled;
    let length = if short {
        stroke.loop_length_short
    } else {
        stroke.loop_length
    };
    let named = if short {
        "m_loopLengthShort"
    } else {
        "m_loopLengthLong"
    };
    let loop_start = frame(at, "loop start", stroke.loop_start, stop)?;
    if !length.is_finite() || length <= 0.0 {
        return Err(format!("{at}'s {named} is {length}, which is not a loop"));
    }
    let end = frame(at, "loop end", stroke.loop_start + length, stop)?;
    if loop_start < start {
        return Err(format!(
            "{at} loops from frame {loop_start} but its audio is trimmed to start at \
             {start}; the loop would begin before the sample does"
        ));
    }

    // Mode 1 rewrites the loop's tail some way this crate has not decoded, and the
    // short crossfade's unit is not frames, so neither can be reproduced.
    if !short && stroke.loop_crossfade_mode != 0 {
        return Err(format!(
            "{at} sets m_loopXFModeLong = {}; only the linear fade (mode 0) is decoded, \
             and the fade is baked into the audio, so this one cannot be written",
            stroke.loop_crossfade_mode
        ));
    }
    if short && stroke.loop_crossfade_short != 0 {
        return Err(format!(
            "{at} sets m_loopXFadeShort = {}, whose unit is not frames and is not \
             decoded; the fade is baked into the audio, so it cannot be written",
            stroke.loop_crossfade_short
        ));
    }
    let crossfade = if short {
        0
    } else {
        frame(at, "loop crossfade", stroke.loop_crossfade, stop)?
    };

    // These reach no instrument: the editor writes the same bytes whatever they hold.
    let mut dropped = Vec::new();
    if stroke.loop_detune != 0 {
        dropped.push(format!("m_loopDetune = {}", stroke.loop_detune));
    }
    if stroke.loop_decay_enabled {
        dropped.push(format!("m_loopDecay = {}", stroke.loop_decay));
    }
    if short && !stroke.short_loop_uses_pitch {
        dropped.push("m_shortLoopUsesPitch = 0".into());
    }
    Ok((
        Some(encode::Loop::new(loop_start - start, end - start).crossfade(crossfade)),
        dropped,
    ))
}

fn validate_key_ranges(zones: &[Zone]) -> Result<(), String> {
    for (index, zone) in zones.iter().enumerate() {
        let at = format!("zone{}", index + 1);
        if !(zone.bottom_note..=zone.top_note).contains(&zone.root_key) {
            return Err(format!(
                "{at}'s root note {} is outside its range {}..={}",
                zone.root_key, zone.bottom_note, zone.top_note
            ));
        }
        let encoded_bottom = match zones.get(index + 1) {
            Some(below) => {
                let below_at = index + 2;
                below.top_note.checked_add(1).ok_or_else(|| {
                    format!(
                        "zone{below_at} reaches note {}, leaving no range for {at}",
                        below.top_note
                    )
                })?
            }
            None => LOWEST_NOTE,
        };
        if zone.bottom_note != encoded_bottom {
            return Err(format!(
                "{at} starts at note {}, but its encoded range would start at \
                 {encoded_bottom}; v2 stores only top notes, so that gap or overlap \
                 cannot be reproduced",
                zone.bottom_note
            ));
        }
    }
    Ok(())
}

/// A project's frame position as an index into the file it points at.
///
/// Positions are `%f` decimals counted at 44 100 Hz whatever the file's own rate says.
/// Inferred from the editor's field counts: a zone encodes `start..stop`, not the
/// whole `begin..end` extent.
fn frame(zone: &str, label: &str, value: f64, frames: usize) -> Result<usize, String> {
    if !value.is_finite() || !(0.0..=frames as f64).contains(&value) {
        return Err(format!(
            "{zone}'s {label} is at frame {value}, outside the {frames} frames its audio holds"
        ));
    }
    Ok(value.round() as usize)
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
    deep_body(&body)
}

fn deep_body(body: &nord_format::Sample) -> Result<String, String> {
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
        let actual: Vec<_> = stream.records.iter().filter(|r| r.mark).collect();
        let mark_is_terminator = names(directory.mark, stream.terminator);
        let mark_agrees = match actual.as_slice() {
            [] => mark_is_terminator,
            [record] => !mark_is_terminator && names(directory.mark, record.at),
            _ => false,
        };
        if !mark_agrees {
            return Err(format!(
                "stroke {index}: marked record disagrees with the directory"
            ));
        }
        // A loop opens a fresh packet, so the words it covers form whole packets. The
        // wide generations pack to their own size and are not checked against this one.
        if let (codec::Layout::V2, Some(record)) = (layout, actual.first()) {
            let words = terminator - record.at;
            if !words.is_multiple_of(nord_format::formats::nsmp::stroke::PACKET_LEN / 3) {
                return Err(format!(
                    "stroke {index}: the loop covers {words} words, which is not whole packets"
                ));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(root_key: u8, bottom_note: u8, top_note: u8) -> Zone {
        Zone {
            zone_id: 0,
            root_key,
            enabled: true,
            bottom_note,
            top_note,
            strokes: Vec::new(),
        }
    }

    #[test]
    fn a_project_key_map_must_be_representable_by_top_notes() {
        let valid = [zone(72, 61, 84), zone(48, LOWEST_NOTE, 60)];
        assert!(validate_key_ranges(&valid).is_ok());

        let gap = [zone(72, 62, 84), zone(48, LOWEST_NOTE, 60)];
        assert!(validate_key_ranges(&gap).is_err());

        let misplaced_root = [zone(60, 61, 84), zone(48, LOWEST_NOTE, 60)];
        assert!(validate_key_ranges(&misplaced_root).is_err());

        let raised_floor = [zone(60, LOWEST_NOTE + 1, 84)];
        assert!(validate_key_ranges(&raised_floor).is_err());
    }

    #[test]
    fn loop_points_read_as_frames_around_a_colon() {
        let points = loop_points("16384:32768", 1024).unwrap();
        assert_eq!(points, encode::Loop::new(16_384, 32_768).crossfade(1_024));
        assert_eq!(loop_points(" 8 : 9 ", 0).unwrap(), encode::Loop::new(8, 9));
        for bad in ["16384", "16384:", "a:b", "16384:32768:1", "-1:5"] {
            assert!(loop_points(bad, 0).is_err(), "{bad}");
        }
    }

    /// The project's loop points count in the audio file's frames, so a trimmed zone
    /// moves them; whichever loop is switched on maps onto the one the container holds.
    #[test]
    fn a_projects_loop_maps_onto_the_one_the_container_holds() {
        let long = stroke_with(|s| s.loop_enabled = true);
        let (points, dropped) = zone_loop("zone1", &long, 1_000, 88_200).unwrap();
        assert_eq!(points, Some(encode::Loop::new(15_384, 31_768)));
        assert!(dropped.is_empty());

        let short = stroke_with(|s| {
            s.loop_enabled = true;
            s.short_loop_enabled = true;
            s.loop_length_short = 1_024.0;
            s.loop_crossfade_short = 0;
            s.loop_crossfade_mode = 1;
        });
        let (points, _) = zone_loop("zone1", &short, 0, 88_200).unwrap();
        assert_eq!(points, Some(encode::Loop::new(16_384, 17_408)));

        let off = stroke_with(|_| {});
        assert_eq!(zone_loop("zone1", &off, 0, 88_200).unwrap().0, None);
    }

    /// A loop setting the instrument cannot carry is named: refused when it would
    /// change the audio, reported when the editor drops it too.
    #[test]
    fn loop_settings_with_nowhere_to_go_are_named() {
        let refused = |edit: fn(&mut Stroke)| {
            let mut s = stroke_with(|s| s.loop_enabled = true);
            edit(&mut s);
            zone_loop("zone1", &s, 0, 88_200).unwrap_err()
        };
        assert!(refused(|s| s.loop_crossfade_mode = 1).contains("m_loopXFModeLong"));
        assert!(refused(|s| {
            s.short_loop_enabled = true;
            s.loop_length_short = 1_024.0;
            s.loop_crossfade_short = 10;
        })
        .contains("m_loopXFadeShort"));
        assert!(refused(|s| s.loop_length = 0.0).contains("m_loopLengthLong"));
        assert!(refused(|s| s.loop_length = 90_000.0).contains("loop end"));

        // A trim that starts after the loop does leaves the loop nowhere to begin.
        let trimmed = stroke_with(|s| s.loop_enabled = true);
        assert!(zone_loop("zone1", &trimmed, 20_000, 88_200).is_err());

        let mut noisy = stroke_with(|s| s.loop_enabled = true);
        noisy.loop_detune = -50;
        noisy.loop_decay_enabled = true;
        let (points, dropped) = zone_loop("zone1", &noisy, 0, 88_200).unwrap();
        assert!(points.is_some());
        assert_eq!(dropped.len(), 2, "{dropped:?}");
        assert!(dropped[0].contains("m_loopDetune"));
        assert!(dropped[1].contains("m_loopDecay"));
    }

    /// The canonical LP rung, whose loop the editor stores at 16384..32768.
    fn stroke_with(edit: impl FnOnce(&mut Stroke)) -> Stroke {
        let mut stroke = Stroke {
            zone_id: 129,
            global_id: 1,
            file_id: 1,
            begin: 0.0,
            end: 88_200.0,
            start: 0.0,
            stop: 88_200.0,
            loop_enabled: false,
            short_loop_enabled: false,
            loop_start: 16_384.0,
            loop_length: 16_384.0,
            loop_length_short: 0.0,
            loop_crossfade: 0.0,
            loop_crossfade_mode: 0,
            loop_crossfade_short: 10,
            short_loop_uses_pitch: true,
            loop_detune: 0,
            loop_decay_enabled: false,
            loop_decay: 20.0,
        };
        edit(&mut stroke);
        stroke
    }

    #[test]
    fn a_frame_position_is_checked_before_rounding() {
        assert_eq!(frame("zone1", "start", 0.4, 10).unwrap(), 0);
        for value in [-0.4, 10.4, f64::NAN, f64::INFINITY] {
            assert!(frame("zone1", "start", value, 10).is_err(), "{value}");
        }
    }

    #[test]
    fn a_directory_cannot_claim_an_unmarked_record_as_a_loop() {
        let mut file = encode::instrument(
            &vec![0; encode::MIN_FRAMES],
            &encode::Options::new("Unmarked"),
        )
        .unwrap();
        let stroke = nord_format::formats::nsmp::section::find_mut(
            &mut file.body.sections,
            nord_format::formats::nsmp::section::STK,
        )
        .unwrap();
        let first = stroke.payload[20..22].to_vec();
        stroke.payload[38..40].copy_from_slice(&first);

        let sample = nord_format::Sample::V2(file);
        assert!(deep_body(&sample)
            .unwrap_err()
            .contains("marked record disagrees"));
    }
}
