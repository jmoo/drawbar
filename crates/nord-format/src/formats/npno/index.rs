//! Where each stroke's audio sits in a piano library, read without the audio.

use super::{overflow, Extent, Library, Stroke, DIRECTORY_AT, FORMAT};
use crate::cbin;
use crate::error::{try_vec, Error, ParseError};
use std::borrow::Cow;
use std::io::{Read, Seek};
use std::ops::Range;

/// A piano library's prefix and stroke directory, with the position of each stroke's
/// audio in the stream, read from the container header, the prefix and the directory.
///
/// No audio byte is read, so memory and I/O grow with the stroke count, not with the
/// library. A stroke's audio can then be read by its range, positionally from a file or
/// as a slice of a browser `File`, and paired with its record by [`Index::stroke`].
///
/// The layout is checked as [`Library::borrow`] checks it, and a file one refuses the
/// other refuses for the same reason.
///
/// ⚠️ The container checksum covers every body byte and is not verified here.
/// [`cbin::inspect`] verifies it in one streaming pass.
#[derive(Debug)]
pub struct Index {
    library: Library<'static>,
    audio: Vec<Range<u64>>,
}

impl Index {
    /// Read the index of the `npno` file that starts at the reader's position and runs
    /// to the end of the stream.
    pub fn read_from(r: &mut (impl Read + Seek)) -> Result<Index, Error> {
        let (header, body) = cbin::locate_body(r, FORMAT)?;
        let body_len = body
            .end
            .checked_sub(body.start)
            .and_then(|len| usize::try_from(len).ok())
            .ok_or_else(|| overflow("the body"))?;

        let mut head = try_vec(body_len.min(DIRECTORY_AT))?;
        cbin::read_at(r, body.start, &mut head)?;
        let head_len = body_len.min(Extent::of(&head)?.first_audio);
        head.try_reserve_exact(head_len - head.len())
            .map_err(|_| overflow("the stroke directory"))?;
        head.resize(head_len, 0);
        let directory_at = offset(&body, DIRECTORY_AT)?;
        cbin::read_at(r, directory_at, &mut head[DIRECTORY_AT..])?;

        let (library, spans) = Library::skeleton(header, &head, body_len)?;
        let audio = spans
            .into_iter()
            .map(|span| Ok(offset(&body, span.start)?..offset(&body, span.end)?))
            .collect::<Result<_, Error>>()?;
        Ok(Index { library, audio })
    }

    /// The header, the prefix and every stroke's record, as a parse holds them.
    ///
    /// ⚠️ Its strokes carry no audio: [`Stroke::audio`] is empty, so
    /// [`codec::decode`](super::codec::decode) refuses them and [`Library::to_body`]
    /// refuses the library. [`Index::stroke`] pairs a record with its audio.
    pub fn library(&self) -> &Library<'static> {
        &self.library
    }

    /// Where each stroke's audio sits in the stream the index was read from, in the
    /// order of [`Library::strokes`]. The ranges abut in that order, and the last one
    /// ends where the body does.
    pub fn audio_ranges(&self) -> &[Range<u64>] {
        &self.audio
    }

    /// The `index`-th stroke, holding `audio`: the bytes read from its range, ready for
    /// [`codec::decode`](super::codec::decode). Audio of any other length is refused.
    pub fn stroke<'a>(&self, index: usize, audio: &'a [u8]) -> Result<Stroke<'a>, Error> {
        let count = self.audio.len();
        let (stroke, range) = self
            .library
            .strokes
            .get(index)
            .zip(self.audio.get(index))
            .ok_or_else(|| ParseError::OutOfBounds {
                value: format!("stroke {index}"),
                bound: format!("the {count} strokes the directory holds"),
            })?;
        let len = range.end - range.start;
        if u64::try_from(audio.len()) != Ok(len) {
            return Err(ParseError::AssertFail(format!(
                "stroke {index} spans {len} bytes, and {} were given",
                audio.len()
            ))
            .into());
        }
        Ok(Stroke {
            root: stroke.root,
            record: stroke.record,
            audio: Cow::Borrowed(audio),
        })
    }
}

/// The stream position of body offset `at`, which must lie within the body.
fn offset(body: &Range<u64>, at: usize) -> Result<u64, Error> {
    u64::try_from(at)
        .ok()
        .and_then(|at| cbin::body_position(body, at))
        .ok_or_else(|| overflow("a body offset"))
}

#[cfg(test)]
mod tests {
    use super::super::synthetic::{take, Build};
    use super::super::{codec, put16, put32, Bank, RECORD, REC_BLOCKS, REC_START, STROKE_COUNT_AT};
    use super::*;
    use crate::cbin::Generation;
    use std::io::Cursor;

    fn index(bytes: &[u8]) -> Result<Index, Error> {
        Index::read_from(&mut Cursor::new(bytes))
    }

    /// Where a synthetic library's body starts: it is written as a type-1 file.
    fn body_start() -> usize {
        Generation::V1.body_start() as usize
    }

    /// A built library's file bytes after `edit` changes its body; the container
    /// checksum is recomputed.
    fn edited(build: &Build, edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut piano = build.piano();
        edit(&mut piano.file.body.0);
        let mut out = Cursor::new(Vec::new());
        piano.write_to(&mut out).unwrap();
        out.into_inner()
    }

    /// The index and a whole parse agree on every stroke: its record, its root, and the
    /// bytes at its range are the audio the parse attributes to it.
    fn agrees_with_a_parse(bytes: &[u8]) {
        let parsed = Library::borrow(bytes).unwrap();
        let index = index(bytes).unwrap();
        let ranges = index.audio_ranges();
        assert_eq!(ranges.len(), parsed.strokes().len());
        for (i, (range, whole)) in ranges.iter().zip(parsed.strokes()).enumerate() {
            let audio = &bytes[range.start as usize..range.end as usize];
            let stroke = index.stroke(i, audio).unwrap();
            assert_eq!(stroke.audio(), whole.audio(), "stroke {i}'s audio");
            assert_eq!(stroke.record(), whole.record(), "stroke {i}'s record");
            assert_eq!(stroke.root, whole.root, "stroke {i}'s root");
        }
        assert_eq!(index.library().key_map(), parsed.key_map());
        assert_eq!(index.library().name(), parsed.name());
        assert_eq!(index.library().channels(), parsed.channels());
    }

    #[test]
    fn each_range_holds_the_audio_a_parse_gives_its_stroke() {
        agrees_with_a_parse(&Build::new().bytes().unwrap());

        let mut stereo = Build::new();
        stereo.channels = 2;
        agrees_with_a_parse(&stereo.bytes().unwrap());
    }

    #[test]
    fn the_last_range_stops_before_a_type_0_trailer() {
        let mut piano = Build::new().piano();
        piano.file.header.generation = Generation::V0;
        let mut out = Cursor::new(Vec::new());
        piano.write_to(&mut out).unwrap();
        let bytes = out.into_inner();

        agrees_with_a_parse(&bytes);
        let last = index(&bytes)
            .unwrap()
            .audio_ranges()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last.end, bytes.len() as u64 - 2);
    }

    #[test]
    fn ranges_are_positions_in_the_stream_wherever_the_file_starts() {
        let bytes = Build::new().bytes().unwrap();
        let mut stream = b"leading".to_vec();
        stream.extend_from_slice(&bytes);
        let mut r = Cursor::new(&stream);
        r.set_position(7);
        let shifted = Index::read_from(&mut r).unwrap();
        let direct = index(&bytes).unwrap();
        for (at, (moved, placed)) in shifted
            .audio_ranges()
            .iter()
            .zip(direct.audio_ranges())
            .enumerate()
        {
            assert_eq!(moved.start, placed.start + 7, "stroke {at}");
            assert_eq!(moved.end, placed.end + 7, "stroke {at}");
        }
    }

    #[test]
    fn a_stroke_read_by_its_range_decodes_as_the_parsed_stroke_does() {
        let mut build = Build::new();
        build.takes = vec![
            take(60, Bank::Attack, 0, 2).silent(),
            take(72, Bank::Attack, 0, 1).silent(),
        ];
        let bytes = build.bytes().unwrap();
        let parsed = Library::borrow(&bytes).unwrap();
        let index = index(&bytes).unwrap();
        for (i, range) in index.audio_ranges().iter().enumerate() {
            let audio = &bytes[range.start as usize..range.end as usize];
            let stroke = index.stroke(i, audio).unwrap();
            assert_eq!(
                codec::decode(&stroke, index.library().channels()).unwrap(),
                codec::decode(&parsed.strokes()[i], parsed.channels()).unwrap(),
                "stroke {i}"
            );
        }
    }

    #[test]
    fn a_stroke_is_refused_audio_of_another_length_or_an_index_past_the_directory() {
        let bytes = Build::new().bytes().unwrap();
        let index = index(&bytes).unwrap();
        let range = index.audio_ranges()[0].clone();
        let audio = &bytes[range.start as usize..range.end as usize];

        let error = index.stroke(0, &audio[1..]).unwrap_err().to_string();
        assert!(error.contains("stroke 0 spans 1022 bytes"), "{error}");
        let error = index.stroke(3, audio).unwrap_err().to_string();
        assert!(error.contains("stroke 3"), "{error}");
        assert!(index.stroke(2, audio).is_err(), "stroke 2 spans two blocks");
    }

    #[test]
    fn a_library_with_no_strokes_has_no_ranges() {
        let mut build = Build::new();
        build.takes.clear();
        build.map.clear();
        let bytes = build.bytes().unwrap();
        assert!(index(&bytes).unwrap().audio_ranges().is_empty());
        agrees_with_a_parse(&bytes);
    }

    /// The index refuses `bytes`, and a whole parse refuses them for the same reason.
    fn refused_as_a_parse_refuses(bytes: &[u8], reason: &str) {
        let error = index(bytes).expect_err("the index accepted it").to_string();
        assert!(
            error.contains(reason),
            "refused for {error:?}, not {reason:?}"
        );
        let parse = Library::borrow(bytes)
            .expect_err("a parse accepted it")
            .to_string();
        assert_eq!(error, parse, "the index and a parse disagree on why");
    }

    #[test]
    fn a_file_cut_inside_the_directory_is_refused() {
        let bytes = Build::new().bytes().unwrap();
        let cut = body_start() + DIRECTORY_AT + RECORD + 5;
        refused_as_a_parse_refuses(&bytes[..cut], "ends inside the stroke directory");
    }

    #[test]
    fn a_file_cut_inside_the_prefix_is_refused() {
        let bytes = Build::new().bytes().unwrap();
        refused_as_a_parse_refuses(&bytes[..body_start() + 0x700], "ends inside the prefix");
    }

    #[test]
    fn a_span_running_past_the_end_of_the_file_is_refused() {
        let build = Build::new();
        let last = DIRECTORY_AT + 2 * RECORD + REC_BLOCKS;
        let bytes = edited(&build, |body| put16(body, last, 3));
        refused_as_a_parse_refuses(&bytes, "ends inside a stroke's audio span");

        let bytes = edited(&build, |body| put16(body, last, u16::MAX));
        refused_as_a_parse_refuses(&bytes, "ends inside a stroke's audio span");
    }

    #[test]
    fn a_span_that_overlaps_the_one_before_it_is_refused() {
        let build = Build::new();
        let second = DIRECTORY_AT + RECORD + REC_START;
        let bytes = edited(&build, |body| {
            let start = u32::from_be_bytes(body[second..second + 4].try_into().unwrap());
            put32(body, second, start - 2);
        });
        refused_as_a_parse_refuses(&bytes, "stroke 1 starts at");
    }

    #[test]
    fn bytes_after_the_last_span_are_refused() {
        let bytes = edited(&Build::new(), |body| body.extend_from_slice(&[0; 4]));
        refused_as_a_parse_refuses(&bytes, "the audio ends at");
    }

    #[test]
    fn a_nonzero_alignment_gap_is_refused() {
        let gap = DIRECTORY_AT + 3 * RECORD;
        let bytes = edited(&Build::new(), |body| body[gap] = 1);
        refused_as_a_parse_refuses(&bytes, "alignment gap before the audio is not zero");
    }

    #[test]
    fn counts_that_disagree_with_the_directory_are_refused() {
        let bytes = edited(&Build::new(), |body| put16(body, STROKE_COUNT_AT, 4));
        refused_as_a_parse_refuses(
            &bytes,
            "per-root counts sum to 3 where the stroke count is 4",
        );
    }

    #[test]
    fn a_stream_version_never_validated_is_refused() {
        let mut build = Build::new();
        build.version = 0x500;
        refused_as_a_parse_refuses(&build.bytes().unwrap(), "schema version 1280");
    }

    #[test]
    fn a_container_of_another_format_is_refused() {
        let mut bytes = Build::new().bytes().unwrap();
        bytes[0x08..0x0c].copy_from_slice(b"nsmp");
        refused_as_a_parse_refuses(&bytes, "expected a npno file, got nsmp");
    }

    #[test]
    fn a_key_routed_to_a_root_no_stroke_records_is_refused() {
        let mut build = Build::new();
        build.map.push((80, 80));
        refused_as_a_parse_refuses(&build.bytes().unwrap(), "key 80 plays root 80");
    }
}
