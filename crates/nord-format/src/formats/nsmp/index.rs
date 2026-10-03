//! Where the stroke that plays each zone sits in a sample instrument, read without
//! the audio.

use super::codec::{self, Layout};
use super::outline::Outline;
use super::section::{self, Framed, Framing, Narrow, Wide};
use super::{names_stroke, no_section, stroke, zone, Body, Chain, ZoneAudio, FORMAT};
use crate::cbin::{self, Cbin, Edit, Header, Patch, Seal};
use crate::error::{try_vec, Error, ParseError};
use std::io::{Read, Seek};
use std::ops::Range;

/// A sample instrument's zones, each with the position in the stream of the stroke that
/// plays it, and its [`Outline`]: read from the container header, every section other
/// than a stroke, and the first bytes of each stroke.
///
/// No stroke is read past its id and root key, so memory and I/O grow with the zone
/// count, not with the instrument. A stroke can then be read by its range, positionally
/// from a file or as a slice of a browser `File`, and paired with its zone by
/// [`Index::zone`], ready for [`codec::decode`].
///
/// Zones pair with strokes as [`crate::Sample::zones`] pairs them, and the section chain
/// is walked under the same rules, so an instrument whose zones one refuses the other
/// refuses. A content version past the generations [`codec`] describes is refused too,
/// since its streams could not be decoded.
///
/// ⚠️ The container checksum covers every body byte and is not verified here.
/// [`cbin::inspect`] verifies it in one streaming pass, and [`Patch::copy`] as it
/// copies.
#[derive(Debug)]
pub struct Index {
    header: Header,
    layout: Layout,
    zones: Vec<ZoneSpan>,
    /// Where the body starts in the stream the index was read from.
    body_start: u64,
    outline: Outline,
    /// Where each section's payload starts in the stream, in file order.
    payloads: Vec<u64>,
    seal: Seal,
}

/// One zone, and where the stream that plays it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneSpan {
    pub root_key: u8,
    pub top_note: u8,
    /// Lowest note, where the generation stores one. See [`ZoneAudio::low_note`].
    pub low_note: Option<u8>,
    /// Where the stream sits in the stream the index was read from.
    pub stream: Range<u64>,
}

impl Index {
    /// The bytes read of each stroke, from its start: its u32 id, a byte, then its root
    /// key.
    pub const STROKE_OPENING: usize = stroke::ROOT_KEY + 1;

    /// Read the index of the `nsmp` file that starts at the reader's position and runs
    /// to the end of the stream.
    pub fn read_from(r: &mut (impl Read + Seek)) -> Result<Index, Error> {
        let (header, body, seal) = cbin::locate_sealed(r, FORMAT)?;
        let layout =
            Layout::from_version(header.version).ok_or_else(|| ParseError::OutOfBounds {
                value: format!("content version {}", header.version),
                bound: format!(
                    "the generations this codec describes, below {}",
                    codec::V5_FROM_VERSION
                ),
            })?;
        let (zones, payloads, sample) = match layout {
            Layout::V2 => {
                let held = walk::<Narrow>(r, &body, section::STK)?;
                let zones = narrow(&body, &held)?;
                let (payloads, body) = outlined(&body, held)?;
                let sample = crate::Sample::V2(Cbin {
                    header: header.clone(),
                    body,
                });
                (zones, payloads, sample)
            }
            Layout::V3 | Layout::V4 => {
                let held = walk::<Wide>(r, &body, section::STK4)?;
                let zones = wide(&body, &held)?;
                let (payloads, body) = outlined(&body, held)?;
                let sample = crate::Sample::V3(Cbin {
                    header: header.clone(),
                    body,
                });
                (zones, payloads, sample)
            }
        };
        Ok(Index {
            header,
            layout,
            zones,
            body_start: body.start,
            outline: Outline::new(sample),
            payloads,
            seal,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The generation's stream units, which [`codec::decode`] takes.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Every zone in stored order, the order [`crate::Sample::zones`] gives. Two zones
    /// that name one stroke share its range.
    pub fn zones(&self) -> &[ZoneSpan] {
        &self.zones
    }

    /// The instrument's fields outside the audio, as the file holds them.
    pub fn outline(&self) -> &Outline {
        &self.outline
    }

    /// The patch that saves `edited`, a clone of [`Self::outline`] that its setters
    /// have edited: each section the edit changed, a stroke by its opening bytes, and
    /// the checksum, at their positions in the stream the index was read from.
    ///
    /// Every edit an outline makes is in place, so the patched file is the one a whole
    /// read, the same setters and a whole write make. An outline whose sections differ
    /// in number, tag, version or length from this index's is refused.
    pub fn patch(&self, edited: &Outline) -> Result<Patch, Error> {
        let edits = match (self.outline.sample(), edited.sample()) {
            (crate::Sample::V2(ours), crate::Sample::V2(theirs)) => self.edits(ours, theirs)?,
            (crate::Sample::V3(ours), crate::Sample::V3(theirs)) => self.edits(ours, theirs)?,
            _ => return Err(not_ours("is of another generation")),
        };
        self.seal.patch(edits)
    }

    /// Each section payload `theirs` changes.
    fn edits<'a, F: Framing>(
        &self,
        ours: &'a Cbin<Body<F>>,
        theirs: &Cbin<Body<F>>,
    ) -> Result<Vec<Edit<'a>>, Error> {
        if ours.header != theirs.header {
            return Err(not_ours("holds another container header"));
        }
        let (ours, theirs) = (&ours.body.sections, &theirs.body.sections);
        if ours.len() != theirs.len() {
            return Err(not_ours(&format!(
                "holds {} sections, not {}",
                theirs.len(),
                ours.len()
            )));
        }
        let mut edits = Vec::new();
        for (i, ((ours, theirs), &at)) in ours.iter().zip(theirs).zip(&self.payloads).enumerate() {
            let same_shape = ours.tag == theirs.tag
                && ours.version == theirs.version
                && ours.payload.len() == theirs.payload.len();
            if !same_shape {
                return Err(not_ours(&format!(
                    "holds {theirs:?} where section {i} is {ours:?}"
                )));
            }
            if ours.payload != theirs.payload {
                edits.push(Edit {
                    at,
                    old: &ours.payload,
                    new: theirs.payload.clone(),
                });
            }
        }
        Ok(edits)
    }

    /// The `index`-th zone, playing `stream`: the bytes read from its range, ready for
    /// [`codec::decode`]. A stream of any other length is refused.
    pub fn zone<'a>(&self, index: usize, stream: &'a [u8]) -> Result<ZoneAudio<'a>, Error> {
        let count = self.zones.len();
        let span = self
            .zones
            .get(index)
            .ok_or_else(|| ParseError::OutOfBounds {
                value: format!("zone {index}"),
                bound: format!("the {count} zones the map holds"),
            })?;
        let len = span.stream.end - span.stream.start;
        if u64::try_from(stream.len()) != Ok(len) {
            return Err(ParseError::AssertFail(format!(
                "zone {index}'s stream is {len} bytes, and {} were given",
                stream.len()
            ))
            .into());
        }
        let at = span
            .stream
            .start
            .checked_sub(self.body_start)
            .and_then(|at| usize::try_from(at).ok())
            .ok_or_else(|| overflow("a stroke's body offset"))?;
        Ok(ZoneAudio {
            root_key: span.root_key,
            top_note: span.top_note,
            low_note: span.low_note,
            at,
            stream,
        })
    }
}

fn not_ours(why: &str) -> Error {
    ParseError::AssertFail(format!(
        "the edited outline {why}, so it is not this index's outline"
    ))
    .into()
}

/// A section as the walk holds it: its payload whole, or a stroke's opening bytes
/// alone, and the body offsets of the payload as stored.
struct Held<F: Framing> {
    section: Framed<F>,
    at: u64,
    end: u64,
}

/// A stroke's id and root key, from the opening bytes of its payload.
struct Head {
    id: u32,
    root_key: u8,
    at: u64,
    end: u64,
}

/// Walk the section chain of the body at `body`, reading each header, every payload
/// other than a stroke's, and the opening bytes of each stroke, and seeking past the
/// rest.
fn walk<F: Framing>(
    r: &mut (impl Read + Seek),
    body: &Range<u64>,
    stroke_tag: &F::Tag,
) -> Result<Vec<Held<F>>, Error> {
    let len = body
        .end
        .checked_sub(body.start)
        .ok_or_else(|| overflow("the body's end"))?;
    if len == 0 {
        return Err(section::missing_opener(F::OPENER.as_ref()).into());
    }
    let mut held = Vec::new();
    let mut strokes = 0;
    let mut head = [0u8; section::HEADER4_LEN];
    let head = &mut head[..F::HEADER];
    let mut pos = 0;
    while pos < len {
        if len - pos < F::HEADER as u64 {
            return Err(section::truncated_header(pos).into());
        }
        cbin::read_at(r, position(body, pos)?, head)?;
        let tag = F::tag(head);
        if pos == 0 && tag != F::OPENER {
            return Err(section::wrong_opener(F::OPENER.as_ref(), tag.as_ref()).into());
        }
        let (version, payload_len) = F::fields(head, pos)?;
        let end = section::section_end(pos, F::HEADER, payload_len, len, tag.as_ref())?;
        let at = pos + F::HEADER as u64;
        let read = match tag == *stroke_tag {
            true => {
                strokes += 1;
                opening(strokes - 1, payload_len)?
            }
            false => payload_len,
        };
        let mut payload = try_vec(read)?;
        cbin::read_at(r, position(body, at)?, &mut payload)?;
        held.push(Held {
            section: Framed {
                tag,
                version,
                payload,
            },
            at,
            end,
        });
        pos = end;
    }
    Ok(held)
}

/// The bytes read of the `index`-th stroke, whose payload is `len` bytes.
fn opening(index: usize, len: usize) -> Result<usize, Error> {
    match len < Index::STROKE_OPENING {
        true => Err(ParseError::AssertFail(format!(
            "stroke {index} is {len} bytes, too short for its id and root key"
        ))
        .into()),
        false => Ok(Index::STROKE_OPENING),
    }
}

/// Every stroke the walk held, with the id and root key it opens with, in file order.
fn heads<F: Framing>(held: &[Held<F>], stroke_tag: &F::Tag) -> Vec<Head> {
    held.iter()
        .filter(|h| h.section.is(stroke_tag))
        .map(|h| {
            let opening = &h.section.payload;
            let [a, b, c, d, ..] = *opening.as_slice() else {
                unreachable!("a held stroke holds its opening")
            };
            Head {
                id: u32::from_be_bytes([a, b, c, d]),
                root_key: opening[stroke::ROOT_KEY],
                at: h.at,
                end: h.end,
            }
        })
        .collect()
}

/// The first section tagged `tag`, as `(version, payload)`.
fn first<'a, F: Framing>(held: &'a [Held<F>], tag: &F::Tag) -> Option<(F::Version, &'a [u8])> {
    held.iter()
        .find(|h| h.section.is(tag))
        .map(|h| (h.section.version, h.section.payload.as_slice()))
}

/// The held sections as a body, and where each payload starts in the stream.
fn outlined<F: Framing>(
    body: &Range<u64>,
    held: Vec<Held<F>>,
) -> Result<(Vec<u64>, Body<F>), Error> {
    let payloads = held
        .iter()
        .map(|h| position(body, h.at))
        .collect::<Result<_, _>>()?;
    let sections = held.into_iter().map(|h| h.section).collect();
    Ok((payloads, Body { sections }))
}

/// Each zone of a narrow chain, played by the first stroke whose id it names.
fn narrow(body: &Range<u64>, held: &[Held<Narrow>]) -> Result<Vec<ZoneSpan>, Error> {
    let (version, map) = first(held, section::MAP).ok_or_else(|| no_section(section::MAP))?;
    let zones = zone::read(Chain::from_map_version(version)?, map)?;
    let strokes = heads(held, section::STK);
    zones
        .iter()
        .map(|zone| {
            let head = strokes
                .iter()
                .find(|head| names_stroke(head.id, zone.stroke_id))
                .ok_or_else(|| {
                    ParseError::AssertFail(format!(
                        "zone reaching up to note {} names stroke {}, which the file does not \
                         contain",
                        zone.top_note, zone.stroke_id
                    ))
                })?;
            span(body, head, head.root_key, zone.top_note, None)
        })
        .collect()
}

/// Each zone of a wide chain, played by the first stroke carrying the id it names.
fn wide(body: &Range<u64>, held: &[Held<Wide>]) -> Result<Vec<ZoneSpan>, Error> {
    let (version, map) = first(held, section::MAP4).ok_or_else(|| no_section(section::MAP4))?;
    let strokes = heads(held, section::STK4);
    let ids: Vec<(u32, u8)> = strokes
        .iter()
        .map(|head| (head.id, head.root_key))
        .collect();
    let zones = zone::Table::locate(version, map, &ids)?.read(map, &ids)?;
    zones
        .iter()
        .enumerate()
        .map(|(index, zone)| {
            let head = strokes
                .iter()
                .find(|head| head.id == zone.stroke_gid)
                .ok_or_else(|| {
                    ParseError::AssertFail(format!(
                        "zone {index} names stroke {}, which the file does not contain",
                        zone.stroke_gid
                    ))
                })?;
            span(body, head, zone.root_key, zone.top_note, zone.low_note)
        })
        .collect()
}

fn span(
    body: &Range<u64>,
    head: &Head,
    root_key: u8,
    top_note: u8,
    low_note: Option<u8>,
) -> Result<ZoneSpan, Error> {
    Ok(ZoneSpan {
        root_key,
        top_note,
        low_note,
        stream: position(body, head.at)?..position(body, head.end)?,
    })
}

/// The stream position of body offset `at`, which must lie within the body.
fn position(body: &Range<u64>, at: u64) -> Result<u64, Error> {
    cbin::body_position(body, at).ok_or_else(|| overflow("a body offset"))
}

fn overflow(what: &str) -> Error {
    ParseError::OutOfBounds {
        value: what.to_string(),
        bound: "an offset within the body".into(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::super::encode::{self, Instrument, NewZone, Preset, DEFAULT_LOOP_DECAY};
    use super::super::Fields;
    use super::*;
    use crate::crc::crc32;
    use crate::Sample;
    use std::io::Cursor;

    fn index(bytes: &[u8]) -> Result<Index, Error> {
        Index::read_from(&mut Cursor::new(bytes))
    }

    /// Three zones whose stroke ids are out of file order.
    fn built(layout: Layout) -> Vec<u8> {
        let source: Vec<i16> = (0..4 * encode::MIN_FRAMES)
            .map(|k| ((k % 64) as i16 - 32) * 256)
            .collect();
        let zone = |global_id, root_key, top_note, frames| NewZone {
            source: &source[..frames],
            channels: 1,
            root_key,
            top_note,
            global_id,
            loops: None,
            secondary_start: encode::default_secondary_start(frames, None),
            shift: None,
            gain: 1.0,
            loop_decay: DEFAULT_LOOP_DECAY,
        };
        let instrument = Instrument {
            name: "Index",
            map_gain: 1.0,
            predictor: encode::Predictor::Minimizing,
            layout,
            preset: Preset::default(),
        };
        let zones = [
            zone(3, 84, 127, 4 * encode::MIN_FRAMES),
            zone(1, 60, 71, 2 * encode::MIN_FRAMES),
            zone(2, 48, 59, 3 * encode::MIN_FRAMES),
        ];
        encode::multi_zone(instrument, &zones)
            .unwrap()
            .to_bytes()
            .unwrap()
    }

    fn sample(bytes: &[u8]) -> Result<Sample, Error> {
        match crate::from_stream(&mut Cursor::new(bytes))? {
            crate::Entity::Sample(sample) => Ok(sample),
            other => panic!("{other:?} is not a sample instrument"),
        }
    }

    /// A whole read's zones, or why it refused them.
    fn whole_zones(bytes: &[u8]) -> Result<(), Error> {
        let sample = sample(bytes)?;
        sample.layout()?;
        sample.zones().map(|_| ())
    }

    /// The bytes at each zone's range are the stream a whole read gives that zone, with
    /// the same notes and body offset.
    fn agrees_with_a_whole_read(bytes: &[u8]) {
        let sample = sample(bytes).unwrap();
        let whole = sample.zones().unwrap();
        let index = index(bytes).unwrap();
        assert_eq!(index.layout(), sample.layout().unwrap());
        assert_eq!(index.zones().len(), whole.len());
        for (i, (span, whole)) in index.zones().iter().zip(&whole).enumerate() {
            let stream = &bytes[span.stream.start as usize..span.stream.end as usize];
            let audio = index.zone(i, stream).unwrap();
            assert_eq!(audio.stream, whole.stream, "zone {i}'s stream");
            assert_eq!(audio.at, whole.at, "zone {i}'s body offset");
            assert_eq!(audio.root_key, whole.root_key, "zone {i}'s root key");
            assert_eq!(audio.top_note, whole.top_note, "zone {i}'s top note");
            assert_eq!(audio.low_note, whole.low_note, "zone {i}'s low note");
        }
    }

    #[test]
    fn each_range_holds_the_stream_a_whole_read_gives_its_zone() {
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            agrees_with_a_whole_read(&built(layout));
        }
    }

    #[test]
    fn a_stream_read_by_its_range_decodes_as_the_whole_reads_does() {
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            let bytes = built(layout);
            let whole = sample(&bytes).unwrap();
            let index = index(&bytes).unwrap();
            for (i, (span, zone)) in index.zones().iter().zip(whole.zones().unwrap()).enumerate() {
                let stream = &bytes[span.stream.start as usize..span.stream.end as usize];
                let audio = index.zone(i, stream).unwrap();
                assert_eq!(
                    codec::decode(audio.stream, audio.at, index.layout()).unwrap(),
                    codec::decode(zone.stream, zone.at, layout).unwrap(),
                    "{layout:?}, the zone up to {}",
                    zone.top_note
                );
            }
        }
    }

    #[test]
    fn a_stream_of_another_length_is_refused() {
        let bytes = built(Layout::V3);
        let index = index(&bytes).unwrap();
        let span = &index.zones()[0];
        let stream = &bytes[span.stream.start as usize..span.stream.end as usize];
        let error = index.zone(0, &stream[1..]).err().unwrap().to_string();
        let given = format!(
            "is {} bytes, and {} were given",
            stream.len(),
            stream.len() - 1
        );
        assert!(error.contains(&given), "{error}");
    }

    #[test]
    fn a_zone_past_the_last_is_refused() {
        let index = index(&built(Layout::V3)).unwrap();
        let count = index.zones().len();
        let error = index.zone(count, &[]).err().unwrap().to_string();
        assert!(
            error.contains(&format!("the {count} zones the map holds")),
            "{error}"
        );
    }

    /// `bytes` with its body changed by `edit` and the type-1 checksum recomputed, so a
    /// whole read reaches the body.
    fn edited(bytes: &[u8], edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let v1 = cbin::Generation::V1;
        let (head, body) = bytes.split_at(v1.body_start() as usize);
        let mut body = body.to_vec();
        edit(&mut body);
        let mut out = head.to_vec();
        let checksum = v1.checksum_range(out.len()).unwrap();
        out[checksum].copy_from_slice(&crc32(&body).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// The body offset of the `n`-th section tagged `tag`.
    fn section_at(bytes: &[u8], tag: &[u8], n: usize) -> usize {
        let body = &bytes[cbin::Generation::V1.body_start() as usize..];
        let header = match body.starts_with(section::CONTAINER4) {
            true => section::HEADER4_LEN,
            false => section::HEADER_LEN,
        };
        // Both framings close their header with the u32 payload length.
        let len_at = header - 4;
        let mut at = 0;
        let mut seen = 0;
        loop {
            let len = u32::from_be_bytes(body[at + len_at..at + len_at + 4].try_into().unwrap());
            if &body[at..at + tag.len()] == tag {
                if seen == n {
                    return at;
                }
                seen += 1;
            }
            at += header + len as usize;
        }
    }

    /// The index refuses `bytes`, and a whole read refuses them for the same reason.
    fn refused_as_a_whole_read_refuses(bytes: &[u8], reason: &str) {
        let error = index(bytes).expect_err("the index accepted it").to_string();
        assert!(
            error.contains(reason),
            "refused for {error:?}, not {reason:?}"
        );
        let whole = whole_zones(bytes)
            .expect_err("a whole read accepted it")
            .to_string();
        assert!(
            whole.contains(&error),
            "the index refused for {error:?} and a whole read for {whole:?}"
        );
    }

    #[test]
    fn a_file_cut_inside_the_map_is_refused() {
        let bytes = built(Layout::V2);
        let map = section_at(&bytes, section::MAP, 0);
        let cut = edited(&bytes, |body| body.truncate(map + 100));
        refused_as_a_whole_read_refuses(&cut, "but the body ends first");
    }

    #[test]
    fn a_stroke_running_past_the_end_of_the_file_is_refused() {
        for layout in [Layout::V2, Layout::V4] {
            let bytes = built(layout);
            let tag: &[u8] = if layout == Layout::V2 {
                section::STK
            } else {
                section::STK4
            };
            let last = section_at(&bytes, tag, 2);
            let cut = edited(&bytes, |body| body.truncate(last + 40));
            refused_as_a_whole_read_refuses(&cut, "but the body ends first");
        }
    }

    #[test]
    fn a_stroke_whose_length_reaches_into_the_next_section_is_refused() {
        for layout in [Layout::V2, Layout::V3] {
            let bytes = built(layout);
            let (tag, len_at): (&[u8], usize) = if layout == Layout::V2 {
                (section::STK, 5)
            } else {
                (section::STK4, 8)
            };
            let first = section_at(&bytes, tag, 0) + len_at;
            let overrun = edited(&bytes, |body| {
                let len = u32::from_be_bytes(body[first..first + 4].try_into().unwrap());
                body[first..first + 4].copy_from_slice(&(len + 1).to_be_bytes());
            });
            assert!(
                index(&overrun).is_err(),
                "{layout:?}: the index accepted it"
            );
            assert!(
                whole_zones(&overrun).is_err(),
                "{layout:?}: a whole read accepted it"
            );
        }
    }

    #[test]
    fn bytes_after_the_last_section_are_refused() {
        let bytes = built(Layout::V2);
        let padded = edited(&bytes, |body| body.extend_from_slice(&[0; 3]));
        refused_as_a_whole_read_refuses(&padded, "truncated section header");
    }

    #[test]
    fn an_instrument_with_no_map_is_refused() {
        let bytes = built(Layout::V4);
        let map = section_at(&bytes, section::MAP4, 0);
        let unmapped = edited(&bytes, |body| body[map..map + 4].copy_from_slice(b"\0xyz"));
        refused_as_a_whole_read_refuses(&unmapped, "no map section");
    }

    #[test]
    fn a_zone_naming_a_stroke_the_file_does_not_hold_is_refused() {
        let bytes = built(Layout::V2);
        let map = section_at(&bytes, section::MAP, 0) + section::HEADER_LEN;
        let first_zone_id = map + zone::RECORDS_AT + 2;
        let orphaned = edited(&bytes, |body| body[first_zone_id] = 9);
        refused_as_a_whole_read_refuses(&orphaned, "names stroke 9");

        let bytes = built(Layout::V4);
        let map = section_at(&bytes, section::MAP4, 0) + section::HEADER4_LEN;
        let count_at = zone::Wide::V21.count_at().unwrap();
        let first_gid = map + count_at + 1 + zone::Wide::V21.gid_at() + 3;
        let orphaned = edited(&bytes, |body| body[first_gid] = 9);
        refused_as_a_whole_read_refuses(&orphaned, "stroke 9");
    }

    #[test]
    fn an_empty_body_is_refused() {
        let bytes = built(Layout::V3);
        let empty = edited(&bytes, Vec::clear);
        refused_as_a_whole_read_refuses(&empty, "found end of body");
    }

    #[test]
    fn a_content_version_past_the_codec_is_refused() {
        let mut bytes = built(Layout::V4);
        bytes[0x14..0x18].copy_from_slice(&codec::V5_FROM_VERSION.to_le_bytes());
        let error = index(&bytes).unwrap_err().to_string();
        let whole = sample(&bytes).unwrap().layout().unwrap_err().to_string();
        assert_eq!(error, whole);
    }

    #[test]
    fn a_container_of_another_format_is_refused() {
        let mut bytes = built(Layout::V2);
        bytes[0x08..0x0c].copy_from_slice(b"npno");
        let error = index(&bytes).unwrap_err().to_string();
        assert!(error.contains("expected a nsmp file, got npno"), "{error}");
    }

    /// A reader that records the span of every read.
    struct Recording<'a> {
        inner: Cursor<&'a [u8]>,
        reads: Vec<Range<u64>>,
    }

    impl Read for Recording<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let at = self.inner.position();
            let n = self.inner.read(buf)?;
            self.reads.push(at..at + n as u64);
            Ok(n)
        }
    }

    impl Seek for Recording<'_> {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    /// The index of `bytes`, having read no stroke byte past its opening.
    fn recorded(bytes: &[u8]) -> Index {
        let mut r = Recording {
            inner: Cursor::new(bytes),
            reads: Vec::new(),
        };
        let index = Index::read_from(&mut r).unwrap();
        for span in index.zones() {
            let audio = span.stream.start + Index::STROKE_OPENING as u64..span.stream.end;
            let past = r
                .reads
                .iter()
                .find(|read| read.start < audio.end && audio.start < read.end);
            assert!(past.is_none(), "{past:?} reads into the audio at {audio:?}");
        }
        index
    }

    /// Every built generation, in both container generations.
    fn every_build() -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for layout in [Layout::V2, Layout::V3, Layout::V4] {
            let bytes = built(layout);
            let mut file = sample(&bytes).unwrap();
            match &mut file {
                crate::Sample::V2(file) => file.header.generation = cbin::Generation::V0,
                crate::Sample::V3(file) => file.header.generation = cbin::Generation::V0,
            }
            out.push(file.to_bytes().unwrap());
            out.push(bytes);
        }
        out
    }

    #[test]
    fn the_outline_answers_as_a_whole_read_does() {
        for bytes in every_build() {
            let whole = sample(&bytes).unwrap();
            let outline = recorded(&bytes).outline().clone();
            assert_eq!(outline.name().unwrap(), whole.name().unwrap());
            assert_eq!(outline.max_name_len(), whole.max_name_len());
            assert_eq!(outline.generation(), whole.generation());
            assert_eq!(outline.chain().unwrap(), whole.chain().unwrap());
            assert_eq!(outline.zones_are_editable(), whole.zones_are_editable());
            assert_eq!(outline.has_low_note(), whole.has_low_note());
            match (outline.fields(), &whole) {
                (Fields::V2(fields), crate::Sample::V2(whole)) => {
                    assert_eq!(fields.chain().unwrap(), whole.chain().unwrap());
                    assert_eq!(fields.zones().unwrap(), whole.zones().unwrap());
                    assert_eq!(fields.key_table().unwrap(), whole.key_table().unwrap());
                    assert_eq!(fields.sty().unwrap(), whole.sty().unwrap());
                    assert_eq!(fields.categories(), whole.categories());
                }
                (Fields::V3(fields), crate::Sample::V3(whole)) => {
                    assert_eq!(fields.sub_name().unwrap(), whole.sub_name().unwrap());
                    assert_eq!(fields.zones().unwrap(), whole.zones().unwrap());
                    assert_eq!(fields.zone_table().unwrap(), whole.zone_table().unwrap());
                    assert_eq!(fields.sty().unwrap(), whole.sty().unwrap());
                    assert_eq!(fields.meta().unwrap(), whole.meta().unwrap());
                    assert_eq!(fields.stroke_count(), whole.stroke_count());
                }
                _ => panic!("the outline is of another generation than the whole read"),
            }
        }
    }

    /// One edit a sample document makes.
    #[derive(Debug, Clone, Copy)]
    enum Edit {
        Name(&'static str),
        Root(usize, u8),
        Top(usize, u8),
        Low(usize, u8),
        Key(u8),
    }

    const EDITS: [Edit; 6] = [
        Edit::Name("Patched"),
        Edit::Root(1, 62),
        Edit::Top(1, 70),
        Edit::Low(0, 72),
        Edit::Key(60),
        Edit::Name("a name far too long for the field either chain gives it, by some way"),
    ];

    /// A per-key level no specimen is built with.
    fn level() -> super::super::Level {
        super::super::Level::new(super::super::keymap::GAIN_UNITY / 2, -128).unwrap()
    }

    fn edit_whole(sample: &mut crate::Sample, edit: Edit) -> Result<(), Error> {
        match edit {
            Edit::Name(name) => sample.set_name(name),
            Edit::Root(i, note) => sample.set_root_key(i, note),
            Edit::Top(i, note) => sample.set_zone_top_note(i, note),
            Edit::Low(i, note) => sample.set_zone_low_note(i, note),
            Edit::Key(note) => match sample {
                crate::Sample::V2(file) => {
                    let mut table = file.key_table()?;
                    table.set_key(note, level())?;
                    file.set_key_table(&table)
                }
                crate::Sample::V3(_) => Err(ParseError::AssertFail("wide".into()).into()),
            },
        }
    }

    fn edit_outline(outline: &mut Outline, edit: Edit) -> Result<(), Error> {
        match edit {
            Edit::Name(name) => outline.set_name(name),
            Edit::Root(i, note) => outline.set_root_key(i, note),
            Edit::Top(i, note) => outline.set_zone_top_note(i, note),
            Edit::Low(i, note) => outline.set_zone_low_note(i, note),
            Edit::Key(note) => {
                let Fields::V2(fields) = outline.fields() else {
                    return Err(ParseError::AssertFail("wide".into()).into());
                };
                let mut table = fields.key_table()?;
                table.set_key(note, level())?;
                outline.set_key_table(&table)
            }
        }
    }

    #[test]
    fn a_patch_through_the_index_writes_the_bytes_a_whole_edit_writes() {
        for bytes in every_build() {
            let index = index(&bytes).unwrap();
            for edit in EDITS {
                let mut whole = sample(&bytes).unwrap();
                let whole = edit_whole(&mut whole, edit).and_then(|()| whole.to_bytes());
                let mut outline = index.outline().clone();
                let patched = edit_outline(&mut outline, edit).and_then(|()| {
                    let mut out = Vec::new();
                    index
                        .patch(&outline)?
                        .copy(&mut Cursor::new(&bytes), &mut out)?;
                    Ok(out)
                });
                match (whole, patched) {
                    (Ok(whole), Ok(patched)) => assert!(whole == patched, "{edit:?}"),
                    (Err(_), Err(_)) => {}
                    (whole, patched) => panic!(
                        "{edit:?}: a whole edit gives {:?} and a patch {:?}",
                        whole.map(|_| ()),
                        patched.map(|_| ())
                    ),
                }
            }
        }
    }

    #[test]
    fn a_patch_replaces_only_the_sections_an_edit_changes() {
        let bytes = built(Layout::V3);
        let index = index(&bytes).unwrap();
        let mut outline = index.outline().clone();
        outline.set_name("Patched").unwrap();
        let patch = index.patch(&outline).unwrap();
        let hdr = section_at(&bytes, section::HDR4, 0) + cbin::Generation::V1.body_start() as usize;
        let at: Vec<u64> = patch.splices().iter().map(|splice| splice.at).collect();
        assert_eq!(at, [0x18, (hdr + section::HEADER4_LEN) as u64]);
        assert!(index.patch(index.outline()).unwrap().splices().len() == 1);
    }

    #[test]
    fn an_outline_from_another_instrument_is_refused() {
        let narrow = index(&built(Layout::V2)).unwrap();
        let wide = index(&built(Layout::V3)).unwrap();
        let v4 = index(&built(Layout::V4)).unwrap();
        let error = narrow.patch(wide.outline()).unwrap_err().to_string();
        assert!(error.contains("another generation"), "{error}");
        assert!(wide.patch(v4.outline()).is_err());
    }
}
