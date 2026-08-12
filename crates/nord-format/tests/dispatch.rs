//! Every registered CBIN tag dispatches to its entity and round-trips, both
//! header generations — no corpus needed: the files are synthesized through the
//! same container writer the library uses.

use nord_format::cbin::{Cbin, Generation, Header, RawBody};
use nord_format::formats::{
    nc2, nc2d, nd2, nd3, ne3, ne4, ne6, ne7, ng2, nl4, nla1, no3, np, np2, np3, np4, np5, npip,
    ns2, ns3, ns4, nsclassic, nw, nw2,
};
use std::io::Cursor;

/// `(tag, corpus body length, a version the reader accepts)` for every format
/// this crate reads as a stub or a globals-decode. The lengths restate the
/// modules' own constants so a swapped pair of tags cannot pass unnoticed.
fn formats() -> Vec<(&'static str, u64, u32)> {
    vec![
        (nc2::program::FORMAT, nc2::program::BODY_LEN, 100),
        (nc2::settings::FORMAT, nc2::settings::BODY_LEN, 100),
        (nc2d::program::FORMAT, nc2d::program::BODY_LEN, 100),
        (nc2d::settings::FORMAT, nc2d::settings::BODY_LEN, 100),
        (nd2::program::FORMAT, nd2::program::BODY_LEN, 3),
        (nd3::kit::FORMAT, nd3::kit::BODY_LEN, 1),
        (ne3::program::FORMAT, ne3::program::BODY_LEN, 101),
        (ne3::organ_preset::FORMAT, ne3::organ_preset::BODY_LEN, 100),
        (ne4::program::FORMAT, ne4::program::BODY_LEN, 103),
        (ne4::live::FORMAT, ne4::live::BODY_LEN, 103),
        (ne4::settings::FORMAT, ne4::settings::BODY_LEN, 100),
        (ne6::program::FORMAT, ne6::program::BODY_LEN, 204),
        (ne6::live::FORMAT, ne6::live::BODY_LEN, 204),
        (ne6::settings::FORMAT, ne6::settings::BODY_LEN, 200),
        (ne7::program::FORMAT, ne7::program::BODY_LEN, 110),
        (ne7::live::FORMAT, ne7::live::BODY_LEN, 110),
        (ne7::settings::FORMAT, ne7::settings::BODY_LEN, 301),
        (ng2::program::FORMAT, ng2::program::BODY_LEN, 102),
        (ng2::live::FORMAT, ng2::live::BODY_LEN, 102),
        (ng2::settings::FORMAT, ng2::settings::BODY_LEN, 102),
        (nl4::program::FORMAT, nl4::program::BODY_LEN, 7),
        (nl4::performance::FORMAT, nl4::performance::BODY_LEN, 7),
        (nl4::settings::FORMAT, nl4::settings::BODY_LEN, 3),
        (nla1::program::FORMAT, nla1::program::BODY_LEN, 6),
        (nla1::performance::FORMAT, nla1::performance::BODY_LEN, 6),
        (nla1::settings::FORMAT, nla1::settings::BODY_LEN, 0),
        (no3::program::FORMAT, no3::program::BODY_LEN, 200),
        (no3::settings::FORMAT, no3::settings::BODY_LEN, 200),
        (np::program::FORMAT, np::program::BODY_LEN, 103),
        (np::live::FORMAT, np::live::BODY_LEN, 104),
        (np::settings::FORMAT, np::settings::BODY_LEN, 100),
        (np2::program::FORMAT, np2::program::BODY_LEN, 1),
        (np2::live::FORMAT, np2::live::BODY_LEN, 0),
        (np2::settings::FORMAT, np2::settings::BODY_LEN, 1),
        (np3::program::FORMAT, np3::program::BODY_LEN, 4),
        (np3::live::FORMAT, np3::live::BODY_LEN, 4),
        (np3::settings::FORMAT, np3::settings::BODY_LEN, 0),
        (np4::program::FORMAT, np4::program::BODY_LEN, 100),
        (np4::live::FORMAT, np4::live::BODY_LEN, 100),
        (np4::settings::FORMAT, np4::settings::BODY_LEN, 100),
        (np5::program::FORMAT, np5::program::BODY_LEN, 101),
        (np5::live::FORMAT, np5::live::BODY_LEN, 101),
        (np5::settings::FORMAT, np5::settings::BODY_LEN, 100),
        (ns2::program::FORMAT, ns2::program::BODY_LEN as u64, 6),
        (ns2::live::FORMAT, ns2::program::BODY_LEN as u64, 6),
        (ns2::synth::FORMAT, ns2::synth::BODY_LEN, 6),
        (ns2::settings::FORMAT, ns2::settings::BODY_LEN, 4),
        (ns3::program::FORMAT, ns3::program::BODY_LEN as u64, 304),
        (ns3::live::FORMAT, ns3::program::BODY_LEN as u64, 304),
        (ns3::song::FORMAT, ns3::song::BODY_LEN, 300),
        (ns3::synth::FORMAT, ns3::synth::BODY_LEN as u64, 300),
        (ns3::settings::FORMAT, ns3::settings::BODY_LEN, 300),
        (ns4::program::FORMAT, ns4::program::BODY_LEN as u64, 313),
        (ns4::live::FORMAT, ns4::program::BODY_LEN as u64, 313),
        (ns4::synth::FORMAT, ns4::synth::BODY_LEN as u64, 208),
        (
            ns4::piano_preset::FORMAT,
            ns4::piano_preset::BODY_LEN as u64,
            203,
        ),
        (
            ns4::organ_preset::FORMAT,
            ns4::organ_preset::BODY_LEN as u64,
            205,
        ),
        (ns4::settings::FORMAT, ns4::settings::BODY_LEN, 106),
        (
            nsclassic::program::FORMAT,
            nsclassic::program::BODY_LEN,
            316,
        ),
        (nsclassic::synth::FORMAT, nsclassic::synth::BODY_LEN, 100),
        (nsclassic::piano_library::FORMAT, 64, 210),
        (npip::pipe_library::FORMAT, 64, 100),
        (nw::program::FORMAT, nw::program::BODY_LEN, 8),
        (nw::settings::FORMAT, nw::settings::BODY_LEN, 5),
        (nw2::program::FORMAT, nw2::program::BODY_LEN, 301),
        (nw2::live::FORMAT, nw2::live::BODY_LEN, 301),
        (nw2::settings::FORMAT, nw2::settings::BODY_LEN, 300),
    ]
}

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
