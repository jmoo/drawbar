//! `nord sample` — the verbs that only mean anything for a sample instrument.
//!
//! `edit` mirrors `nord program edit`, but the fields come from
//! [`editors::SampleEditor`]'s accessors rather than a declarative panel: a
//! sample is mostly encoded audio, and only what the format crate can patch in
//! place is settable — the name, and each zone's root key and top note.
//! `decode` turns the audio back into WAV, and `verify --deep` walks the
//! encoded stream rather than only the container. Both take a slot wherever
//! they take a file; reading a slot is a read-only transaction, so neither
//! asks for confirmation.
//!
//! `project new` writes the Sample Editor's own `.nsmpproj` save file from a
//! set of WAVs, one zone per file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::Args;
use nord_format::formats::nsmp::{codec, encode};
use nord_format::formats::nsmpproj::{self, NewZone, Project};
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
    /// The sample instruments to decode: `.nsmp`, `.nsmp3` or `.nsmp4` files, or
    /// slots on the instrument (`3:14`), which are read and never written.
    #[arg(required = true, value_name = "FILE|BANK:SLOT")]
    pub targets: Vec<String>,

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

    /// Acknowledge that this is not a vendor-identical encode. Required.
    #[arg(long)]
    pub experimental: bool,
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

    /// The instrument's name inside the project. Defaults to the output's stem.
    #[arg(long)]
    pub name: Option<String>,

    /// Where to write the project. Defaults to `<name>.nsmpproj`.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,
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

/// The bytes behind a target. A slot is read over USB and never written back,
/// so this is the whole of what `decode` and `verify` do to an instrument.
fn read(origin: &Target) -> Result<Vec<u8>, String> {
    match origin {
        Target::File(path) => std::fs::read(path).map_err(|e| e.to_string()),
        Target::Slot(at) => crate::device::fetch(*at, ObjectClass::Sample),
    }
}

/// A decoded name reduced to a filename: what a path takes literally survives,
/// and every run of anything else becomes a single `-`.
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

/// What the WAVs of one target are named after: a file's own stem, or — a slot
/// having no name outside the instrument — the name the body carries.
fn stem(origin: &Target, body: &nord_format::Sample) -> String {
    match origin {
        Target::File(path) => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sample".into()),
        Target::Slot(at) => match body {
            nord_format::Sample::V2(s) => s.name().ok(),
            nord_format::Sample::V3(s) => s.name().ok(),
        }
        .map(|name| sanitized(&name))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{}-{}", at.user_bank(), at.user_slot())),
    }
}

/// `nord sample decode`: the encoded audio back to WAV, and what did not decode.
pub fn decode(ui: &Ui, args: DecodeArgs) -> Result<(), String> {
    if let Some(dir) = &args.out {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut coverage = Coverage::default();
    let mut failed = 0usize;
    for spec in &args.targets {
        ui.out(ui.bold(spec));
        match decode_target(ui, spec, args.out.as_deref(), &mut coverage) {
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
    if failed == args.targets.len() {
        return Err("nothing decoded".into());
    }
    Ok(())
}

fn decode_target(
    ui: &Ui,
    spec: &str,
    out: Option<&Path>,
    coverage: &mut Coverage,
) -> Result<(), String> {
    let origin = crate::slot::target(spec)?;
    let bytes = read(&origin).map_err(|e| format!("{spec}: {e}"))?;
    let body = body(&bytes).map_err(|e| format!("{spec}: {e}"))?;
    let stem = stem(&origin, &body);
    let layout = body.layout();

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

/// `nord sample encode`: a WAV into a one-zone v2 instrument.
pub fn encode(ui: &Ui, args: EncodeArgs) -> Result<(), String> {
    if !args.experimental {
        return Err(
            "encoding is experimental: the file it writes is structurally sound, \
             decodes back exactly and plays on an Electro 5, but it is not \
             byte-identical to the editor's output and its stroke does not loop. \
             Pass --experimental to write it anyway."
                .into(),
        );
    }

    let bytes = std::fs::read(&args.wav).map_err(|e| format!("{}: {e}", args.wav.display()))?;
    let source =
        nord_format::wav::read_pcm16(&bytes).map_err(|e| format!("{}: {e}", args.wav.display()))?;
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
    let stream = codec::walk(stroke, at, codec::Layout::V2).map_err(|e| e.to_string())?;
    let audio = codec::decode(stroke, at, codec::Layout::V2).map_err(|e| e.to_string())?;
    ui.out(format!(
        "{} frames -> {} fields ({:.3} s), shift {}, peak {}, {} record(s)",
        source.frames(),
        stream.fields,
        audio.seconds(),
        codec::shift(stroke, codec::Layout::V2).unwrap_or_default(),
        codec::peak(stroke, codec::Layout::V2).unwrap_or_default(),
        stream.records.len(),
    ));
    ui.out(ui.dim(if audio.differenced == 0 {
        "every field is stated outright: no record differences its own".to_string()
    } else {
        format!(
            "{}% of fields came through the predictor",
            100 * audio.differenced / audio.samples.len().max(1),
        )
    }));

    let path = args
        .out
        .unwrap_or_else(|| args.wav.with_file_name(format!("{stem}.nsmp")));
    write_file(ui, &path, &out)
}

/// `nord sample verify`: the container round trip, and with `--deep` the stream.
pub fn verify(ui: &Ui, args: VerifyArgs) -> Result<(), String> {
    let mut failed = 0usize;
    for spec in &args.targets {
        match verify_target(spec, args.deep) {
            Ok(line) => ui.out(line),
            Err(line) => {
                failed += 1;
                ui.out(line);
            }
        }
    }
    if failed > 0 {
        return Err(format!(
            "{failed} of {} did not check out",
            args.targets.len()
        ));
    }
    Ok(())
}

/// One target's verdict line, `Ok` when it checked out and `Err` when it did not.
fn verify_target(spec: &str, walk: bool) -> Result<String, String> {
    let origin = crate::slot::target(spec).map_err(|e| format!("error  {e}"))?;
    let original = read(&origin).map_err(|e| format!("error  {spec} ({e})"))?;
    let round_trip = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .and_then(|entity| nord_format::to_bytes(&entity))
        .map_err(|e| format!("error  {spec} ({e})"))?;
    if round_trip != original {
        return Err(format!("DIFFER {spec} (re-encode is not byte-identical)"));
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

/// The rate every frame position in a project counts at, whatever the file's
/// own `m_sampleRate` says. See the `nsmpproj` module doc.
const PROJECT_RATE: u64 = 44_100;

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

/// A frame count restated at [`PROJECT_RATE`], to the nearest whole frame — the
/// ratio does not divide for every rate, and the field holds frames.
fn project_frames(frames: usize, rate: u32) -> Result<u64, String> {
    let rate = u64::from(rate);
    if rate == 0 {
        return Err("the WAV declares 0 Hz".into());
    }
    u64::try_from(frames)
        .ok()
        .and_then(|f| f.checked_mul(PROJECT_RATE))
        .and_then(|scaled| scaled.checked_add(rate / 2))
        .map(|rounded| rounded / rate)
        .ok_or_else(|| format!("{frames} frames at {rate} Hz overflows a frame count"))
}

/// The path a project records for a WAV: relative to the project's own
/// directory when the file lies under it, which is how the editor stores every
/// specimen; otherwise exactly what was given.
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

/// The instrument's name and the file to write, each standing in for the other
/// when only one was given.
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
        (None, None) => Err(
            "pass --name or -o: the name defaults to the output's stem, \
                             and the output to the name, so one of them has to be given"
                .into(),
        ),
    }
}

/// The Unix time [`Project::new`] stamps on every `m_modifyDate`.
fn modified() -> Result<u32, String> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock is before the Unix epoch: {e}"))?;
    u32::try_from(elapsed.as_secs()).map_err(|_| "system time does not fit m_modifyDate".into())
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

    let project = Project::new(&name, &zones, modified()?).map_err(|e| e.to_string())?;

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

    fn instrument(name: &str) -> nord_format::Sample {
        let options = encode::Options::new(name).root_key(60);
        let bytes = encode::instrument(&vec![0i16; 4096], &options)
            .unwrap()
            .to_bytes()
            .unwrap();
        match nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap() {
            Entity::Sample(sample) => sample,
            other => panic!("encoded a {}", other.identity().format),
        }
    }

    #[test]
    fn a_frame_count_is_stated_at_the_project_rate_whatever_the_wav_says() {
        assert_eq!(project_frames(4410, 44_100).unwrap(), 4410);
        assert_eq!(project_frames(2205, 22_050).unwrap(), 4410);
        assert_eq!(project_frames(9600, 96_000).unwrap(), 4410);
        assert_eq!(
            project_frames(1, 48_000).unwrap(),
            1,
            "rounded, not floored"
        );
        assert!(project_frames(1, 0).is_err());
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn a_frame_count_refuses_rounding_that_would_overflow() {
        let frames = (u64::MAX / PROJECT_RATE) as usize;
        assert!(project_frames(frames, u32::MAX).is_err());
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

    #[test]
    fn verifying_a_target_that_is_neither_a_file_nor_a_slot_names_both_readings() {
        let line = verify_target("no-such-instrument.nsmp", false).unwrap_err();
        assert!(line.starts_with("error"), "{line}");
        assert!(line.contains("no such file"), "{line}");
        assert!(line.contains("not a slot"), "{line}");
    }
}
