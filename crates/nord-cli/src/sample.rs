//! `nord sample`: the verbs that apply only to a sample instrument.
//!
//! `edit` runs [`crate::edit::run`] with the class fixed. Only what the format crate
//! can patch in place is settable: always the name, plus each zone's root key and
//! boundaries when its keyboard map can be read and recomputed.
//!
//! `decode` turns the audio back into WAV, and `verify --deep` walks the encoded
//! stream as well as the container. Both take a slot wherever they take a file.
//! Reading a slot changes nothing, so neither asks for confirmation.
//!
//! `encode` builds a one-zone instrument from a WAV. `build` builds a whole
//! instrument from a Sample Editor project, which supplies the zones, root keys, top
//! notes and trim points.
//!
//! `project new` writes the Sample Editor's `.nsmpproj` save file from a set of
//! WAVs, one zone per file.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::Args;
use nord_format::formats::nsmp::{self, codec, encode};
use nord_format::formats::nsmpproj::{self, build, AudioFile, NewZone, Project, PROJECT_RATE};
use nord_format::note;
use nord_format::Entity;
use nord_usb::ObjectClass;

use crate::edit::write_file;
use crate::slot::Target;
use crate::ui::Ui;

#[derive(Args)]
pub struct EditArgs {
    /// A `.nsmp` file or a slot on the instrument (`1:14`). Editing a slot reads it,
    /// changes it and writes it back over USB, so it needs `--yes` or a confirmation.
    #[arg(value_name = "FILE|BANK:SLOT")]
    pub target: String,

    #[command(flatten)]
    pub common: crate::edit::SetArgs,
}

#[derive(Args)]
pub struct DecodeArgs {
    /// The sample instruments to decode: `.nsmp`, `.nsmp3` or `.nsmp4` files, or
    /// slots on the instrument (`3:14`), which are read and never written.
    #[arg(required = true, value_name = "FILE|BANK:SLOT")]
    pub targets: Vec<String>,

    /// Write one WAV per zone into this directory. Without it, nothing is written
    /// and the run only reports what decodes.
    #[arg(short, long, value_name = "DIR")]
    pub out: Option<PathBuf>,
}

#[derive(Args)]
pub struct EncodeArgs {
    /// A 44.1 kHz mono or stereo 16-bit PCM WAV.
    #[arg(value_name = "WAV")]
    pub wav: PathBuf,

    /// Where to write the instrument. Defaults to the WAV's name with the selected
    /// generation's extension.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Instrument name. Defaults to the WAV's file name without its extension. The
    /// name field has a fixed width, wider in v3 and v4, and a longer name is refused.
    #[arg(long)]
    pub name: Option<String>,

    /// The note that plays the sample at its recorded pitch: a name (`C4`, `F#3`) or
    /// 0-127.
    #[arg(long, value_name = "NOTE", default_value = "C4")]
    pub root_key: String,

    /// The highest note the zone covers. Defaults to two octaves above the root.
    #[arg(long, value_name = "NOTE")]
    pub top_note: Option<String>,

    /// Loop over `START:END`, in frames of the WAV. The audio after END is not
    /// stored, but its first few frames shape the loop's last ones: END at the end of
    /// the WAV reads the loop's start there instead. The loop's crossfade is applied
    /// to the samples themselves.
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

    #[command(flatten)]
    pub coding: CodingArgs,
}

/// How `encode` and `build` code the audio, and which generation they write.
#[derive(Args)]
pub struct CodingArgs {
    /// Store every content field directly, in order-zero records, instead of the
    /// editor's record coding. The file is larger and differs from the editor's, but
    /// decodes to the same audio.
    #[arg(long)]
    pub plain: bool,

    /// Which generation to write: 2 (`.nsmp`), 3 (`.nsmp3`) or 4 (`.nsmp4`). The
    /// audio is the same in all three. The container and the stream's units differ.
    /// Only v2 has been played on hardware, so 3 and 4 need `--unverified`.
    #[arg(long, value_name = "N", default_value_t = 2, value_parser = clap::value_parser!(u8).range(2..=4))]
    pub generation: u8,

    /// Quantize every stroke at this shift instead of the computed one. Experimental.
    #[arg(long, hide = true, value_name = "BITS", value_parser = clap::value_parser!(u8).range(0..=15))]
    pub shift: Option<u8>,

    /// Acknowledge that no v3 or v4 encode has been played on an instrument.
    /// Required with `--generation 3` and `--generation 4`.
    #[arg(long)]
    pub unverified: bool,
}

impl CodingArgs {
    /// The generation `--generation` names, once `--unverified` admits it.
    ///
    /// v2 has no gate: mono, stereo and looped v2 encodes play on an Electro 5. Confirmed
    /// on hardware.
    fn layout(&self) -> Result<codec::Layout, String> {
        let layout = match self.generation {
            2 => codec::Layout::V2,
            3 => codec::Layout::V3,
            4 => codec::Layout::V4,
            n => return Err(format!("--generation {n}: the format has 2, 3 and 4")),
        };
        if layout == codec::Layout::V2 || self.unverified {
            return Ok(layout);
        }
        Err(format!(
            "no v{} encode has been played on an instrument, so the file is known only to \
             match what Nord Sample Editor renders. Pass --unverified to write it anyway.",
            self.generation
        ))
    }

    fn predictor(&self) -> encode::Predictor {
        if self.plain {
            encode::Predictor::Plain
        } else {
            encode::Predictor::Minimizing
        }
    }
}

#[derive(Args)]
pub struct BuildArgs {
    /// A Nord Sample Editor project (`.nsmpproj`). Relative audio paths in it
    /// resolve from the project's directory, with `/` or `\` between folders.
    #[arg(value_name = "PROJECT")]
    pub project: PathBuf,

    /// Where to write the instrument. Defaults to the project's path with the selected
    /// generation's extension.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Instrument name. Defaults to the name in the project. The name field has a
    /// fixed width, wider in v3 and v4, and a longer name is refused.
    #[arg(long)]
    pub name: Option<String>,

    #[command(flatten)]
    pub coding: CodingArgs,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// The sample instruments to check: `.nsmp`, `.nsmp3` or `.nsmp4` files, or
    /// slots on the instrument (`3:14`), which are read and never written.
    #[arg(required = true, value_name = "FILE|BANK:SLOT")]
    pub targets: Vec<String>,

    /// Also walk each stroke's encoded stream, and check the walk against the
    /// stroke header's own word directory.
    #[arg(long)]
    pub deep: bool,
}

#[derive(Args)]
pub struct ProjectNewArgs {
    /// `WAV=NOTE`, repeatable: one zone per file, at the key it was recorded at.
    /// Notes are names (`C4`, `F#3`) or numbers (0-127).
    #[arg(long = "zone", required = true, value_name = "WAV=NOTE")]
    pub zones: Vec<String>,

    /// The instrument's name inside the project. Defaults to the output file's name
    /// without its extension.
    #[arg(long)]
    pub name: Option<String>,

    /// Where to write the project. Defaults to `<name>.nsmpproj`.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

/// How a run's zones came out: decoded, or refused with a reason.
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

/// The sample body behind some bytes, or why this command cannot reach one.
fn body(bytes: &[u8]) -> Result<nord_format::Sample, String> {
    let entity =
        nord_format::from_stream(&mut std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    match entity {
        Entity::Sample(sample) => Ok(sample),
        other => Err(format!(
            "a {} file, not a sample instrument",
            other.identity().format
        )),
    }
}

/// The bytes of a target. A slot is read over USB and never written back, so this
/// is all `decode` and `verify` do to an instrument.
fn read(origin: &Target) -> Result<Vec<u8>, String> {
    match origin {
        Target::File(path) => std::fs::read(path).map_err(|e| e.to_string()),
        Target::Slot(at) => crate::device::fetch(*at, ObjectClass::Sample),
    }
}

/// A decoded name reduced to a file name: ASCII letters, digits and `_` survive,
/// and each run of anything else becomes one `-`.
fn sanitized(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// What one target's WAVs are named after: a file's stem, or for a slot, the name
/// stored in the body.
fn stem(origin: &Target, body: &nord_format::Sample) -> String {
    match origin {
        Target::File(path) => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sample".into()),
        Target::Slot(at) => body
            .name()
            .ok()
            .map(|name| sanitized(&name))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("{}-{}", at.user_bank(), at.user_slot())),
    }
}

/// Where a decode puts its WAVs, and the ones it has already put there.
///
/// ⚠️ Two targets can share a stem: the same instrument in two directories, or a file
/// and a slot with the same name. Without this check, the second target's audio would
/// replace the first's.
struct Wavs<'a> {
    dir: &'a Path,
    written: BTreeSet<PathBuf>,
}

impl Wavs<'_> {
    /// The path one zone's WAV takes, or a refusal where this run already wrote it.
    fn claim(&mut self, name: &str) -> Result<PathBuf, String> {
        let path = self.dir.join(name);
        if !self.written.insert(path.clone()) {
            return Err(format!(
                "{}: an earlier target already wrote this file; decode targets that share \
                 a name into separate directories",
                path.display()
            ));
        }
        Ok(path)
    }
}

/// `nord sample decode`: the encoded audio back to WAV, and a report of what did not
/// decode.
pub fn decode(ui: &Ui, args: DecodeArgs) -> Result<(), String> {
    if let Some(dir) = &args.out {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut wavs = args.out.as_deref().map(|dir| Wavs {
        dir,
        written: BTreeSet::new(),
    });
    let mut coverage = Coverage::default();
    let decoded = crate::file::report_each(
        ui,
        &args.targets,
        "target(s) did not decode",
        String::clone,
        |spec| {
            // A file that will not open is not a codec gap, so it is reported but not
            // counted in the coverage line.
            decode_target(ui, spec, wavs.as_mut(), &mut coverage)?;
            coverage.files += 1;
            Ok(())
        },
    );
    ui.note(coverage.line());
    decoded
}

fn decode_target(
    ui: &Ui,
    spec: &str,
    mut out: Option<&mut Wavs<'_>>,
    coverage: &mut Coverage,
) -> Result<(), String> {
    let origin = crate::slot::target(spec)?;
    let bytes = read(&origin).map_err(|e| format!("{spec}: {e}"))?;
    let body = body(&bytes).map_err(|e| format!("{spec}: {e}"))?;
    let stem = stem(&origin, &body);
    let layout = body.layout().map_err(|e| e.to_string())?;

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
                if let Some(wavs) = out.as_mut() {
                    let file = wavs.claim(&format!("{stem}-zone{n}.wav"))?;
                    let wav =
                        nord_format::wav::pcm16(&audio.samples, codec::FIELD_RATE, audio.channels)
                            .map_err(|e| format!("{}: {e}", file.display()))?;
                    crate::edit::replace_file(&file, &wav)?;
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

/// One WAV as this encoder needs it; see [`build::source_pcm`].
fn pcm_source(path: &Path) -> Result<nord_format::wav::Pcm16, String> {
    let named = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| named(&e))?;
    build::source_pcm(&bytes).map_err(|e| named(&e))
}

/// What one encoded stroke came out as, for the report.
fn stroke_line(stream: &[u8], at: usize, layout: codec::Layout) -> Result<String, String> {
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
fn loop_points(text: &str, crossfade: f64) -> Result<encode::Loop, String> {
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

/// `nord sample encode`: a WAV into a one-zone instrument of the selected generation.
pub fn encode(ui: &Ui, args: EncodeArgs) -> Result<(), String> {
    let layout = args.coding.layout()?;
    let source = pcm_source(&args.wav)?;

    let stem = args
        .wav
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sample".into());
    let name = args.name.unwrap_or_else(|| stem.clone());
    let mut options = encode::Options::new(&name)
        .root_key(note::parse(&args.root_key)?)
        .channels(source.channels)
        .predictor(args.coding.predictor())
        .layout(layout);
    if let Some(top) = &args.top_note {
        options = options.top_note(note::parse(top)?);
    }
    if let Some(points) = &args.loop_points {
        options = options.loops(loop_points(points, args.loop_crossfade as f64)?);
    }
    if let Some(bits) = args.coding.shift {
        options = options.shift(bits);
    }

    let instrument = encode::instrument(&source.samples, &options).map_err(|e| e.to_string())?;
    let out = instrument.to_bytes().map_err(|e| e.to_string())?;

    let (at, stroke) = instrument.stroke_streams()[0];
    ui.out(format!(
        "{} frames -> {}",
        source.frames(),
        stroke_line(stroke, at, layout)?
    ));

    let path = args.out.unwrap_or_else(|| {
        args.wav
            .with_file_name(format!("{stem}.{}", layout.extension()))
    });
    write_file(ui, &path, &out)
}

/// A project's WAVs, read from disk relative to the project's directory.
struct Directory<'a>(&'a Path);

impl build::Source for Directory<'_> {
    fn wav(&self, file: &AudioFile) -> Result<Cow<'_, [u8]>, build::Unavailable> {
        Ok(Cow::Owned(std::fs::read(self.path(file))?))
    }

    fn name(&self, file: &AudioFile) -> String {
        self.path(file).display().to_string()
    }
}

impl Directory<'_> {
    fn path(&self, file: &AudioFile) -> PathBuf {
        build::AudioPath::parse(&file.path).on_disk(self.0)
    }
}

/// `nord sample build`: a Sample Editor project into the instrument it describes.
pub fn build(ui: &Ui, args: BuildArgs) -> Result<(), String> {
    let layout = args.coding.layout()?;

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
    let plan = build::plan(&project, layout, &Directory(dir)).map_err(|e| e.to_string())?;
    let name = args.name.as_deref().unwrap_or(&plan.name);
    let instrument = plan
        .encode(name, args.coding.predictor(), args.coding.shift)
        .map_err(|e| e.to_string())?;
    let out = instrument.to_bytes().map_err(|e| e.to_string())?;

    ui.out(format!("{} — {} zone(s)", ui.bold(name), plan.zones.len()));
    let warnings = plan.warnings();
    let warn = |zone: Option<usize>| {
        for warning in warnings.iter().filter(|w| w.zone() == zone) {
            match warning {
                build::Warning::MapGainClamped { .. }
                | build::Warning::SecondaryStartRepaired { .. } => ui.note(ui.dim(warning)),
                build::Warning::GainWraps { .. } | build::Warning::Dropped { .. } => {
                    ui.warn(warning)
                }
            }
        }
    };
    warn(None);
    let placed = instrument.zones().map_err(|e| e.to_string())?;
    for (index, zone) in plan.zones.iter().enumerate() {
        let stream = placed
            .get(index)
            .ok_or_else(|| format!("zone{} did not reach the file", index + 1))?;
        ui.out(format!(
            "  zone{:<2} root {:<4} top {:<4} {}",
            index + 1,
            note::name(zone.root_key),
            note::name(zone.top_note),
            stroke_line(stream.stream, stream.at, layout)?,
        ));
        let gain = match zone.gain == 1.0 {
            true => String::new(),
            false => format!(" gain {:.3}", zone.gain),
        };
        ui.out(ui.dim(format!(
            "         stroke {}{gain} from {}",
            zone.global_id, zone.source
        )));
        warn(Some(index + 1));
    }

    let path = args
        .out
        .unwrap_or_else(|| args.project.with_extension(layout.extension()));
    write_file(ui, &path, &out)
}

/// `nord sample verify`: the container round trip, and with `--deep` the stream.
pub fn verify(ui: &Ui, args: VerifyArgs) -> Result<(), String> {
    crate::file::check_each(ui, &args.targets, "target(s) did not check out", |spec| {
        verify_target(spec, args.deep)
    })
}

/// One target's verdict line, `Ok` when it checked out and `Err` when it did not.
fn verify_target(spec: &str, walk: bool) -> Result<String, String> {
    let origin = crate::slot::target(spec).map_err(|e| format!("error  {e}"))?;
    let original = read(&origin).map_err(|e| format!("error  {spec} ({e})"))?;
    let round_trip = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .and_then(|entity| nord_format::to_bytes(&entity))
        .map_err(|e| format!("error  {spec} ({e})"))?;
    if round_trip != original {
        return Err(format!(
            "DIFFER {spec} (re-encode is not byte-identical; first difference at {})",
            crate::file::first_difference(&round_trip, &original),
        ));
    }
    if !walk {
        return Ok(format!("ok     {spec} ({} bytes)", original.len()));
    }
    match deep(&original) {
        Ok(note) => Ok(format!("ok     {spec} ({note})")),
        Err(e) => Err(format!("STREAM {spec} ({e})")),
    }
}

/// Walks every stroke and verifies all four directory landmarks.
fn deep(bytes: &[u8]) -> Result<String, String> {
    let body = body(bytes)?;
    deep_body(&body)
}

fn deep_body(body: &nord_format::Sample) -> Result<String, String> {
    let layout = body.layout().map_err(|e| e.to_string())?;
    let chain = body.chain().map_err(|e| e.to_string())?;
    let streams = body.stroke_streams();
    let mut records = 0usize;
    let mut looped = 0usize;
    for (index, (at, stroke)) in streams.iter().enumerate() {
        let stream =
            codec::walk(stroke, *at, layout).map_err(|e| format!("stroke {index}: {e}"))?;
        records += stream.records.len();
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
        // ⚠️ A directory pointer is a u16 word count, so on a stroke longer than WRAP
        // words it matches every record a multiple of WRAP apart. Only the walk tells
        // which one it means, and a pointer at the terminator can still alias a record.
        let ends_at_mark = names(directory.mark, stream.terminator);
        let named_by_mark: Vec<_> = stream
            .records
            .iter()
            .filter(|r| names(directory.mark, r.at))
            .collect();
        let flagged: Vec<_> = stream.records.iter().filter(|r| r.mark).collect();
        if flagged.len() > 1 {
            return Err(format!(
                "stroke {index}: {} records carry the mark bit",
                flagged.len()
            ));
        }
        if let [record] = flagged.as_slice() {
            if ends_at_mark || !names(directory.mark, record.at) {
                return Err(format!(
                    "stroke {index}: the marked record is not the one the directory names"
                ));
            }
        }
        if ends_at_mark {
            continue;
        }
        let Some(mark) = named_by_mark.first() else {
            return Err(format!(
                "stroke {index}: the mark names neither a record nor the terminator"
            ));
        };
        if chain.flags_the_marked_record() && flagged.is_empty() {
            return Err(format!(
                "stroke {index}: the loop mark names a record that does not carry the mark bit"
            ));
        }
        looped += 1;
        // A loop opens a new packet, so the words it covers form whole packets. v3 and
        // v4 use their own packet size and are not checked here.
        if layout == codec::Layout::V2 {
            let packet = nsmp::stroke::packet_len(layout) / layout.word();
            if !named_by_mark
                .iter()
                .any(|r| (terminator - r.at).is_multiple_of(packet))
            {
                return Err(format!(
                    "stroke {index}: the loop covers {} words, which is not whole packets",
                    terminator - mark.at
                ));
            }
        }
    }
    let mut note = format!(
        "{}, {} stroke(s), {records} record(s), directory agrees",
        body.generation(),
        streams.len()
    );
    if looped > 0 {
        note.push_str(&format!(", {looped} looped"));
    }
    Ok(note)
}

/// One `--zone WAV=NOTE`, before the file behind it has been read.
#[derive(Debug)]
struct ZoneSpec {
    wav: PathBuf,
    root_key: u8,
}

fn zone_spec(spec: &str) -> Result<ZoneSpec, String> {
    // From the right: a note never holds `=`, but a path may.
    let (wav, note) = spec
        .rsplit_once('=')
        .ok_or_else(|| format!("expected WAV=NOTE, got {spec:?}"))?;
    if wav.is_empty() {
        return Err(format!("{spec:?} names no WAV"));
    }
    Ok(ZoneSpec {
        wav: PathBuf::from(wav),
        root_key: note::parse(note)?,
    })
}

/// A WAV's frame count as the project states it, or an error when it cannot be stated.
fn project_frames(frames: usize, rate: u32) -> Result<u64, String> {
    if rate == 0 {
        return Err("the WAV declares 0 Hz".into());
    }
    u64::try_from(frames)
        .ok()
        .and_then(|frames| nsmpproj::project_frames(frames, rate))
        .ok_or_else(|| format!("{frames} frames at {rate} Hz overflows a frame count"))
}

/// The path a project records for a WAV: relative to the project's directory when
/// the file is inside it, as the editor stores it, and otherwise as given.
fn stored_path(wav: &Path, project: &Path) -> String {
    let dir = project.parent().unwrap_or(Path::new(""));
    if dir.as_os_str().is_empty() {
        return wav.display().to_string();
    }
    wav.strip_prefix(dir).unwrap_or(wav).display().to_string()
}

fn zone(spec: &ZoneSpec, wav: &[u8], project: &Path) -> Result<NewZone, String> {
    let named = |e: String| format!("{}: {e}", spec.wav.display());
    let audio = nord_format::wav::read_pcm16(wav).map_err(|e| named(e.to_string()))?;
    Ok(NewZone {
        path: stored_path(&spec.wav, project),
        sample_rate: audio.rate,
        frames: project_frames(audio.frames(), audio.rate).map_err(named)?,
        root_key: spec.root_key,
    })
}

/// The instrument's name and the file to write, each derived from the other when
/// only one is given.
fn destination(name: Option<String>, out: Option<PathBuf>) -> Result<(String, PathBuf), String> {
    match (name, out) {
        (Some(name), Some(out)) => Ok((name, out)),
        (Some(name), None) => {
            let out = PathBuf::from(format!("{name}.{}", nsmpproj::FORMAT));
            Ok((name, out))
        }
        (None, Some(out)) => {
            let name = out
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    format!(
                        "{}: no file stem to name the instrument after; pass --name",
                        out.display()
                    )
                })?;
            Ok((name, out))
        }
        (None, None) => {
            Err("pass --name or -o: each defaults from the other, so one of them is needed".into())
        }
    }
}

/// `nord sample project new`: a Sample Editor project from one WAV per zone.
pub fn project_new(ui: &Ui, args: ProjectNewArgs) -> Result<(), String> {
    let specs: Vec<ZoneSpec> = args
        .zones
        .iter()
        .map(|spec| zone_spec(spec))
        .collect::<Result<_, _>>()?;
    let (name, out) = destination(args.name, args.out)?;

    let mut zones = Vec::with_capacity(specs.len());
    for spec in &specs {
        let wav = std::fs::read(&spec.wav).map_err(|e| format!("{}: {e}", spec.wav.display()))?;
        zones.push(zone(spec, &wav, &out)?);
    }

    let project =
        Project::new(&name, &zones, crate::edit::unix_seconds_now()?).map_err(|e| e.to_string())?;

    for z in &zones {
        ui.out(format!(
            "  root {:<4} {:>6} Hz {:>10} frames  {}",
            note::name(z.root_key),
            z.sample_rate,
            z.frames,
            z.path,
        ));
    }
    let rescaled = zones
        .iter()
        .filter(|z| u64::from(z.sample_rate) != PROJECT_RATE)
        .count();
    if rescaled > 0 {
        ui.note(ui.dim(format!(
            "{rescaled} zone(s) are not {PROJECT_RATE} Hz; a project counts every frame \
             position at {PROJECT_RATE} Hz, so those counts are restated"
        )));
    }

    write_file(ui, &out, project.render().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn wav(rate: u32, frames: usize) -> Vec<u8> {
        nord_format::wav::mono_pcm16(&vec![0i16; frames], rate).unwrap()
    }

    fn encoded(name: &str) -> Vec<u8> {
        let options = encode::Options::new(name).root_key(60);
        encode::instrument(&vec![0i16; 4096], &options)
            .unwrap()
            .to_bytes()
            .unwrap()
    }

    fn instrument(name: &str) -> nord_format::Sample {
        let bytes = encoded(name);
        match nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap() {
            Entity::Sample(sample) => sample,
            other => panic!("encoded a {}", other.identity().format),
        }
    }

    fn scratch() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("nord-sample-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn encode_args(wav: &Path, out: PathBuf, generation: u8, unverified: bool) -> EncodeArgs {
        EncodeArgs {
            wav: wav.to_path_buf(),
            out: Some(out),
            name: Some("Gate".into()),
            root_key: "C4".into(),
            top_note: None,
            loop_points: None,
            loop_crossfade: 0,
            coding: CodingArgs {
                plain: false,
                generation,
                shift: None,
                unverified,
            },
        }
    }

    #[test]
    fn a_v2_encode_writes_unasked_and_the_unplayed_generations_do_not() {
        let dir = scratch();
        let source = dir.join("tone.wav");
        std::fs::write(&source, wav(codec::SOURCE_RATE, 4096)).unwrap();
        let ui = Ui::new(crate::ui::ColorChoice::Never);

        let played = dir.join("gate.nsmp");
        encode(&ui, encode_args(&source, played.clone(), 2, false))
            .expect("v2 is confirmed on hardware and needs no --unverified");
        assert!(played.is_file());

        for generation in [3u8, 4] {
            let out = dir.join(format!("gate.nsmp{generation}"));
            let refused =
                encode(&ui, encode_args(&source, out.clone(), generation, false)).unwrap_err();
            assert!(refused.contains("--unverified"), "v{generation}: {refused}");
            assert!(!out.exists(), "v{generation} was written anyway");

            encode(&ui, encode_args(&source, out.clone(), generation, true)).expect("acknowledged");
            assert!(out.is_file(), "v{generation}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A frame count the project cannot state is refused, and the error says why.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn a_frame_count_a_project_cannot_state_is_refused_by_name() {
        assert_eq!(project_frames(2205, 22_050).unwrap(), 4410);
        let rateless = project_frames(1, 0).unwrap_err();
        assert!(rateless.contains("0 Hz"), "{rateless}");
        let frames = (u64::MAX / PROJECT_RATE) as usize;
        let over = project_frames(frames, u32::MAX).unwrap_err();
        assert!(over.contains("overflows"), "{over}");
    }

    #[test]
    fn a_wav_beside_the_project_is_stored_relative_to_it() {
        let project = Path::new("kit/marimba.nsmpproj");
        assert_eq!(stored_path(Path::new("kit/low.wav"), project), "low.wav");
        assert_eq!(
            stored_path(Path::new("/elsewhere/low.wav"), project),
            "/elsewhere/low.wav",
        );
        // With the project in the working directory, a relative path already is.
        assert_eq!(
            stored_path(Path::new("low.wav"), Path::new("marimba.nsmpproj")),
            "low.wav",
        );
    }

    #[test]
    fn a_built_project_reads_back_with_its_zones_roots_and_paths() {
        let out = Path::new("kit/marimba.nsmpproj");
        let specs = [
            zone_spec("kit/low.wav=C3").unwrap(),
            zone_spec("kit/high.wav=72").unwrap(),
        ];
        let zones = [
            zone(&specs[0], &wav(22_050, 2205), out).unwrap(),
            zone(&specs[1], &wav(44_100, 4410), out).unwrap(),
        ];
        assert_eq!((zones[0].path.as_str(), zones[0].frames), ("low.wav", 4410));

        let bytes = Project::new("Marimba", &zones, 0).unwrap().render();
        let read =
            match nord_format::from_stream(&mut std::io::Cursor::new(bytes.as_bytes())).unwrap() {
                Entity::SampleProject(project) => project,
                other => panic!("wrote a {}", other.identity().format),
            };
        assert_eq!(read.name().unwrap(), "Marimba");
        let roots: Vec<u8> = read.zones().unwrap().iter().map(|z| z.root_key).collect();
        assert_eq!(roots, [72, 48], "zones are stored high to low");
        let paths: Vec<String> = read
            .audio_files()
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["low.wav", "high.wav"], "ids rise with the root key");
        let rates: Vec<u32> = read
            .audio_files()
            .unwrap()
            .iter()
            .map(|f| f.sample_rate)
            .collect();
        assert_eq!(rates, [22_050, 44_100], "the file's own rate is kept");
    }

    #[test]
    fn a_malformed_zone_says_what_it_expected() {
        assert!(zone_spec("low.wav").unwrap_err().contains("WAV=NOTE"));
        assert!(zone_spec("=C4").unwrap_err().contains("names no WAV"));
        assert!(zone_spec("low.wav=H9").is_err());
        assert!(zone_spec("low.wav=128").is_err());
        assert_eq!(zone_spec("a=b.wav=C4").unwrap().wav, Path::new("a=b.wav"));
    }

    #[test]
    fn a_project_needs_at_least_one_zone() {
        let new = ["nord", "sample", "project", "new", "--name", "X"];
        assert!(crate::Cli::try_parse_from(new).is_err());
        assert!(crate::Cli::try_parse_from([&new[..], &["--zone", "a.wav=C4"]].concat()).is_ok());
    }

    #[test]
    fn a_name_and_an_output_stand_in_for_each_other_but_not_for_nothing() {
        assert_eq!(
            destination(Some("Marimba".into()), None).unwrap(),
            ("Marimba".into(), PathBuf::from("Marimba.nsmpproj")),
        );
        assert_eq!(
            destination(None, Some("kit/marimba.nsmpproj".into())).unwrap(),
            ("marimba".into(), PathBuf::from("kit/marimba.nsmpproj")),
        );
        assert!(destination(None, None).is_err());
    }

    #[test]
    fn a_decoded_slot_names_its_wavs_after_the_instrument() {
        let at = crate::slot::parse("2:7").unwrap();
        assert_eq!(
            stem(&Target::Slot(at), &instrument("Vibes 2/3")),
            "Vibes-2-3"
        );
        assert_eq!(stem(&Target::Slot(at), &instrument("")), "2-7");
        assert_eq!(
            stem(&Target::File("kit/Bass.nsmp".into()), &instrument("Vibes")),
            "Bass",
        );
    }

    /// A file the verb does not take is named by its format and steered to the command
    /// that does read it.
    #[test]
    fn a_project_under_sample_edit_is_steered_to_the_file_verb() {
        let dir = scratch();
        let path = dir.join("kit.nsmpproj");
        let project = Project::new(
            "Kit",
            &[NewZone {
                path: "kit.wav".into(),
                sample_rate: 44_100,
                frames: 44_100,
                root_key: 60,
            }],
            0,
        )
        .unwrap();
        std::fs::write(&path, project.render()).unwrap();

        let args = crate::EditArgs {
            target: Some(path.display().to_string()),
            common: crate::edit::SetArgs {
                set: vec!["name=Vibes".into()],
                dry_run: false,
                fields: false,
                out: None,
                yes: false,
            },
        };
        let err = crate::edit::run(&Ui::piped(), args, ObjectClass::Sample).unwrap_err();
        assert!(err.contains(nsmpproj::FORMAT), "{err}");
        assert!(err.contains("nord edit"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A target that will not open is a failure of the run, not a gap in the codec's
    /// coverage, so the exit status says so even where other targets decoded.
    #[test]
    fn a_decode_that_loses_one_target_of_several_fails() {
        let dir = scratch();
        let kit = dir.join("kit.nsmp");
        std::fs::write(&kit, encoded("Kit")).unwrap();
        let ui = Ui::new(crate::ui::ColorChoice::Never);
        let targets = |specs: &[&Path]| DecodeArgs {
            targets: specs.iter().map(|p| p.display().to_string()).collect(),
            out: None,
        };

        decode(&ui, targets(&[&kit])).expect("a target that decodes");
        let err = decode(&ui, targets(&[&kit, &dir.join("absent.nsmp")])).unwrap_err();
        assert!(err.contains("1 of 2"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two targets can be named the same thing in different directories, and the
    /// second one's zones must not land on top of the first one's WAVs.
    #[test]
    fn a_second_target_of_the_same_name_does_not_overwrite_the_first_targets_wavs() {
        let dir = scratch();
        let out = dir.join("wavs");
        let mut kits = Vec::new();
        for side in ["a", "b"] {
            let held = dir.join(side);
            std::fs::create_dir_all(&held).unwrap();
            let kit = held.join("kit.nsmp");
            std::fs::write(&kit, encoded("Kit")).unwrap();
            kits.push(kit.display().to_string());
        }

        let err = decode(
            &Ui::new(crate::ui::ColorChoice::Never),
            DecodeArgs {
                targets: kits,
                out: Some(out.clone()),
            },
        )
        .unwrap_err();
        assert!(err.contains("1 of 2"), "{err}");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verifying_a_target_that_is_neither_a_file_nor_a_slot_names_both_readings() {
        let line = verify_target("no-such-instrument.nsmp", false).unwrap_err();
        assert!(line.starts_with("error"), "{line}");
        assert!(line.contains("no such file"), "{line}");
        assert!(line.contains("not a slot"), "{line}");
    }

    #[test]
    fn loop_points_read_as_frames_around_a_colon() {
        let points = loop_points("16384:32768", 1024.0).unwrap();
        assert_eq!(points, encode::Loop::new(16_384, 32_768).crossfade(1_024.0));
        assert_eq!(
            loop_points(" 8 : 9 ", 0.0).unwrap(),
            encode::Loop::new(8, 9)
        );
        for bad in ["16384", "16384:", "a:b", "16384:32768:1", "-1:5"] {
            assert!(loop_points(bad, 0.0).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_directory_cannot_claim_an_unmarked_record_as_a_loop() {
        let mut sample =
            encode::instrument(&[0; encode::MIN_FRAMES], &encode::Options::new("Unmarked"))
                .unwrap();
        let nord_format::Sample::V2(file) = &mut sample else {
            panic!("the default options build the narrow chain");
        };
        let stroke = nord_format::formats::nsmp::section::find_mut(
            &mut file.body.sections,
            nord_format::formats::nsmp::section::STK,
        )
        .unwrap();
        let first = stroke.payload[FIRST_RECORD..][..POINTER].to_vec();
        stroke.payload[MARK..][..POINTER].copy_from_slice(&first);
        // The offsets below are restated, so the poke is only the intended one if the
        // directory the codec reads back now names the first record as its loop.
        let directory = codec::Directory::read(&stroke.payload).expect("a directory");
        assert_eq!(directory.mark, directory.first_record);

        assert!(deep_body(&sample)
            .unwrap_err()
            .contains("does not carry the mark bit"));
    }

    /// The stroke header's directory: four big-endian pointers, at `codec::SEEK_AT` and
    /// every `codec::SEEK_STRIDE` after it, which `nsmp` keeps to itself.
    const POINTER: usize = 2;
    const FIRST_RECORD: usize = 20;
    const MARK: usize = FIRST_RECORD + 9 * 2;
}
