//! The seekable indexes against a whole read. Each range an index gives holds the
//! bytes a whole read gives its stroke or zone, the index reads no audio to find
//! them, and a file whose strokes or zones a whole read refuses has no index.

use nord_format::formats::{npno, nsmp};
use nord_format::{Entity, Sample};
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::ops::Range;

pub fn check(bytes: &[u8], entity: &Entity) -> Result<(), String> {
    match entity {
        Entity::Piano(piano) => piano_index(bytes, piano),
        Entity::Sample(sample) => sample_index(bytes, sample),
        #[cfg(feature = "bundle")]
        Entity::Bundle(bundle) => members(bundle)?.into_iter().try_for_each(|(name, bytes)| {
            let entity = crate::samples::parse(&bytes)?;
            check(&bytes, &entity).map_err(|e| format!("{name}: {e}"))
        }),
        _ => Ok(()),
    }
}

/// A bundle's piano libraries and sample instruments, each as its own file.
#[cfg(feature = "bundle")]
fn members(bundle: &nord_format::Bundle) -> Result<Vec<(String, Vec<u8>)>, String> {
    use nord_format::Bundle;
    match bundle {
        Bundle::Electro5(bundle) => {
            let pianos = bundle
                .pianos()
                .iter()
                .enumerate()
                .map(|(i, piano)| encoded(format!("piano {i}"), |out| piano.write_to(out)));
            let samples = bundle
                .samples()
                .iter()
                .enumerate()
                .map(|(i, sample)| encoded(format!("sample {i}"), |out| sample.write_to(out)));
            pianos.chain(samples).collect()
        }
        Bundle::Members(members) => members
            .iter()
            .filter(|(_, member)| {
                [npno::FORMAT, nsmp::FORMAT]
                    .iter()
                    .any(|format| format.as_bytes() == member.header.tag)
            })
            .map(|(name, member)| encoded(name.clone(), |out| member.write_to(out)))
            .collect(),
        Bundle::Drum2Bank(_) | Bundle::Drum3KitBank(_) => Ok(Vec::new()),
    }
}

#[cfg(feature = "bundle")]
fn encoded(
    name: String,
    write: impl FnOnce(&mut Cursor<Vec<u8>>) -> Result<(), nord_format::error::Error>,
) -> Result<(String, Vec<u8>), String> {
    let mut out = Cursor::new(Vec::new());
    write(&mut out).map_err(|e| format!("{name}: re-encode: {e}"))?;
    Ok((name, out.into_inner()))
}

/// A reader over a file that records the span of every read.
struct Recording<'a> {
    inner: Cursor<&'a [u8]>,
    reads: Vec<Range<u64>>,
}

impl<'a> Recording<'a> {
    fn new(bytes: &'a [u8]) -> Recording<'a> {
        Recording {
            inner: Cursor::new(bytes),
            reads: Vec::new(),
        }
    }
}

impl Read for Recording<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let at = self.inner.position();
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.reads.push(at..at + n as u64);
        }
        Ok(n)
    }
}

impl Seek for Recording<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Both reads refuse, or both read. A refusal on one side alone is the failure.
fn both<I, W, E: std::fmt::Display, F: std::fmt::Display>(
    index: Result<I, E>,
    whole: Result<W, F>,
) -> Result<Option<(I, W)>, String> {
    match (index, whole) {
        (Ok(index), Ok(whole)) => Ok(Some((index, whole))),
        (Err(_), Err(_)) => Ok(None),
        (Ok(_), Err(e)) => Err(format!("the index reads what a whole read refuses: {e}")),
        (Err(e), Ok(_)) => Err(format!("a whole read reads what the index refuses: {e}")),
    }
}

fn at<'a>(bytes: &'a [u8], range: &Range<u64>) -> Result<&'a [u8], String> {
    usize::try_from(range.start)
        .ok()
        .zip(usize::try_from(range.end).ok())
        .and_then(|(start, end)| bytes.get(start..end))
        .ok_or_else(|| format!("{range:?} is outside the {}-byte file", bytes.len()))
}

/// Every read overlapping `range` stays within its first `allowed` bytes.
fn reads_within(reads: &[Range<u64>], range: &Range<u64>, allowed: u64) -> Result<(), String> {
    let overlaps = |read: &&Range<u64>| read.start < range.end && range.start < read.end;
    let escapes = |read: &&Range<u64>| read.start < range.start || read.end > range.start + allowed;
    match reads.iter().find(|read| overlaps(read) && escapes(read)) {
        Some(read) => Err(format!(
            "the index read {read:?}, which reaches past the first {allowed} bytes of {range:?}"
        )),
        None => Ok(()),
    }
}

fn piano_index(bytes: &[u8], piano: &npno::Piano) -> Result<(), String> {
    let mut reader = Recording::new(bytes);
    let Some((index, whole)) = both(npno::Index::read_from(&mut reader), piano.library())? else {
        return Ok(());
    };
    let ranges = index.audio_ranges();
    ensure!(
        ranges.len() == whole.strokes().len(),
        "the index holds {} strokes and a whole read {}",
        ranges.len(),
        whole.strokes().len()
    );
    let indexed = index.library().strokes();
    for (i, (range, stroke)) in ranges.iter().zip(whole.strokes()).enumerate() {
        ensure!(
            at(bytes, range)? == stroke.audio(),
            "stroke {i}: the bytes at {range:?} are not the audio a whole read gives it"
        );
        ensure!(
            indexed[i].record() == stroke.record() && indexed[i].root == stroke.root,
            "stroke {i}: the index holds another record than a whole read"
        );
        reads_within(&reader.reads, range, 0).map_err(|e| format!("stroke {i}: {e}"))?;
    }
    Ok(())
}

fn sample_index(bytes: &[u8], sample: &Sample) -> Result<(), String> {
    let mut reader = Recording::new(bytes);
    let whole = sample
        .layout()
        .and_then(|layout| Ok((layout, sample.zones()?)));
    let Some((index, (layout, zones))) = both(nsmp::Index::read_from(&mut reader), whole)? else {
        return Ok(());
    };
    ensure!(
        index.layout() == layout,
        "the index reads {:?} streams and a whole read {layout:?}",
        index.layout()
    );
    ensure!(
        index.zones().len() == zones.len(),
        "the index holds {} zones and a whole read {}",
        index.zones().len(),
        zones.len()
    );
    for (i, (span, zone)) in index.zones().iter().zip(&zones).enumerate() {
        let audio = index
            .zone(i, at(bytes, &span.stream)?)
            .map_err(|e| format!("zone {i}: {e}"))?;
        ensure!(
            audio.stream == zone.stream,
            "zone {i}: the bytes at {:?} are not the stream a whole read gives it",
            span.stream
        );
        let placed = (audio.at, audio.root_key, audio.low_note, audio.top_note);
        let whole = (zone.at, zone.root_key, zone.low_note, zone.top_note);
        ensure!(
            placed == whole,
            "zone {i}: the index places it as {placed:?} and a whole read as {whole:?} \
             (body offset, root key, low note, top note)"
        );
        let opening = nsmp::Index::STROKE_OPENING as u64;
        reads_within(&reader.reads, &span.stream, opening).map_err(|e| format!("zone {i}: {e}"))?;
    }
    outline_agrees(index.outline(), sample)?;
    for edit in sample_edits(&index) {
        patch_agrees(bytes, &index, edit).map_err(|e| format!("{edit:?}: {e}"))?;
    }
    Ok(())
}

/// Both sides give the same value, or both refuse.
fn same<T: PartialEq + std::fmt::Debug, E: std::fmt::Display>(
    what: &str,
    outline: Result<T, E>,
    whole: Result<T, E>,
) -> Result<(), String> {
    match (outline, whole) {
        (Ok(outline), Ok(whole)) => {
            ensure!(
                outline == whole,
                "{what}: the outline reads {outline:?} and a whole read {whole:?}"
            );
            Ok(())
        }
        (Err(_), Err(_)) => Ok(()),
        (outline, whole) => Err(format!(
            "{what}: the outline reads {:?} and a whole read {:?}",
            outline.map_err(|e| e.to_string()),
            whole.map_err(|e| e.to_string())
        )),
    }
}

type Answer<T> = Result<T, nord_format::error::Error>;

/// A value read without the chance of a refusal, for [`same`].
fn ok<T>(value: T) -> Answer<T> {
    Ok(value)
}

fn outline_agrees(outline: &nsmp::Outline, sample: &Sample) -> Result<(), String> {
    same("the name", outline.name(), sample.name())?;
    same("the chain", outline.chain(), sample.chain())?;
    same(
        "the name's capacity",
        ok(outline.max_name_len()),
        ok(sample.max_name_len()),
    )?;
    same(
        "the generation",
        ok(outline.generation()),
        ok(sample.generation()),
    )?;
    same(
        "whether the zones edit",
        ok(outline.zones_are_editable()),
        ok(sample.zones_are_editable()),
    )?;
    match (outline.fields(), sample) {
        (nsmp::Fields::V2(fields), Sample::V2(whole)) => {
            same("the chain", fields.chain(), whole.chain())?;
            same("the zones", fields.zones(), whole.zones())?;
            same("the keyboard map", fields.key_table(), whole.key_table())?;
            same("the preset", fields.sty(), whole.sty())?;
            same(
                "the categories",
                ok(fields.categories()),
                ok(whole.categories()),
            )
        }
        (nsmp::Fields::V3(fields), Sample::V3(whole)) => {
            same("the sub name", fields.sub_name(), whole.sub_name())?;
            same("the zones", fields.zones(), whole.zones())?;
            same("the zone table", fields.zone_table(), whole.zone_table())?;
            same("the preset", fields.sty(), whole.sty())?;
            same("the meta section", fields.meta(), whole.meta())?;
            same(
                "the stroke count",
                ok(fields.stroke_count()),
                ok(whole.stroke_count()),
            )
        }
        _ => Err("the outline is of another generation than a whole read".into()),
    }
}

/// One edit the sample document makes.
#[derive(Debug, Clone, Copy)]
enum Edit {
    Name,
    Root(u8),
    Top(u8),
    Low(u8),
    Key,
}

/// An edit of each kind, to the first zone, moving each note it sets.
fn sample_edits(index: &nsmp::Index) -> Vec<Edit> {
    let mut edits = vec![Edit::Name, Edit::Key];
    if let Some(zone) = index.zones().first() {
        edits.push(Edit::Root(zone.root_key ^ 1));
        edits.push(Edit::Top(zone.top_note.saturating_sub(1)));
        edits.push(Edit::Low(zone.low_note.map_or(0, |low| low ^ 1)));
    }
    edits
}

/// A per-key level the keyboard map is unlikely to hold already.
fn level() -> nsmp::Level {
    nsmp::Level::new(nsmp::keymap::GAIN_UNITY / 2, -128).unwrap()
}

fn edit_whole(sample: &mut Sample, edit: Edit) -> Answer<()> {
    match edit {
        Edit::Name => sample.set_name("Patched"),
        Edit::Root(note) => sample.set_root_key(0, note),
        Edit::Top(note) => sample.set_zone_top_note(0, note),
        Edit::Low(note) => sample.set_zone_low_note(0, note),
        Edit::Key => match sample {
            Sample::V2(file) => {
                let mut table = file.key_table()?;
                table.set_key(60, level())?;
                file.set_key_table(&table)
            }
            Sample::V3(_) => Err(wide()),
        },
    }
}

fn edit_outline(outline: &mut nsmp::Outline, edit: Edit) -> Answer<()> {
    match edit {
        Edit::Name => outline.set_name("Patched"),
        Edit::Root(note) => outline.set_root_key(0, note),
        Edit::Top(note) => outline.set_zone_top_note(0, note),
        Edit::Low(note) => outline.set_zone_low_note(0, note),
        Edit::Key => {
            let nsmp::Fields::V2(fields) = outline.fields() else {
                return Err(wide());
            };
            let mut table = fields.key_table()?;
            table.set_key(60, level())?;
            outline.set_key_table(&table)
        }
    }
}

fn wide() -> nord_format::error::Error {
    nord_format::error::ParseError::AssertFail(
        "the wide chain's keyboard map is not written".into(),
    )
    .into()
}

/// The edit made through the index is the file a whole read, the edit and a whole
/// write make, or both refuse it.
fn patch_agrees(bytes: &[u8], index: &nsmp::Index, edit: Edit) -> Result<(), String> {
    let mut entity = crate::samples::parse(bytes)?;
    let Entity::Sample(sample) = &mut entity else {
        return Err("not a sample instrument".into());
    };
    let whole = edit_whole(sample, edit).and_then(|()| sample.to_bytes());
    let mut outline = index.outline().clone();
    let patched = edit_outline(&mut outline, edit).and_then(|()| {
        let mut out = Vec::new();
        index
            .patch(&outline)?
            .copy(&mut Cursor::new(bytes), &mut out)?;
        Ok(out)
    });
    match (whole, patched) {
        (Ok(whole), Ok(patched)) => {
            let first = whole.iter().zip(&patched).position(|(a, b)| a != b);
            ensure!(
                whole.len() == patched.len() && first.is_none(),
                "the patch writes {} bytes and a whole edit {}, first apart at {first:?}",
                patched.len(),
                whole.len()
            );
            Ok(())
        }
        (Err(_), Err(_)) => Ok(()),
        (Ok(_), Err(e)) => Err(format!("a whole edit takes it and the patch refuses: {e}")),
        (Err(e), Ok(_)) => Err(format!("the patch takes it and a whole edit refuses: {e}")),
    }
}
