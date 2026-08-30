#![cfg(feature = "corpus")]
//! Cross-specimen codec contracts that the per-file corpus sweep cannot express.

use nord_format::formats::nsmp;
use nord_format::{Entity, Sample};
use std::io::Cursor;

#[path = "support/scan.rs"]
mod scan;
#[path = "support/sidecar.rs"]
mod sidecar;

use scan::{corpus, named, Specimen};

fn sample_streams() -> impl Iterator<
    Item = (
        &'static Specimen,
        nsmp::codec::Layout,
        Vec<(usize, &'static [u8])>,
    ),
> {
    corpus()
        .iter()
        .filter_map(|specimen| match &specimen.entity {
            Entity::Sample(Sample::V2(sample)) => {
                Some((specimen, nsmp::codec::Layout::V2, sample.stroke_streams()))
            }
            Entity::Sample(Sample::V3(sample)) => Some((
                specimen,
                nsmp::codec::Layout::from_version(sample.header.version),
                sample.stroke_streams(),
            )),
            _ => None,
        })
}

fn v2_samples() -> impl Iterator<
    Item = (
        &'static Specimen,
        &'static nord_format::cbin::Cbin<nsmp::Sample>,
    ),
> {
    corpus()
        .iter()
        .filter_map(|specimen| match &specimen.entity {
            Entity::Sample(Sample::V2(sample)) => Some((specimen, sample)),
            _ => None,
        })
}

fn v2_named(name: &str) -> nord_format::cbin::Cbin<nsmp::Sample> {
    match nord_format::from_stream(&mut Cursor::new(&named(name).bytes)).unwrap() {
        Entity::Sample(Sample::V2(sample)) => sample,
        other => panic!("{name} decoded as {other:?}"),
    }
}

#[test]
fn stream_directories_name_walked_landmarks() {
    let mut walked = 0;
    for (specimen, layout, streams) in sample_streams() {
        for (index, (stroke_at, stroke)) in streams.into_iter().enumerate() {
            let where_ = specimen.path.display();
            let stream = nsmp::codec::walk(stroke, stroke_at, layout)
                .unwrap_or_else(|error| panic!("{where_} stroke {index}: {error}"));
            let directory = nsmp::codec::Directory::read(stroke)
                .unwrap_or_else(|| panic!("{where_} stroke {index}: no directory"));
            let resolve = |pointer| nsmp::codec::Directory::resolve(pointer, stroke_at, layout);
            let words = (stroke.len() - layout.header_len()) / layout.word();

            assert_eq!(
                resolve(directory.first_record),
                stream.first_record,
                "{where_} stroke {index}: first record"
            );
            assert_eq!(
                nsmp::codec::Directory::resolve_end(directory.terminator, stroke_at, layout, words,),
                stream.terminator,
                "{where_} stroke {index}: terminator"
            );

            let names =
                |pointer, word| word % nsmp::codec::WRAP == resolve(pointer) % nsmp::codec::WRAP;
            assert!(
                names(directory.resync, stream.terminator)
                    || stream
                        .records
                        .iter()
                        .any(|record| names(directory.resync, record.at)),
                "{where_} stroke {index}: resync is not a record boundary"
            );
            assert!(
                names(directory.mark, stream.terminator)
                    || stream
                        .records
                        .iter()
                        .any(|record| names(directory.mark, record.at)),
                "{where_} stroke {index}: mark is not a record or terminator"
            );

            let marked = stream
                .records
                .iter()
                .filter(|record| record.mark)
                .collect::<Vec<_>>();
            assert!(
                marked.is_empty() || (marked.len() == 1 && names(directory.mark, marked[0].at)),
                "{where_} stroke {index}: marked record disagrees with directory"
            );
            assert!(!stream.records.is_empty(), "{where_} stroke {index}");
            walked += 1;
        }
    }
    assert!(walked > 0, "no stream walked");
}

#[test]
fn every_stroke_decodes_with_the_terminators_channel_count() {
    let mut decoded = 0;
    let mut stereo = 0;
    for (specimen, layout, streams) in sample_streams() {
        for (index, (stroke_at, stroke)) in streams.into_iter().enumerate() {
            let where_ = specimen.path.display();
            let stream = nsmp::codec::walk(stroke, stroke_at, layout)
                .unwrap_or_else(|error| panic!("{where_} stroke {index}: {error}"));
            let audio = nsmp::codec::decode(stroke, stroke_at, layout)
                .unwrap_or_else(|error| panic!("{where_} stroke {index}: {error}"));
            let channels = if stream.cell == Some(2 * layout.cell()) {
                2
            } else {
                1
            };
            assert_eq!(
                usize::from(audio.channels),
                channels,
                "{where_} stroke {index}"
            );
            assert_eq!(
                audio.samples.len(),
                stream
                    .records
                    .iter()
                    .map(|record| record.values.len())
                    .sum::<usize>(),
                "{where_} stroke {index}"
            );
            assert!(!audio.samples.is_empty(), "{where_} stroke {index}");
            decoded += 1;
            stereo += usize::from(channels == 2);
        }
    }
    assert!(decoded > 0, "no stroke decoded");
    assert!(stereo > 0, "no stereo decode exercised");
}

#[test]
fn same_source_generations_decode_within_one_quantiser_step() {
    let decode = |specimen: &'static Specimen, layout| {
        let (stroke_at, stroke) = match &specimen.entity {
            Entity::Sample(Sample::V2(sample)) => sample.stroke_streams()[0],
            Entity::Sample(Sample::V3(sample)) => sample.stroke_streams()[0],
            other => panic!("{}: {other:?}", specimen.path.display()),
        };
        (
            nsmp::codec::decode(stroke, stroke_at, layout)
                .unwrap_or_else(|error| panic!("{}: {error}", specimen.path.display())),
            nsmp::codec::shift(stroke, layout).unwrap(),
        )
    };
    let by_name = |name: &str| {
        corpus()
            .iter()
            .find(|specimen| specimen.path.ends_with(name))
    };

    let mut triplets = 0;
    for v3 in corpus()
        .iter()
        .filter(|specimen| specimen.path.extension().is_some_and(|ext| ext == "nsmp3"))
    {
        let stem = v3.path.file_stem().unwrap().to_string_lossy();
        let (Some(v2), Some(v4)) = (
            by_name(&format!("{stem}.nsmp")),
            by_name(&format!("{stem}.nsmp4")),
        ) else {
            continue;
        };
        triplets += 1;

        let (narrow, shift2) = decode(v2, nsmp::codec::Layout::V2);
        let (wide, shift3) = decode(v3, nsmp::codec::Layout::V3);
        let (widest, shift4) = decode(v4, nsmp::codec::Layout::V4);
        assert_eq!(narrow.samples.len(), wide.samples.len(), "{stem}: v2/v3");
        assert_eq!(narrow.samples.len(), widest.samples.len(), "{stem}: v2/v4");

        let step = |a: i32, b: i32| 4.max(1i32 << a.max(b).clamp(0, 30));
        for (other, allowed) in [
            (&wide, step(shift2, shift3)),
            (&widest, step(shift2, shift4)),
        ] {
            let worst = narrow
                .samples
                .iter()
                .zip(&other.samples)
                .map(|(&a, &b)| (i32::from(a) - i32::from(b)).abs())
                .max()
                .unwrap_or(0);
            assert!(
                worst <= allowed,
                "{stem}: difference {worst}, limit {allowed}"
            );
        }
    }
    assert!(triplets > 0, "no v2/v3/v4 triplet");
}

#[test]
fn known_sines_decode_to_their_recorded_pitch() {
    for name in ["A-sine-C4.nsmp", "F-sine-1s-C4.nsmp"] {
        let sample = v2_named(name);
        let (stroke_at, stroke) = sample.stroke_streams()[0];
        let audio = nsmp::codec::decode(stroke, stroke_at, nsmp::codec::Layout::V2).unwrap();
        let want = 440.0 * 2f64.powf((60.0 - 69.0) / 12.0);
        let window = audio
            .samples
            .iter()
            .skip(audio.samples.len() / 4)
            .take(2048)
            .map(|&sample| f64::from(sample))
            .collect::<Vec<_>>();
        let power = |hz: f64| {
            let radians = std::f64::consts::TAU * hz / f64::from(nsmp::codec::FIELD_RATE);
            let (mut real, mut imaginary) = (0.0, 0.0);
            for (index, &sample) in window.iter().enumerate() {
                real += sample * (radians * index as f64).cos();
                imaginary -= sample * (radians * index as f64).sin();
            }
            real * real + imaginary * imaginary
        };
        let cent = 2f64.powf(1.0 / 1200.0);
        let best = (-100..=100)
            .map(|offset| want * cent.powi(offset))
            .max_by(|a, b| power(*a).total_cmp(&power(*b)))
            .unwrap();
        let error = 1200.0 * (best / want).log2();
        assert!(error.abs() < 10.0, "{name}: {error:.1} cents");
    }
}

#[test]
fn non_overridden_zones_match_the_editors_default_layout() {
    let mut checked = 0;
    for (specimen, sample) in v2_samples() {
        let sidecar = sidecar::sidecar_of(&specimen.path);
        if !sidecar.exists() {
            continue;
        }
        let overridden = sidecar::load(&sidecar, sidecar::SPECIMEN_KEYS)
            .unwrap_or_else(|error| panic!("{}: {error}", specimen.path.display()))
            .get("traits")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|traits| {
                traits
                    .iter()
                    .any(|trait_| trait_ == "zone_top_notes_overridden")
            });
        if overridden {
            continue;
        }

        let roots = sample
            .strokes()
            .unwrap()
            .iter()
            .map(|stroke| stroke.root_key)
            .collect::<Vec<_>>();
        let stored = sample
            .zones()
            .unwrap()
            .iter()
            .map(|zone| zone.top_note)
            .collect::<Vec<_>>();
        assert_eq!(
            stored,
            nsmp::zone::derive_top_notes(&roots),
            "{}: roots {roots:?}",
            specimen.path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "no sidecar-backed zone layout checked");
}

#[test]
fn silent_encode_differs_from_the_editors_specimen_only_at_reviewed_bytes() {
    let expected = &named("T-sil.nsmp").bytes;
    let actual = nsmp::encode::instrument(
        &vec![0i16; 44_100],
        &nsmp::encode::Options::new("T-sil").root_key(60),
    )
    .unwrap()
    .to_bytes()
    .unwrap();

    assert_eq!(actual.len(), expected.len());
    let differing = (0..expected.len())
        .filter(|&index| actual[index] != expected[index])
        .collect::<Vec<_>>();
    assert_eq!(differing, [0x18, 0x19, 0x1a, 0x1b, 0x410, 0x47d, 0x47e]);
}

#[test]
fn count_laws_reproduce_editor_landmarks() {
    for (name, frames) in [("T-sil.nsmp", 44_100usize), ("A-impulse-C4.nsmp", 4_410)] {
        let plan = nsmp::encode::Plan::new(frames).unwrap();
        let sample = v2_named(name);
        let (stroke_at, stroke) = sample.stroke_streams()[0];
        let stream = nsmp::codec::walk(stroke, stroke_at, nsmp::codec::Layout::V2).unwrap();

        assert_eq!(stream.fields, plan.fields, "{name}: fields");
        let warmup = stream
            .records
            .iter()
            .take_while(|record| record.one_to_one)
            .map(|record| record.values.len())
            .sum::<usize>();
        assert_eq!(warmup, plan.warmup, "{name}: warmup");

        let resync = stream
            .records
            .iter()
            .skip_while(|record| record.one_to_one)
            .find(|record| record.one_to_one)
            .unwrap();
        assert_eq!(resync.first_field, plan.resync_at, "{name}: resync field");
        let resync_fields = stream
            .records
            .iter()
            .filter(|record| record.one_to_one && record.first_field >= plan.resync_at)
            .map(|record| record.values.len())
            .sum::<usize>();
        assert_eq!(resync_fields, plan.resync, "{name}: resync length");
    }
}
