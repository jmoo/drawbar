//! What the sample-instrument checks share: typed access to a decoded specimen,
//! edits through the generation-neutral accessors, and the codec's derived values.

use crate::Context;
use nord_format::cbin::{Cbin, Generation};
use nord_format::formats::nsmp;
use nord_format::formats::nsmpproj::{self, build};
use nord_format::{Entity, Sample};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

pub fn parse(bytes: &[u8]) -> Result<Entity, String> {
    nord_format::from_stream(&mut Cursor::new(bytes)).context("parse")
}

/// The file at `name`, relative to the directory of `beside`, read and parsed.
pub fn related(beside: &Path, name: &str) -> Result<(Vec<u8>, Entity), String> {
    let path = beside.parent().unwrap().join(name);
    let bytes = std::fs::read(&path).context(name)?;
    let entity = parse(&bytes).context(name)?;
    Ok((bytes, entity))
}

pub fn sample(entity: &Entity) -> Result<&Sample, String> {
    match entity {
        Entity::Sample(sample) => Ok(sample),
        other => Err(format!(
            "{} is not a sample instrument",
            other.identity().kind
        )),
    }
}

pub fn narrow(entity: &Entity) -> Result<&Cbin<nsmp::Sample>, String> {
    match sample(entity)? {
        Sample::V2(sample) => Ok(sample),
        Sample::V3(_) => Err("a wide instrument, not a narrow one".into()),
    }
}

pub fn wide(entity: &Entity) -> Result<&Cbin<nsmp::SampleV3>, String> {
    match sample(entity)? {
        Sample::V3(sample) => Ok(sample),
        Sample::V2(_) => Err("a narrow instrument, not a wide one".into()),
    }
}

pub fn project(entity: &Entity) -> Result<&nsmpproj::Project, String> {
    match entity {
        Entity::SampleProject(project) => Ok(project),
        other => Err(format!(
            "{} is not a Sample Editor project",
            other.identity().kind
        )),
    }
}

/// The four counts a walked stream states about its own shape: the fields it
/// covers, the 1:1 fields it opens with, the field its second 1:1 run starts at,
/// and how many fields that run covers.
pub fn landmarks(stream: &nsmp::codec::Stream) -> Result<[usize; 4], String> {
    let warmup = stream
        .records
        .iter()
        .take_while(|record| record.one_to_one)
        .map(|record| record.values.len())
        .sum::<usize>();
    let resync_at = stream
        .records
        .iter()
        .skip_while(|record| record.one_to_one)
        .find(|record| record.one_to_one)
        .ok_or("the stream does not return to 1:1 records after its lattice content")?
        .first_field;
    let resync = stream
        .records
        .iter()
        .filter(|record| record.one_to_one && record.first_field >= resync_at)
        .map(|record| record.values.len())
        .sum::<usize>();
    Ok([stream.fields, warmup, resync_at, resync])
}

pub fn planned(plan: &nsmp::encode::Plan) -> [usize; 4] {
    [plan.fields, plan.warmup, plan.resync_at, plan.resync]
}

/// Every stroke's decoded fields, in file order.
pub fn audio(sample: &Sample) -> Result<Vec<Vec<i16>>, String> {
    let layout = sample.layout().context("layout")?;
    sample
        .stroke_streams()
        .into_iter()
        .enumerate()
        .map(|(index, (at, stream))| {
            nsmp::codec::decode(stream, at, layout)
                .map(|audio| audio.samples)
                .context(format!("stroke {index}"))
        })
        .collect()
}

/// `bytes` re-encoded after `edit`. The edit may not resize the file, and the result
/// must read back, which proves the container checksum was recomputed.
pub fn edited(
    bytes: &[u8],
    edit: impl FnOnce(&mut Sample) -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    let mut entity = parse(bytes)?;
    let Entity::Sample(sample) = &mut entity else {
        return Err("not a sample instrument".into());
    };
    edit(sample)?;
    let after = nord_format::to_bytes(&entity).context("re-encode")?;
    ensure!(
        after.len() == bytes.len(),
        "the edit resized the file from {} to {} bytes",
        bytes.len(),
        after.len()
    );
    parse(&after).context("reading the edit back")?;
    Ok(after)
}

/// The offsets an edit changed, less the container checksum, which follows any
/// change to the body: a V1 header's CRC-32, or the CRC-16 after a V0 body.
pub fn moved(before: &[u8], after: &[u8]) -> Vec<usize> {
    let checksum = match nord_format::cbin::inspect(&mut Cursor::new(before)) {
        Ok(info) if info.header.generation == Generation::V0 => before.len() - 2..before.len(),
        _ => 0x18..0x1c,
    };
    (0..before.len())
        .filter(|&i| before[i] != after[i] && !checksum.contains(&i))
        .collect()
}

/// Converts decibels to a linear gain with [`nsmp::zone::GAIN_BITS`] fractional bits,
/// computed at higher precision than the field and rounded once, as the writer does.
/// Silence (`-inf`) and a negative gain (NaN) both convert to zero.
pub fn gain_units(decibels: f32) -> u64 {
    (10f64.powf(f64::from(decibels) / 20.0) * f64::from(nsmp::zone::GAIN_UNITY)).round() as u64
}

/// A stroke's statistic A is the reciprocal of the file peak, scaled by the stroke's
/// gain and stored mod `2^24`.
///
/// Inferred from specimens; not confirmed on hardware.
pub fn statistic_a(stroke: &[u8], peak: u64, gain: u64) -> Result<(), String> {
    let peak = peak.max(1);
    let bits = 64 - peak.leading_zeros();
    let exact_power = u32::from(peak.is_power_of_two());
    let reciprocal = (1u64 << (21 + bits + (1 - exact_power))) / peak;
    let mantissa = (reciprocal * gain) >> (nsmp::zone::GAIN_BITS + 3);
    let want = ((mantissa % (1 << 24)) as u32).to_be_bytes();
    ensure!(
        stroke[9..12] == want[1..],
        "stroke {} statistic A is {:02x?}, and peak {peak} at gain {gain} gives {:02x?}",
        stroke[3],
        &stroke[9..12],
        &want[1..]
    );
    Ok(())
}

/// The file peak over every stroke, as statistic A measures it.
pub fn peak(streams: &[(usize, &[u8])], layout: nsmp::codec::Layout) -> u64 {
    streams
        .iter()
        .filter_map(|(_, s)| nsmp::codec::peak(s, layout))
        .map(|p| u64::from(p.unsigned_abs()))
        .max()
        .unwrap_or(0)
}

/// The gain stroke `id` was built with, read from a wide render's decibel field.
pub fn wide_stroke_gain(wide: &Cbin<nsmp::SampleV3>, id: u8) -> Option<u64> {
    let layout = nsmp::codec::Layout::from_version(wide.header.version)?;
    let (_, stroke) = wide
        .stroke_streams()
        .into_iter()
        .find(|(_, s)| s[3] == id)?;
    Some(gain_units(nsmp::codec::zone_gain_db(stroke, layout)?))
}

/// A project's audio files as generated WAVs, each long enough for every stroke that
/// plays it.
///
/// The editor's WAVs are not corpus material, so the audio is generated. Only the frame
/// count affects what is compared: a stroke's field count comes from its length, and
/// every other field compared is metadata.
pub struct Synthetic(BTreeMap<u32, usize>);

impl Synthetic {
    pub fn of(project: &nsmpproj::Project) -> Result<Synthetic, String> {
        let mut frames = BTreeMap::new();
        for stroke in project.strokes().context("project strokes")? {
            let needed = stroke.end.max(stroke.stop).ceil() as usize;
            let longest = frames.entry(stroke.file_id).or_insert(0);
            *longest = needed.max(*longest);
        }
        Ok(Synthetic(frames))
    }
}

impl build::Source for Synthetic {
    fn wav(&self, file: &nsmpproj::AudioFile) -> Result<Cow<'_, [u8]>, build::Unavailable> {
        let frames = *self.0.get(&file.id).ok_or(build::Unavailable::Missing)?;
        let audio: Vec<i16> = (0..frames).map(|k| (k % 512) as i16 * 16 - 4096).collect();
        nord_format::wav::mono_pcm16(&audio, nsmp::codec::SOURCE_RATE)
            .map(Cow::Owned)
            .map_err(|e| build::Unavailable::Io(std::io::Error::other(e)))
    }
}

/// A project's narrow instrument, as the library plans and encodes it from
/// [`Synthetic`] audio with the editor's predictor choice.
pub fn built_v2(project: &nsmpproj::Project) -> Result<(build::Plan, Cbin<nsmp::Sample>), String> {
    let plan =
        build::plan(project, nsmp::codec::Layout::V2, &Synthetic::of(project)?).context("plan")?;
    let built = plan
        .encode(&plan.name, nsmp::encode::Predictor::Minimizing, None)
        .context("build")?;
    match built {
        Sample::V2(file) => Ok((plan, file)),
        Sample::V3(_) => Err("the narrow layout built a wide chain".into()),
    }
}

/// A v4 `map`'s per-key table, stored and as the planner would write it from the
/// zones.
pub struct KeyMapPlan {
    pub kind: nsmp::zone::KeyMap,
    pub stored: Vec<u8>,
    pub planned: Vec<u8>,
    #[cfg(feature = "corpus")]
    pub writes: bool,
}

/// The per-key table's plan, or `None` where the `map` holds no table.
pub fn planned_key_map(
    sample: &nord_format::cbin::Cbin<nsmp::SampleV3>,
) -> Result<Option<KeyMapPlan>, String> {
    let map =
        nsmp::section::find(&sample.body.sections, nsmp::section::MAP4).ok_or("no map section")?;
    let table = sample.zone_table().context("zone table")?;
    let kind = table.key_map(&map.payload).context("per-key table")?;
    if kind == nsmp::zone::KeyMap::Absent {
        return Ok(None);
    }
    let zones = sample.zones().context("zones")?;
    let plan = table
        .plan_key_map(&map.payload, &zones)
        .context("the key-map planner")?;
    let mut planned = map.payload.clone();
    for (at, quad) in &plan {
        planned[*at..*at + quad.len()].copy_from_slice(quad);
    }
    Ok(Some(KeyMapPlan {
        kind,
        stored: map.payload.clone(),
        planned,
        #[cfg(feature = "corpus")]
        writes: !plan.is_empty(),
    }))
}
