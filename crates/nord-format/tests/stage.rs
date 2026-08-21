//! The Stage bodies in the default suite: a synthetic specimen per decoded body, so
//! placing, gating and the round-trip invariant are exercised without the corpus.
//!
//! A zeroed body is a legal one for every Stage format; a body of patterned bytes is
//! not (ranged fields refuse it) — so the re-encode check runs on zeros, and the
//! pattern is only asked to fail cleanly rather than panic.

use nord_format::cbin::{Cbin, Header};
use nord_format::formats::{ns2, ns3, ns4};
use nord_format::{Entity, Live, OrganPreset, PianoPreset, Program, Synth};

macro_rules! stage_body {
    ($name:ident, $body:ty, $len:expr, $format:expr, $versions:expr, $wrap:expr, $unwrap:pat => $inner:expr) => {
        mod $name {
            use super::*;

            fn file(body: $body, version: u32) -> Vec<u8> {
                let file = Cbin {
                    header: Header::new($format, (0, 0), version),
                    body,
                };
                nord_format::to_bytes(&$wrap(file)).expect("a synthetic file encodes")
            }

            #[test]
            fn a_zeroed_body_decodes_and_re_encodes_byte_for_byte() {
                let body = <$body>::try_from([0u8; $len]).expect("a zeroed body decodes");
                let version = *$versions.last().unwrap();
                let bytes = file(body, version);
                let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes))
                    .expect("the file reads back");
                match &entity {
                    $unwrap => assert_eq!($inner.header.version, version),
                    other => panic!("decoded to {other:?}"),
                }
                assert_eq!(nord_format::to_bytes(&entity).unwrap(), bytes);
            }

            #[test]
            fn unclaimed_bits_ride_through_a_re_encode() {
                // Every byte set: what no field claims must come back as it went in,
                // and what a field claims either decodes or is refused — never wrapped.
                let raw = [0xffu8; $len];
                if let Ok(body) = <$body>::try_from(raw) {
                    assert_eq!(<[u8; $len]>::from(&body), raw);
                }
            }

            #[test]
            fn an_unknown_version_is_refused() {
                let body = <$body>::try_from([0u8; $len]).unwrap();
                let bytes = file(body, 999_999);
                let err = nord_format::from_stream(&mut std::io::Cursor::new(&bytes))
                    .expect_err("a version the offsets were never checked against");
                assert!(err.to_string().contains("999999"), "{err}");
            }
        }
    };
}

stage_body!(
    stage2_program,
    ns2::Program,
    ns2::program::BODY_LEN,
    ns2::program::FORMAT,
    ns2::program::KNOWN_VERSIONS,
    |f| Entity::Program(Program::Stage2(f)),
    Entity::Program(Program::Stage2(f)) => f
);
stage_body!(
    stage2_live,
    ns2::Program,
    ns2::program::BODY_LEN,
    ns2::live::FORMAT,
    ns2::program::KNOWN_VERSIONS,
    |f| Entity::Live(Live::Stage2(f)),
    Entity::Live(Live::Stage2(f)) => f
);
stage_body!(
    stage3_program,
    ns3::Program,
    ns3::program::BODY_LEN,
    ns3::program::FORMAT,
    ns3::program::KNOWN_VERSIONS,
    |f| Entity::Program(Program::Stage3(f)),
    Entity::Program(Program::Stage3(f)) => f
);
stage_body!(
    stage3_live,
    ns3::Program,
    ns3::program::BODY_LEN,
    ns3::live::FORMAT,
    ns3::program::KNOWN_VERSIONS,
    |f| Entity::Live(Live::Stage3(f)),
    Entity::Live(Live::Stage3(f)) => f
);
stage_body!(
    stage3_synth,
    ns3::SynthPreset,
    ns3::synth::BODY_LEN,
    ns3::synth::FORMAT,
    ns3::synth::KNOWN_VERSIONS,
    |f| Entity::Synth(Synth::Stage3(f)),
    Entity::Synth(Synth::Stage3(f)) => f
);
stage_body!(
    stage4_program,
    ns4::Program,
    ns4::program::BODY_LEN,
    ns4::program::FORMAT,
    ns4::program::KNOWN_VERSIONS,
    |f| Entity::Program(Program::Stage4(f)),
    Entity::Program(Program::Stage4(f)) => f
);
stage_body!(
    stage4_live,
    ns4::Program,
    ns4::program::BODY_LEN,
    ns4::live::FORMAT,
    ns4::program::KNOWN_VERSIONS,
    |f| Entity::Live(Live::Stage4(f)),
    Entity::Live(Live::Stage4(f)) => f
);
stage_body!(
    stage4_synth,
    ns4::synth::SynthPreset,
    ns4::synth::BODY_LEN,
    ns4::synth::FORMAT,
    ns4::synth::KNOWN_VERSIONS,
    |f| Entity::Synth(Synth::Stage4(f)),
    Entity::Synth(Synth::Stage4(f)) => f
);
stage_body!(
    stage4_piano_preset,
    ns4::piano_preset::PianoPreset,
    ns4::piano_preset::BODY_LEN,
    ns4::piano_preset::FORMAT,
    ns4::piano_preset::KNOWN_VERSIONS,
    |f| Entity::PianoPreset(PianoPreset::Stage4(f)),
    Entity::PianoPreset(PianoPreset::Stage4(f)) => f
);
stage_body!(
    stage4_organ_preset,
    ns4::organ_preset::OrganPreset,
    ns4::organ_preset::BODY_LEN,
    ns4::organ_preset::FORMAT,
    ns4::organ_preset::KNOWN_VERSIONS,
    |f| Entity::OrganPreset(OrganPreset::Stage4(f)),
    Entity::OrganPreset(OrganPreset::Stage4(f)) => f
);

/// The one placement in the Stage 4 synth preset the corpus confirms by analogy rather
/// than by a published table: layer A's zone sits one layer stride above B's.
#[test]
fn stage4_synth_layers_are_one_stride_apart() {
    let mut raw = [0u8; ns4::synth::BODY_LEN];
    // Both zones = 9, MSB-first: bits 338..=341 are byte 42's bits 2..=5, and
    // 746..=749 byte 93's — the stride is a whole number of bytes.
    raw[42] |= 0b0010_0100;
    raw[93] |= 0b0010_0100;
    let body = ns4::synth::SynthPreset::try_from(raw).unwrap();
    assert_eq!(
        format!("{:?}", body.synth_a_kb_zones),
        format!("{:?}", body.synth_b_kb_zones)
    );
    assert_eq!(<[u8; ns4::synth::BODY_LEN]>::from(&body), raw);
}
