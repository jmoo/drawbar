//! Regions and loops: `cue ` points, the `LIST`/`adtl` chunk that names and spans them,
//! and the loops of a `smpl` chunk. Saving replaces those three chunks whole and copies
//! every other byte of the file as it was.

use std::collections::BTreeMap;

use super::{chunks, layout, only, u32_at, Chunk};
use crate::error::{malformed, out_of_bounds, Error};

/// A named span of frames: a cue point, its `labl` text and its `ltxt` length. A cue
/// point with no length is a region of no frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub start: u32,
    pub frames: u32,
    pub label: String,
}

/// The `smpl` loop type: 0, 1 and 2 are defined, 3 to 31 reserved, and 32 up the
/// sampler maker's own, kept as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Play {
    Forward,
    Alternating,
    Backward,
    Other(u32),
}

/// A loop over frames `start` through `end`, both played. `fraction` (of a frame past
/// `end`) and `plays` (0 = for ever) are the `smpl` loop's own, kept as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loop {
    pub start: u32,
    pub end: u32,
    pub play: Play,
    pub fraction: u32,
    pub plays: u32,
}

impl Loop {
    /// A forward loop over frames `start` through `end`, played for ever.
    pub fn forward(start: u32, end: u32) -> Loop {
        Loop {
            start,
            end,
            play: Play::Forward,
            fraction: 0,
            plays: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub regions: Vec<Region>,
    pub loops: Vec<Loop>,
}

impl Region {
    fn holds(&self, found: &Loop) -> bool {
        found.start >= self.start
            && u64::from(found.end) < u64::from(self.start) + u64::from(self.frames)
    }

    fn touches(&self, found: &Loop) -> bool {
        u64::from(found.start) < u64::from(self.start) + u64::from(self.frames)
            && found.end >= self.start
    }
}

impl Marks {
    /// Each region with the loop it holds, its frames counted from the region's start.
    /// Every loop lies wholly inside exactly one region, crosses the end of none, and no
    /// region holds two.
    pub fn regions_with_loops(&self) -> Result<Vec<(&Region, Option<Loop>)>, Error> {
        let mut out: Vec<(&Region, Option<Loop>)> =
            self.regions.iter().map(|region| (region, None)).collect();
        for found in &self.loops {
            let span = format!("the loop over frames {} to {}", found.start, found.end);
            if let Some(region) = self
                .regions
                .iter()
                .find(|r| r.touches(found) && !r.holds(found))
            {
                return malformed(format!(
                    "{span} crosses an end of region {:?}",
                    region.label
                ));
            }
            let mut holders = out.iter_mut().filter(|(region, _)| region.holds(found));
            let (Some((region, held)), None) = (holders.next(), holders.next()) else {
                return malformed(format!("{span} lies in no region, or in more than one"));
            };
            if held.is_some() {
                return malformed(format!("region {:?} holds two loops", region.label));
            }
            *held = Some(Loop {
                start: found.start - region.start,
                end: found.end - region.start,
                ..*found
            });
        }
        Ok(out)
    }
}

const CUE_POINT: usize = 24;
/// Bytes of a `smpl` chunk ahead of its loops; the loop count is the u32 at 28 and the
/// sampler-specific data length at 32.
const SAMPLER_HEAD: usize = 36;
const SAMPLER_LOOP: usize = 24;
/// The `ltxt` fields ahead of its text: cue id, length, purpose, country, language,
/// dialect and code page.
const LTXT_HEAD: usize = 20;

fn is_adtl(bytes: &[u8], chunk: &Chunk) -> bool {
    &chunk.id == b"LIST" && bytes.get(chunk.body.start..chunk.body.start + 4) == Some(b"adtl")
}

/// Cue `id`'s label: `bytes` up to its first NUL, refused unless UTF-8, so a save never
/// rewrites it as something else.
fn label(id: u32, bytes: &[u8]) -> Result<String, Error> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    match std::str::from_utf8(&bytes[..end]) {
        Ok(text) => Ok(text.to_string()),
        Err(_) => malformed(format!("cue {id}'s label is not UTF-8")),
    }
}

/// Cue ids and their sample offsets, in the cue chunk's order.
fn cue_points(cue: &[u8]) -> Result<Vec<(u32, u32)>, Error> {
    let count = cue.get(..4).map(|_| u32_at(cue, 0) as usize);
    if count.and_then(|n| n.checked_mul(CUE_POINT)?.checked_add(4)) != Some(cue.len()) {
        return malformed(format!(
            "cue chunk is {} bytes, not 4 plus {CUE_POINT} per point it declares",
            cue.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    cue[4..]
        .as_chunks::<CUE_POINT>()
        .0
        .iter()
        .map(|point| {
            let id = u32_at(point, 0);
            if &point[8..12] != b"data" {
                return malformed(format!(
                    "cue point {id} sits in a {} chunk; only points in the data chunk are read",
                    String::from_utf8_lossy(&point[8..12])
                ));
            }
            if !seen.insert(id) {
                return malformed(format!("cue id {id} is declared twice"));
            }
            Ok((id, u32_at(point, 20)))
        })
        .collect()
}

/// The `labl` text and `ltxt` length the adtl list gives each cue id.
#[derive(Default)]
struct Adtl {
    labels: BTreeMap<u32, String>,
    lengths: BTreeMap<u32, u32>,
}

fn read_adtl(list: &[u8]) -> Result<Adtl, Error> {
    let mut adtl = Adtl::default();
    let mut at = 4;
    while list.len() - at >= 8 {
        let id = &list[at..at + 4];
        let size = u32_at(list, at + 4) as usize;
        let Some(body) = list.get(at + 8..).and_then(|rest| rest.get(..size)) else {
            return malformed(format!(
                "adtl entry {} claims {size} bytes but the list ends first",
                String::from_utf8_lossy(id)
            ));
        };
        if body.len() < 4 && matches!(id, b"labl" | b"ltxt") {
            return malformed(format!(
                "adtl entry {} is too short to name its cue",
                String::from_utf8_lossy(id)
            ));
        }
        let twice = match id {
            b"labl" => adtl
                .labels
                .insert(u32_at(body, 0), label(u32_at(body, 0), &body[4..])?)
                .is_some(),
            b"ltxt" if body.len() < LTXT_HEAD => {
                return malformed(format!(
                    "an ltxt entry is {size} bytes; it needs {LTXT_HEAD}"
                ))
            }
            b"ltxt" => adtl
                .lengths
                .insert(u32_at(body, 0), u32_at(body, 4))
                .is_some(),
            _ => false,
        };
        if twice {
            return malformed(format!(
                "cue {} has two {} entries",
                u32_at(body, 0),
                String::from_utf8_lossy(id)
            ));
        }
        at += 8 + size + size % 2;
        if at > list.len() {
            return malformed("an odd-sized adtl entry has no pad byte");
        }
    }
    if at < list.len() {
        return malformed(format!(
            "{} trailing byte(s) of the adtl list do not form an entry",
            list.len() - at
        ));
    }
    Ok(adtl)
}

pub(super) fn read_loops(smpl: &[u8], frames: usize) -> Result<Vec<Loop>, Error> {
    if smpl.len() < SAMPLER_HEAD {
        return malformed(format!(
            "smpl chunk is {} bytes; its header is {SAMPLER_HEAD}",
            smpl.len()
        ));
    }
    let count = u32_at(smpl, 28) as usize;
    let needed = count
        .checked_mul(SAMPLER_LOOP)
        .and_then(|n| n.checked_add(SAMPLER_HEAD));
    if needed.is_none_or(|n| n > smpl.len()) {
        return malformed(format!(
            "smpl chunk declares {count} loop(s) but is {} bytes, too short for them",
            smpl.len()
        ));
    }
    smpl[SAMPLER_HEAD..]
        .as_chunks::<SAMPLER_LOOP>()
        .0
        .iter()
        .take(count)
        .map(|entry| {
            let play = match u32_at(entry, 4) {
                0 => Play::Forward,
                1 => Play::Alternating,
                2 => Play::Backward,
                kind => Play::Other(kind),
            };
            let found = Loop {
                start: u32_at(entry, 8),
                end: u32_at(entry, 12),
                play,
                fraction: u32_at(entry, 16),
                plays: u32_at(entry, 20),
            };
            check_loop(&found, frames)?;
            Ok(found)
        })
        .collect()
}

fn check_loop(found: &Loop, frames: usize) -> Result<(), Error> {
    if found.start > found.end || found.end as usize >= frames {
        return out_of_bounds(
            format!("a loop over frames {} to {}", found.start, found.end),
            format!("the {frames} frame(s) of audio"),
        );
    }
    Ok(())
}

fn check_region(region: &Region, frames: usize) -> Result<(), Error> {
    if u64::from(region.start) + u64::from(region.frames) > frames as u64 {
        return out_of_bounds(
            format!(
                "region {:?} over {} frame(s) from {}",
                region.label, region.frames, region.start
            ),
            format!("the {frames} frame(s) of audio"),
        );
    }
    Ok(())
}

/// The regions and loops a WAV declares.
pub fn read_marks(bytes: &[u8]) -> Result<Marks, Error> {
    let frames = layout(bytes)?.frames();
    let chunks = chunks(bytes)?;
    let mut adtl_lists = chunks.iter().filter(|c| is_adtl(bytes, c));
    let adtl = match (adtl_lists.next(), adtl_lists.next()) {
        (_, Some(_)) => return malformed("more than one adtl list"),
        (Some(list), None) => read_adtl(&bytes[list.body.clone()])?,
        (None, None) => Adtl::default(),
    };
    let points = match only(&chunks, b"cue ")? {
        Some(cue) => cue_points(&bytes[cue.body.clone()])?,
        None => Vec::new(),
    };
    if let Some(id) = adtl
        .labels
        .keys()
        .chain(adtl.lengths.keys())
        .find(|id| !points.iter().any(|(point, _)| point == *id))
    {
        return malformed(format!(
            "the adtl list names cue {id}, which no cue point has"
        ));
    }
    let regions = points
        .iter()
        .map(|(id, start)| {
            let region = Region {
                start: *start,
                frames: adtl.lengths.get(id).copied().unwrap_or(0),
                label: adtl.labels.get(id).cloned().unwrap_or_default(),
            };
            check_region(&region, frames)?;
            Ok(region)
        })
        .collect::<Result<_, Error>>()?;
    let loops = match only(&chunks, b"smpl")? {
        Some(smpl) => read_loops(&bytes[smpl.body.clone()], frames)?,
        None => Vec::new(),
    };
    Ok(Marks { regions, loops })
}

pub(super) fn push_chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

fn cue_chunk(regions: &[Region]) -> Vec<u8> {
    let mut body = (regions.len() as u32).to_le_bytes().to_vec();
    for (id, region) in (1u32..).zip(regions) {
        for word in [id, region.start] {
            body.extend_from_slice(&word.to_le_bytes());
        }
        body.extend_from_slice(b"data");
        for word in [0, 0, region.start] {
            body.extend_from_slice(&u32::to_le_bytes(word));
        }
    }
    body
}

fn adtl_list(regions: &[Region]) -> Vec<u8> {
    let mut body = b"adtl".to_vec();
    for (id, region) in (1u32..).zip(regions) {
        let mut labl = id.to_le_bytes().to_vec();
        labl.extend_from_slice(region.label.as_bytes());
        labl.push(0);
        push_chunk(&mut body, b"labl", &labl);
        if region.frames > 0 {
            let mut ltxt = id.to_le_bytes().to_vec();
            ltxt.extend_from_slice(&region.frames.to_le_bytes());
            ltxt.extend_from_slice(b"rgn ");
            ltxt.extend_from_slice(&[0; 8]); // country, language, dialect, code page
            push_chunk(&mut body, b"ltxt", &ltxt);
        }
    }
    body
}

/// Each loop's id: the cue id of the first region holding it that no earlier loop took,
/// else one past every cue id, so ids stay unique.
fn loop_ids(regions: &[Region], loops: &[Loop]) -> Vec<u32> {
    let mut taken = std::collections::BTreeSet::new();
    let mut spare = regions.len() as u32 + 1..;
    loops
        .iter()
        .map(|found| {
            let held = (1u32..)
                .zip(regions)
                .find(|(id, region)| region.holds(found) && !taken.contains(id));
            let id = held.map_or_else(|| spare.next().unwrap(), |(id, _)| id);
            taken.insert(id);
            id
        })
        .collect()
}

/// `old` is the body of the `smpl` chunk being replaced, if there was one, so its unity
/// note, tuning, timing and sampler data survive.
fn smpl_chunk(old: Option<&[u8]>, rate: u32, regions: &[Region], loops: &[Loop]) -> Vec<u8> {
    let (mut body, data, tail) = match old {
        Some(old) => {
            let after = SAMPLER_HEAD + u32_at(old, 28) as usize * SAMPLER_LOOP;
            (old[..28].to_vec(), u32_at(old, 32), &old[after..])
        }
        None => {
            let period = (1e9 / f64::from(rate)).round() as u32;
            let mut fresh = vec![0; 28];
            fresh[8..12].copy_from_slice(&period.to_le_bytes());
            fresh[12..16].copy_from_slice(&60u32.to_le_bytes()); // unity note: middle C
            (fresh, 0, &[][..])
        }
    };
    body.extend_from_slice(&(loops.len() as u32).to_le_bytes());
    body.extend_from_slice(&data.to_le_bytes());
    for (id, found) in loop_ids(regions, loops).into_iter().zip(loops) {
        let play = match found.play {
            Play::Forward => 0,
            Play::Alternating => 1,
            Play::Backward => 2,
            Play::Other(kind) => kind,
        };
        for word in [
            id,
            play,
            found.start,
            found.end,
            found.fraction,
            found.plays,
        ] {
            body.extend_from_slice(&word.to_le_bytes());
        }
    }
    body.extend_from_slice(tail);
    body
}

/// `bytes` with its `cue `, `adtl` and `smpl` chunks replaced by ones declaring `marks`,
/// written after every other chunk, which is copied as it was. No regions writes no
/// `cue ` or `adtl`; a `smpl` chunk is kept, with `marks.loops` and the old one's header
/// and sampler data, if the file had one or `marks` has loops. Each loop is tied to the
/// region holding it by the region's cue id.
pub fn with_marks(bytes: &[u8], marks: &Marks) -> Result<Vec<u8>, Error> {
    let layout = layout(bytes)?;
    let frames = layout.frames();
    for region in &marks.regions {
        check_region(region, frames)?;
        if region.label.contains('\0') {
            return out_of_bounds(format!("label {:?}", region.label), "text with no NUL");
        }
    }
    for found in &marks.loops {
        check_loop(found, frames)?;
    }
    let chunks = chunks(bytes)?;
    let old_smpl = only(&chunks, b"smpl")?;
    if let Some(smpl) = old_smpl {
        read_loops(&bytes[smpl.body.clone()], frames)?;
    }

    let mut out = bytes[..12].to_vec();
    for chunk in &chunks {
        if !(&chunk.id == b"cue " || &chunk.id == b"smpl" || is_adtl(bytes, chunk)) {
            out.extend_from_slice(&bytes[chunk.whole()]);
        }
    }
    if !marks.regions.is_empty() {
        push_chunk(&mut out, b"cue ", &cue_chunk(&marks.regions));
        push_chunk(&mut out, b"LIST", &adtl_list(&marks.regions));
    }
    if old_smpl.is_some() || !marks.loops.is_empty() {
        let old = old_smpl.map(|smpl| &bytes[smpl.body.clone()]);
        push_chunk(
            &mut out,
            b"smpl",
            &smpl_chunk(old, layout.format.rate, &marks.regions, &marks.loops),
        );
    }
    let Ok(riff) = u32::try_from(out.len() - 8) else {
        return out_of_bounds(
            format!("{} bytes", out.len()),
            "a WAV whose RIFF length fits u32",
        );
    };
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::{read, write, Audio, Encoding};

    fn take(frames: usize) -> Vec<u8> {
        let samples = (0..frames * 2)
            .map(|i| (i as f32 / 1000.0).sin() / 2.0)
            .collect();
        let audio = Audio {
            rate: 44_100,
            channels: 2,
            samples,
        };
        write(&audio, Encoding::Int { bits: 24 }).unwrap()
    }

    /// `wav` with `id` holding `body` added after its last chunk.
    fn append(wav: &[u8], id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = wav.to_vec();
        push_chunk(&mut out, id, body);
        let riff = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&riff.to_le_bytes());
        out
    }

    fn marks() -> Marks {
        Marks {
            regions: vec![
                Region {
                    start: 100,
                    frames: 400,
                    label: "060-b0-l00".into(),
                },
                Region {
                    start: 520,
                    frames: 81,
                    label: "060-b2-l00".into(),
                },
                Region {
                    start: 700,
                    frames: 0,
                    label: "odd".into(),
                },
            ],
            loops: vec![Loop::forward(300, 499)],
        }
    }

    fn other_chunks(bytes: &[u8]) -> Vec<Vec<u8>> {
        chunks(bytes)
            .unwrap()
            .into_iter()
            .filter(|c| !(&c.id == b"cue " || &c.id == b"smpl" || is_adtl(bytes, c)))
            .map(|c| bytes[c.whole()].to_vec())
            .collect()
    }

    #[test]
    fn marks_saved_read_back_and_leave_every_other_byte_alone() {
        let info = b"INFOISFT\x05\x00\x00\x00take\x00\x00";
        let wav = append(&append(&take(1000), b"LIST", info), b"junk", b"abc");
        let saved = with_marks(&wav, &marks()).unwrap();
        assert_eq!(read_marks(&saved).unwrap(), marks());
        assert_eq!(other_chunks(&saved), other_chunks(&wav));
        assert_eq!(read(&saved).unwrap(), read(&wav).unwrap());
    }

    #[test]
    fn saving_again_replaces_the_marks_and_keeps_the_sampler_header() {
        let saved = with_marks(&take(1000), &marks()).unwrap();
        let mut tuned = saved.clone();
        let smpl = only(&chunks(&saved).unwrap(), b"smpl")
            .unwrap()
            .unwrap()
            .body
            .start;
        tuned[smpl + 12..smpl + 16].copy_from_slice(&64u32.to_le_bytes()); // unity note E4

        let fewer = Marks {
            regions: marks().regions[..1].to_vec(),
            loops: vec![],
        };
        let again = with_marks(&tuned, &fewer).unwrap();
        assert_eq!(read_marks(&again).unwrap(), fewer);
        let chunks = chunks(&again).unwrap();
        for id in [b"cue ", b"smpl"] {
            assert_eq!(chunks.iter().filter(|c| &c.id == id).count(), 1);
        }
        let smpl = only(&chunks, b"smpl").unwrap().unwrap().body.start;
        assert_eq!(u32_at(&again, smpl + 12), 64);

        let cleared = with_marks(&take(1000), &Marks::default()).unwrap();
        assert_eq!(cleared, take(1000));
    }

    fn smpl_body(bytes: &[u8]) -> &[u8] {
        let chunks = chunks(bytes).unwrap();
        let body = only(&chunks, b"smpl").unwrap().unwrap().body.clone();
        &bytes[body]
    }

    #[test]
    fn saving_keeps_the_sampler_data_and_each_loops_fraction_and_play_count() {
        let unlooped = Marks {
            loops: vec![],
            ..marks()
        };
        let mut body = vec![0; 28];
        body[12..16].copy_from_slice(&64u32.to_le_bytes());
        // one loop, 5 bytes of sampler data; the loop is cue 1's, the region holding it
        for word in [1, 5, 1, 0, 300, 499, 1 << 31, 3] {
            body.extend_from_slice(&u32::to_le_bytes(word));
        }
        body.extend_from_slice(b"maker");
        let wav = append(&with_marks(&take(1000), &unlooped).unwrap(), b"smpl", &body);

        let read = read_marks(&wav).unwrap();
        let found = Loop {
            fraction: 1 << 31,
            plays: 3,
            ..Loop::forward(300, 499)
        };
        assert_eq!(read.loops, [found]);
        let saved = with_marks(&wav, &read).unwrap();
        assert_eq!(smpl_body(&saved), body);
    }

    #[test]
    fn each_loop_is_tied_to_the_cue_id_of_the_region_holding_it() {
        let loops = [(530, 550), (300, 399), (400, 449), (650, 660)];
        let marks = Marks {
            loops: loops.map(|(start, end)| Loop::forward(start, end)).to_vec(),
            ..marks()
        };
        let saved = with_marks(&take(1000), &marks).unwrap();
        let ids: Vec<u32> = smpl_body(&saved)[SAMPLER_HEAD..]
            .as_chunks::<SAMPLER_LOOP>()
            .0
            .iter()
            .map(|entry| u32_at(entry, 0))
            .collect();
        // the second loop in region 1, and one in no region, take ids no cue point has
        assert_eq!(ids, [2, 1, 4, 5]);
    }

    #[test]
    fn a_loop_type_of_the_sampler_makers_own_is_kept() {
        let mut theirs = marks();
        theirs.loops[0].play = Play::Other(32);
        let saved = with_marks(&take(1000), &theirs).unwrap();
        assert_eq!(read_marks(&saved).unwrap(), theirs);
    }

    #[test]
    fn each_loop_belongs_to_the_region_that_holds_it() {
        let marks = marks();
        let held = marks.regions_with_loops().unwrap();
        let loops: Vec<_> = held
            .iter()
            .map(|(region, found)| (region.label.as_str(), *found))
            .collect();
        let attack = Loop::forward(200, 399);
        assert_eq!(
            loops,
            [
                ("060-b0-l00", Some(attack)),
                ("060-b2-l00", None),
                ("odd", None)
            ]
        );
    }

    #[test]
    fn a_loop_that_no_one_region_holds_is_refused() {
        let at = |start, end| Loop::forward(start, end);
        for (loops, reason) in [
            (vec![at(450, 509)], "crosses an end"),
            (vec![at(505, 510)], "lies in no region"),
            (vec![at(300, 349), at(350, 399)], "holds two loops"),
        ] {
            let bad = Marks { loops, ..marks() };
            let err = bad.regions_with_loops().unwrap_err().to_string();
            assert!(err.contains(reason), "want {reason:?}, got {err}");
        }
    }

    #[test]
    fn a_cue_point_without_adtl_is_an_unnamed_region_of_no_frames() {
        let one = Region {
            start: 42,
            frames: 0,
            label: String::new(),
        };
        let wav = append(&take(100), b"cue ", &cue_chunk(std::slice::from_ref(&one)));
        assert_eq!(read_marks(&wav).unwrap().regions, [one]);
    }

    #[test]
    fn marks_outside_the_audio_are_refused_on_save() {
        let wav = take(1000);
        let mut long = marks();
        long.regions[0].frames = 901;
        let mut late = marks();
        late.loops[0].end = 1000;
        let mut backward = marks();
        backward.loops[0].start = 500;
        let mut nul = marks();
        nul.regions[0].label = "a\0b".into();
        for bad in [long, late, backward, nul] {
            assert!(with_marks(&wav, &bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn malformed_or_unread_marks_are_refused_by_name() {
        let wav = take(1000);
        let saved = with_marks(&wav, &marks()).unwrap();
        let chunks = chunks(&saved).unwrap();
        let cue = only(&chunks, b"cue ").unwrap().unwrap().body.start;
        let smpl = only(&chunks, b"smpl").unwrap().unwrap().body.start;
        let edit = |at: usize, bytes: &[u8]| {
            let mut w = saved.clone();
            w[at..at + bytes.len()].copy_from_slice(bytes);
            w
        };
        let cases = [
            (edit(cue + 4 + 8, b"slnt"), "slnt chunk"),
            (
                edit(cue + 4 + CUE_POINT, &1u32.to_le_bytes()),
                "cue id 1 is declared twice",
            ),
            (edit(cue, &9u32.to_le_bytes()), "per point it declares"),
            (
                edit(cue + 4 + 20, &950u32.to_le_bytes()),
                "the 1000 frame(s)",
            ),
            (
                edit(smpl + SAMPLER_HEAD + 12, &1000u32.to_le_bytes()),
                "the 1000 frame(s)",
            ),
            (edit(smpl + 28, &2u32.to_le_bytes()), "too short for them"),
        ];
        for (wav, reason) in cases {
            let err = read_marks(&wav).unwrap_err().to_string();
            assert!(err.contains(reason), "want {reason:?}, got {err}");
        }

        let orphan = append(&wav, b"LIST", &adtl_list(&marks().regions));
        let err = read_marks(&orphan).unwrap_err().to_string();
        assert!(err.contains("names cue 1, which no cue point has"), "{err}");
    }

    #[test]
    fn a_label_that_is_not_utf8_is_refused_not_rewritten() {
        let mut list = b"adtl".to_vec();
        push_chunk(&mut list, b"labl", b"\x01\x00\x00\x00C\xe94\x00");
        let one = Region {
            start: 42,
            frames: 0,
            label: String::new(),
        };
        let cue = append(&take(100), b"cue ", &cue_chunk(std::slice::from_ref(&one)));
        let err = read_marks(&append(&cue, b"LIST", &list))
            .unwrap_err()
            .to_string();
        assert!(err.contains("cue 1's label is not UTF-8"), "{err}");
    }

    #[test]
    fn an_odd_sized_adtl_entry_ending_the_list_without_its_pad_byte_is_refused() {
        let mut list = b"adtl".to_vec();
        list.extend_from_slice(b"labl");
        list.extend_from_slice(&5u32.to_le_bytes());
        list.extend_from_slice(&1u32.to_le_bytes());
        list.push(b'a');
        let wav = append(&take(100), b"LIST", &list);
        let err = read_marks(&wav).unwrap_err().to_string();
        assert!(err.contains("no pad byte"), "{err}");
    }
}
