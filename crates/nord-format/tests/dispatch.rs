//! Every registered CBIN tag dispatches to its entity and round-trips, both
//! header generations — no corpus needed: the files are synthesized through the
//! same container writer the library uses.

use nord_format::cbin::{Cbin, Generation, Header, RawBody};
use nord_format::formats::{ns3, ns4};
use std::io::Cursor;

#[path = "support/format_table.rs"]
mod format_table;
use format_table::formats;

fn synthesize(tag: &str, body_len: u64, version: u32, generation: Generation) -> Vec<u8> {
    let mut header = Header::new(tag, (0, 0), version);
    header.generation = generation;
    let file = Cbin {
        header,
        body: RawBody(vec![0u8; body_len as usize]),
    };
    let mut out = Cursor::new(Vec::new());
    file.write_to(&mut out).unwrap();
    out.into_inner()
}

#[test]
fn every_tag_dispatches_and_round_trips_both_generations() {
    for (tag, body_len, version) in formats() {
        for generation in [Generation::V1, Generation::V0] {
            let bytes = synthesize(tag, body_len, version, generation);
            let entity = nord_format::from_stream(&mut Cursor::new(&bytes))
                .unwrap_or_else(|e| panic!("{tag:?} ({generation:?}): {e}"));
            assert_eq!(
                entity.identity().format,
                tag,
                "{tag:?} dispatched to {:?}",
                entity.identity()
            );
            let back = nord_format::to_bytes(&entity)
                .unwrap_or_else(|e| panic!("{tag:?} ({generation:?}) re-encode: {e}"));
            assert_eq!(back, bytes, "{tag:?} ({generation:?}) round trip");
        }
    }
}

/// A tag with a NUL in it dispatches by all four bytes: `nss\0` is not `nssX`.
#[test]
fn nul_padded_tags_are_matched_in_full() {
    let bytes = synthesize("nssX", 27, 100, Generation::V0);
    let err = nord_format::from_stream(&mut Cursor::new(&bytes)).unwrap_err();
    assert!(
        err.to_string().contains("unknown format"),
        "expected an unknown-format refusal, got {err}"
    );
}

/// An unknown version on a globals-decoded format refuses rather than misreads;
/// the same version on a stub is preserved, because a raw body cannot misread.
#[test]
fn version_gates_cover_the_decoded_formats_only() {
    let bytes = synthesize(
        ns3::program::FORMAT,
        ns3::program::BODY_LEN as u64,
        999,
        Generation::V1,
    );
    assert!(nord_format::from_stream(&mut Cursor::new(&bytes)).is_err());

    let bytes = synthesize(
        ns4::settings::FORMAT,
        ns4::settings::BODY_LEN,
        999,
        Generation::V1,
    );
    assert!(nord_format::from_stream(&mut Cursor::new(&bytes)).is_ok());
}
