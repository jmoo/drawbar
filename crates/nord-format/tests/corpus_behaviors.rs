#![cfg(feature = "corpus")]
//! Behavioral checks against files produced by the instrument and sample editor.

use nord_format::formats::nsmp;
use nord_format::{Entity, Live, Program, Sample};
use std::io::Cursor;

#[path = "support/scan.rs"]
mod scan;

use scan::{corpus, named, Specimen};

fn ne5_programs() -> impl Iterator<
    Item = (
        &'static Specimen,
        &'static nord_format::cbin::Cbin<nord_format::formats::ne5::Program>,
    ),
> {
    corpus().iter().filter_map(|s| match &s.entity {
        Entity::Program(Program::Electro5(p)) => Some((s, p)),
        _ => None,
    })
}

fn ne5_lives() -> impl Iterator<
    Item = (
        &'static Specimen,
        &'static nord_format::cbin::Cbin<nord_format::formats::ne5::Program>,
    ),
> {
    corpus().iter().filter_map(|s| match &s.entity {
        Entity::Live(Live::Electro5(p)) => Some((s, p)),
        _ => None,
    })
}

fn v2_samples() -> impl Iterator<
    Item = (
        &'static Specimen,
        &'static nord_format::cbin::Cbin<nsmp::Sample>,
    ),
> {
    corpus().iter().filter_map(|s| match &s.entity {
        Entity::Sample(Sample::V2(v)) => Some((s, v)),
        _ => None,
    })
}

fn v2_named(name: &str) -> nord_format::cbin::Cbin<nsmp::Sample> {
    match nord_format::from_stream(&mut Cursor::new(&named(name).bytes)).unwrap() {
        Entity::Sample(Sample::V2(sample)) => sample,
        other => panic!("{name} decoded as {other:?}"),
    }
}

/// Confirmed on hardware: a live slot and a stored program use the same body.
#[test]
fn ne5_live_body_decodes_as_a_program() {
    use nord_format::formats::ne5;

    let mut seen = 0;
    for (specimen, live) in ne5_lives() {
        let mut bytes = specimen.bytes.clone();
        bytes[0x08..0x0c].copy_from_slice(ne5::program::FORMAT.as_bytes());
        if bytes[0x04] == 0 {
            let at = bytes.len() - 2;
            let crc = nord_format::crc::crc16(&bytes[..at]);
            bytes[at..].copy_from_slice(&crc.to_le_bytes());
        }

        let Entity::Program(Program::Electro5(program)) =
            nord_format::from_stream(&mut Cursor::new(&bytes)).unwrap()
        else {
            panic!("retagged live slot decoded as another entity")
        };

        let fields = |fields: Vec<nord_format::fields::Field>| {
            fields
                .into_iter()
                .map(|field| (field.path, field.display))
                .collect::<Vec<_>>()
        };
        assert_eq!(fields(live.fields()), fields(program.fields()));
        seen += 1;
    }
    assert!(seen > 0, "no Electro 5 live slot in the corpus");
}

/// The drawbar accessors must be read/write inverses without disturbing other bits.
#[test]
fn ne5_drawbars_survive_a_rewrite() {
    use nord_format::formats::ne5::OrganModel::{Farfisa, Pipe, Vox, B3};

    let mut seen = 0;
    for (specimen, _) in ne5_programs() {
        let Entity::Program(Program::Electro5(mut program)) =
            nord_format::from_stream(&mut Cursor::new(&specimen.bytes)).unwrap()
        else {
            unreachable!()
        };
        for model in [B3, Vox, Farfisa, Pipe] {
            for preset in [1, 2] {
                let bars = program.organ_panel.drawbars(model, preset);
                if bars.iter().all(|&bar| bar <= 8) {
                    program
                        .organ_panel
                        .set_drawbars(model, preset, bars)
                        .unwrap();
                }
            }
        }

        let mut rewritten = Vec::new();
        program.write_to(&mut Cursor::new(&mut rewritten)).unwrap();
        assert_eq!(specimen.bytes, rewritten, "{}", specimen.path.display());
        seen += 1;
    }
    assert!(seen > 0, "no Electro 5 program in the corpus");
}

/// Each readable zone table must account for every encoded stroke.
#[test]
fn nsmp_strokes_match_zones() {
    let mut seen = 0;
    for (specimen, sample) in v2_samples() {
        let Ok(zones) = sample.zones() else {
            continue;
        };
        let strokes = sample
            .strokes()
            .unwrap_or_else(|error| panic!("{}: {error}", specimen.path.display()));
        assert_eq!(strokes.len(), zones.len(), "{}", specimen.path.display());
        seen += strokes.len();
    }
    assert!(seen > 0, "no readable v2 strokes in the corpus");
}

/// Rename and remap reproduce the file written by the sample editor.
#[test]
fn nsmp_edits_reproduce_editor_output() {
    let mut sample = v2_named("D4-3zones.nsmp");
    sample.set_name("D7-upperkey").unwrap();
    sample.set_zone_top_note(1, 60).unwrap();
    assert_eq!(sample.to_bytes().unwrap(), named("D7-upperkey.nsmp").bytes);
}

/// Retuning changes the root-key byte and the container checksum only.
#[test]
fn nsmp_retune_is_surgical() {
    let before = &named("D1-one-zone.nsmp").bytes;
    let mut sample = v2_named("D1-one-zone.nsmp");
    sample.set_root_key(0, 48).unwrap();
    let after = sample.to_bytes().unwrap();

    let changed = (0..before.len())
        .filter(|&index| before[index] != after[index])
        .collect::<Vec<_>>();
    assert_eq!(changed.len(), 5, "changed bytes: {changed:?}");
    assert!(changed[..4].iter().eq([0x18, 0x19, 0x1a, 0x1b].iter()));
    assert_eq!(sample.strokes().unwrap()[0].root_key, 48);
}

#[test]
fn nsmp_overlong_name_is_refused_without_mutation() {
    let mut sample = v2_named("D1-one-zone.nsmp");
    assert!(sample.set_name("a name that is far too long").is_err());
    assert_eq!(sample.name().unwrap(), "TEST");
}

#[test]
fn nsmp_bad_checksum_is_refused() {
    let mut bytes = named("D1-one-zone.nsmp").bytes.clone();
    *bytes.last_mut().unwrap() ^= 0xff;
    assert!(nord_format::from_stream(&mut Cursor::new(&bytes)).is_err());
}
