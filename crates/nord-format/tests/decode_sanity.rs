#![cfg(feature = "corpus")]
//! Corpus-backed checks that one-file-at-a-time trials cannot express: cross-file
//! invariants over factory content that carries no oracle, and behavior pins
//! against files the vendor's own editor wrote.
//!
//! The per-specimen sweep is `tests/corpus`; this file holds what it can't say.
//! The corpus is read and parsed once, by `scan::corpus()`, and every test
//! here picks the entities it wants by type — never by where they sit.
//!
//! ```sh
//! NORD_CORPUS_ROOT=/path/to/nord-corpus \
//!   cargo test -p nord-format --features corpus --test decode_sanity
//! ```

use nord_format::formats::nsmp;
use nord_format::{Entity, Live, Program, Sample};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

#[path = "support/format_table.rs"]
mod format_table;
#[path = "support/scan.rs"]
mod scan;
#[path = "support/sidecar.rs"]
mod sidecar;

use format_table::formats;
use scan::{corpus, named, Specimen};

/// Every specimen that is a CBIN container.
fn cbins() -> impl Iterator<Item = &'static Specimen> {
    corpus().iter().filter(|s| s.bytes.starts_with(b"CBIN"))
}

/// Every Electro 5 program, with its bytes.
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

/// Every Electro 5 live slot, with its bytes.
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

/// Every `.nsmp` (v2) sample instrument, with its bytes.
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

/// Every sample instrument's stroke streams, with the codec layout its generation
/// implies — `.nsmp`, `.nsmp3` and `.nsmp4` alike. One codec reads all three, so
/// the stream tests below make no distinction beyond this.
fn sample_streams() -> impl Iterator<
    Item = (
        &'static Specimen,
        nsmp::codec::Layout,
        Vec<(usize, &'static [u8])>,
    ),
> {
    corpus().iter().filter_map(|s| match &s.entity {
        Entity::Sample(Sample::V2(v)) => Some((s, nsmp::codec::Layout::V2, v.stroke_streams())),
        Entity::Sample(Sample::V3(v)) => Some((s, nsmp::codec::Layout::V3, v.stroke_streams())),
        _ => None,
    })
}

/// A fresh, owned parse of the one specimen with this file name — for the
/// tests that edit one.
fn v2_named(name: &str) -> nord_format::cbin::Cbin<nsmp::Sample> {
    match nord_format::from_stream(&mut Cursor::new(&named(name).bytes)).unwrap() {
        Entity::Sample(Sample::V2(s)) => s,
        other => panic!("{name} decoded as {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Container-level facts across every model
// ---------------------------------------------------------------------------

/// The header's `aux` word holds one of three shapes everywhere: `0xFFFFFFFF`
/// (no category), a low u16 under a zero high u16 (the program category id), or
/// — on the preset/library tags alone — both halves set. A fourth shape, or a
/// both-halves value on a program tag, means the word carries something the
/// container docs don't model.
#[test]
fn aux_matches_one_of_the_three_documented_shapes() {
    // The tags observed holding both u16 halves.
    const BOTH_HALVES: &[&str] = &["ns3y", "nsmp", "nd2p"];

    let mut failures: Vec<String> = Vec::new();
    let mut seen = 0usize;
    for s in cbins() {
        let tag = String::from_utf8_lossy(&s.bytes[8..12]).replace('\0', "");
        let aux = u32::from_le_bytes(s.bytes[0x10..0x14].try_into().unwrap());
        let ok = aux == 0xFFFF_FFFF || (aux >> 16) == 0 || BOTH_HALVES.contains(&tag.as_str());
        if !ok {
            failures.push(format!("{}: {tag} aux {aux:#010x}", s.path.display()));
        }
        seen += 1;
    }
    assert!(seen > 0, "no CBIN specimen");
    assert!(
        failures.is_empty(),
        "{} specimens hold an undocumented aux shape:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The stub modules' observed body lengths hold across every specimen.
#[test]
fn observed_body_lengths_match_the_documented_constants() {
    let expected: BTreeMap<&str, u64> = formats()
        .into_iter()
        .map(|(tag, len, _)| (tag, len))
        .collect();

    let mut checked = 0usize;
    for s in cbins() {
        let info = nord_format::cbin::inspect(&mut Cursor::new(&s.bytes)).unwrap();
        let tag = String::from_utf8_lossy(&info.header.tag).into_owned();
        if let Some(&want) = expected.get(tag.as_str()) {
            assert_eq!(
                info.body_len,
                want,
                "{}: {tag} body is {} bytes where the module declares {want}",
                s.path.display(),
                info.body_len,
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no stub-format specimen");
}

// ---------------------------------------------------------------------------
// Stage decode sanity — factory content has no oracle, so distribution
// sanity is the strongest available check that the placements are right
// ---------------------------------------------------------------------------

/// The Stage 2/3 globals decode reads values the panel could actually show,
/// across every factory program of both models.
#[test]
fn stage_globals_decode_to_panel_values() {
    let mut ns2_seen = 0usize;
    let mut ns3_seen = 0usize;
    let mut ns3_split_on = 0usize;
    let mut ns3_at_default_clock = 0usize;

    for s in corpus() {
        match &s.entity {
            Entity::Program(Program::Stage2(_)) | Entity::Live(Live::Stage2(_)) => ns2_seen += 1,
            Entity::Program(Program::Stage3(p)) | Entity::Live(Live::Stage3(p)) => {
                ns3_split_on += usize::from(p.split_enabled);
                ns3_at_default_clock += usize::from(p.master_clock.bpm() == 120);
                ns3_seen += 1;
            }
            _ => {}
        }
    }

    assert!(ns2_seen > 0, "no Stage 2 program");
    assert!(ns3_seen > 0, "no Stage 3 program");
    // A decode where no factory program ever splits is reading the wrong bits,
    // and one where the master clock is not overwhelmingly at its 120 bpm
    // default is reading the wrong bits shifted — either failure moves these.
    assert!(ns3_split_on > 0, "no ns3f decodes with a split enabled");
    assert!(
        ns3_at_default_clock * 2 > ns3_seen,
        "only {ns3_at_default_clock}/{ns3_seen} programs read the 120 bpm default"
    );
}

/// The Stage 4 decode, whose placements came from an external offset table and
/// no hardware. Two independent checks that the table was read into the right
/// bit space: the body echoes the header's version byte at its own offset 3, and
/// the selector fields — each a fixed-width slot holding a short list of panel
/// choices — never hold a value past the end of that list. A base offset off by
/// a byte, or bits numbered the other way round, breaks both at once.
#[test]
fn stage4_bodies_decode_to_panel_values() {
    use nord_format::{OrganPreset, PianoPreset, Synth};

    let (mut programs, mut organs, mut pianos, mut synths) = (0usize, 0usize, 0usize, 0usize);
    let mut split_on = 0usize;

    // A selector's slot is wider than the choices the panel offers, so the
    // unused encodings are the check: they must never appear. The octave shift
    // reads through `OctaveShiftNibble`, so the two's-complement wrap that puts
    // -1 at a stored 15 is the type's business rather than this test's.
    let octave_shift = |v: i8| (-2..=2).contains(&v);

    for s in corpus() {
        let where_ = s.path.display();
        match &s.entity {
            Entity::Program(Program::Stage4(p)) | Entity::Live(Live::Stage4(p)) => {
                assert_eq!(
                    p.version_echo as u32,
                    p.header.version & 0xff,
                    "{where_}: the body's version echo disagrees with the header"
                );
                assert!(
                    p.organ_section_enabled || p.piano_section_enabled || p.synth_section_enabled,
                    "{where_}: no section is routed to the keyboard"
                );
                assert!(p.organ_a.model.raw() <= 5, "{where_}");
                assert!(p.organ_b.model.raw() <= 5, "{where_}");
                assert!(p.piano_a.piano_type.raw() <= 5, "{where_}");
                assert!(p.piano_b.piano_type.raw() <= 5, "{where_}");
                assert!(p.synth_a_voice.filter_type.raw() <= 5, "{where_}");
                assert!(p.synth_a_voice.lfo_shape.raw() <= 4, "{where_}");
                assert!(p.synth_a_performance.voice_priority.raw() <= 2, "{where_}");
                assert!(p.organ_fx.reverb_type.raw() <= 11, "{where_}");
                assert!(octave_shift(p.organ_a.octave_shift.octaves()), "{where_}");
                split_on += usize::from(p.split_enabled);
                programs += 1;
            }
            Entity::OrganPreset(OrganPreset::Stage4(o)) => {
                assert!(o.organ_a_model.raw() <= 5, "{where_}");
                assert!(o.organ_b_model.raw() <= 5, "{where_}");
                assert!(o.organ_fx.reverb_type.raw() <= 11, "{where_}");
                assert!(octave_shift(o.organ_a_octave_shift.octaves()), "{where_}");
                organs += 1;
            }
            Entity::PianoPreset(PianoPreset::Stage4(n)) => {
                assert!(n.piano_a_type.raw() <= 5, "{where_}");
                assert!(n.piano_b_type.raw() <= 5, "{where_}");
                assert!(n.piano_a_fx.reverb_type.raw() <= 11, "{where_}");
                assert!(octave_shift(n.piano_a_octave_shift.octaves()), "{where_}");
                pianos += 1;
            }
            Entity::Synth(Synth::Stage4(y)) => {
                assert!(y.synth_a_voice.filter_type.raw() <= 5, "{where_}");
                assert!(y.synth_b_voice.filter_type.raw() <= 5, "{where_}");
                assert!(y.synth_a_voice.lfo_shape.raw() <= 4, "{where_}");
                assert!(y.synth_a_voice_priority.raw() <= 2, "{where_}");
                assert!(y.synth_a_fx.reverb_type.raw() <= 11, "{where_}");
                assert!(octave_shift(y.synth_a_octave_shift.octaves()), "{where_}");
                synths += 1;
            }
            _ => {}
        }
    }

    assert!(programs > 0, "no Stage 4 program");
    assert!(organs > 0, "no Stage 4 organ preset");
    assert!(pianos > 0, "no Stage 4 piano preset");
    assert!(synths > 0, "no Stage 4 synth preset");
    // A decode where no factory program ever splits is reading the wrong bits.
    assert!(split_on > 0, "no Stage 4 program reads a split");
}

/// Every nsmp3/nsmp4 specimen decodes as the wide section chain, with a name
/// and at least one stroke — in both container generations — and the zone maps
/// that pair with their strokes check out zone by zone.
#[test]
fn v3_samples_decode_names_and_strokes() {
    let mut paired = 0usize;
    let mut unpaired = 0usize;
    for s in corpus() {
        let Entity::Sample(Sample::V3(v)) = &s.entity else {
            continue;
        };
        let where_ = s.path.display();
        let name = v.name().unwrap();
        assert!(!name.is_empty(), "{where_}: empty name");
        assert!(v.stroke_count() > 0, "{where_}: no strokes");
        match v.zones() {
            // Unexplained: a large share of the vendor sample pool carries a
            // zone map holding roughly one entry per keyboard key (108, 107,
            // 96 entries) — or none at all — rather than one per stroke. The
            // reader refuses to pair those, and this test accepts the refusal;
            // a decode of the wide map would turn these back into assertions.
            Err(_) => unpaired += 1,
            Ok(zones) => {
                assert_eq!(zones.len(), v.stroke_count(), "{where_}");
                for z in &zones {
                    assert!(z.top_note <= 127 && z.root_key <= 127, "{where_}");
                    if let Some(low) = z.low_note {
                        assert!(low <= z.top_note, "{where_}: low above top");
                    }
                }
                paired += 1;
            }
        }
    }
    // A reader change that stops pairing what used to pair turns paired files
    // into unpaired ones, which moves both of these.
    assert!(paired > 0, "no v3 zone map paired with its strokes");
    assert!(
        paired > unpaired,
        "{unpaired} of {} v3 zone maps failed to pair with their strokes",
        paired + unpaired
    );
}

/// The drum banks: every member of every bank parses and the counts match the
/// devices' bank sizes.
#[test]
fn drum_banks_walk_to_their_members() {
    use nord_format::Bundle;

    let mut banks = 0usize;
    for s in corpus() {
        match &s.entity {
            Entity::Bundle(Bundle::Drum2Bank(b)) => {
                assert_eq!(b.programs.len(), 50, "{}", s.path.display())
            }
            Entity::Bundle(Bundle::Drum3KitBank(b)) => {
                assert_eq!(b.kits.len(), 50, "{}", s.path.display())
            }
            _ => continue,
        }
        banks += 1;
    }
    assert!(banks > 0, "no drum bank");
}

// ---------------------------------------------------------------------------
// Electro 5: the live/program equivalence
// ---------------------------------------------------------------------------

/// The three live slots are `0:0`, `0:1` and `0:2` on the wire, and nothing else.
#[test]
fn ne5_live_occupies_three_slots_of_one_bank() {
    use nord_format::bank::Item;

    let mut seen: BTreeSet<(u16, u16)> = BTreeSet::new();
    for (s, live) in ne5_lives() {
        let at = live.location();
        assert_eq!(
            at.x(),
            0,
            "live slot outside bank 0 in {}",
            s.path.display()
        );
        seen.insert(at.inner());
    }

    assert_eq!(
        seen,
        BTreeSet::from([(0, 0), (0, 1), (0, 2)]),
        "the corpus no longer covers all three live slots",
    );
}

/// A live specimen with its tag swapped is a valid program, down to the last field.
///
/// Confirmed on hardware: the same panel state read as a live slot and as a program gives
/// byte-identical bodies. One body serves both, so the field values cannot disagree —
/// what this pins on real specimens is that everything around them agrees too: the slot
/// falls in the program space, the version is one programs accept, and the body checksum
/// still holds.
#[test]
fn ne5_a_live_body_decodes_as_a_program() {
    use nord_format::formats::ne5;

    let mut seen = 0usize;
    for (s, live) in ne5_lives() {
        let name = s.path.display();

        // The tag is the whole difference. On a type-1 file the crc32 covers the body
        // and never sees the header, so the retag alone leaves a valid file; a type-0
        // file's trailing crc16 covers the header too, so it gets restamped.
        let mut bytes = s.bytes.clone();
        bytes[0x08..0x0c].copy_from_slice(ne5::program::FORMAT.as_bytes());
        if bytes[0x04] == 0 {
            let at = bytes.len() - 2;
            let crc = nord_format::crc::crc16(&bytes[..at]);
            bytes[at..].copy_from_slice(&crc.to_le_bytes());
        }

        let Entity::Program(Program::Electro5(program)) =
            nord_format::from_stream(&mut Cursor::new(&bytes)).unwrap()
        else {
            panic!("a retagged live slot did not decode as a program: {name}")
        };

        let named = |fields: Vec<nord_format::fields::Field>| -> Vec<(String, String)> {
            fields.into_iter().map(|f| (f.path, f.display)).collect()
        };
        let fields = named(live.fields());
        assert!(!fields.is_empty(), "no fields to compare");
        assert_eq!(
            fields,
            named(program.fields()),
            "live and program decodes disagree on {name}",
        );
        seen += 1;
    }
    assert!(seen > 0, "no Electro 5 live slot");
}

/// Reading nine drawbar nibbles and writing them back is a no-op, on every
/// program the corpus holds. The organ accessors sit outside the field
/// registry, so the sweep's registry mutation cannot cover them — this is
/// their read/write inverse proof, against real data.
#[test]
fn ne5_organ_drawbars_survive_a_rewrite() {
    use nord_format::formats::ne5::OrganModel::*;

    let mut checked = 0usize;
    for (s, _) in ne5_programs() {
        let Entity::Program(Program::Electro5(mut program)) =
            nord_format::from_stream(&mut Cursor::new(&s.bytes)).unwrap()
        else {
            unreachable!()
        };
        let organ = &mut program.organ_panel;
        for model in [B3, Vox, Farfisa, Pipe] {
            for preset in [1u8, 2] {
                let bars = organ.drawbars(model, preset);
                if bars.iter().any(|&b| b > 8) {
                    continue;
                }
                organ.set_drawbars(model, preset, bars).unwrap();
            }
        }

        let mut rewritten: Vec<u8> = Vec::new();
        program.write_to(&mut Cursor::new(&mut rewritten)).unwrap();
        assert_eq!(
            s.bytes,
            rewritten,
            "rewriting the drawbars changed {}",
            s.path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "no Electro 5 program");
}

// ---------------------------------------------------------------------------
// Sample instruments (`.nsmp`): behavior pins against the editor's own files
// ---------------------------------------------------------------------------

/// Every stroke decomposes into its header plus whole packets.
///
/// A wrong header rule shows up here as a leftover remainder, on every specimen at once.
/// A content version whose zone table the reader declines to pair (it says so in its
/// error) is read for its strokes alone.
#[test]
fn nsmp_strokes_decompose() {
    let mut seen = 0;
    for (s, sample) in v2_samples() {
        let where_ = s.path.display();
        let Ok(zones) = sample.zones() else {
            continue;
        };
        let strokes = sample.strokes().unwrap_or_else(|e| panic!("{where_}: {e}"));
        assert_eq!(
            strokes.len(),
            zones.len(),
            "{where_}: {} zones but {} strokes",
            zones.len(),
            strokes.len()
        );
        seen += strokes.len();
    }
    assert!(seen > 0, "no stroke walked");
}

/// Every stroke's encoded audio walks end to end, and the walk lands exactly
/// where the stroke header's own word directory says it should — in every
/// generation.
///
/// The directory is written by the encoder and read by nothing else here, so
/// agreement between it and an independent walk is a real check on both. A grammar
/// that framed any record at the wrong size would arrive somewhere else.
#[test]
fn nsmp_streams_walk_to_the_terminator_the_header_names() {
    let mut walked = 0;
    for (s, layout, streams) in sample_streams() {
        let where_ = s.path.display();
        for (index, (at, stroke)) in streams.into_iter().enumerate() {
            let stream = nsmp::codec::walk(stroke, at, layout)
                .unwrap_or_else(|e| panic!("{where_} stroke {index}: {e}"));
            let directory = nsmp::codec::Directory::read(stroke)
                .unwrap_or_else(|| panic!("{where_} stroke {index}: no word directory"));
            let resolve = |p: u16| nsmp::codec::Directory::resolve(p, at, layout);
            assert_eq!(
                resolve(directory.first_record),
                stream.first_record,
                "{where_} stroke {index}: the chain starts somewhere the header does not name"
            );
            assert_eq!(
                resolve(directory.terminator),
                stream.terminator,
                "{where_} stroke {index}: the chain ends somewhere the header does not name"
            );
            // The resync pointer is the one the walk does not consume, so it is an
            // independent check that the record boundaries fell where they should.
            let resync = resolve(directory.resync);
            assert!(
                stream.records.iter().any(|r| r.at == resync),
                "{where_} stroke {index}: the resync pointer is not a record boundary"
            );
            // So is the pointer that names the marked record: reading it as a second
            // copy of the terminator instead stops the walk partway through every
            // vendor stroke. A stroke marks at most one record, and where it marks
            // one this is where it says to look. The converse does not hold — the
            // pointer names a record on unmarked strokes too.
            let named = resolve(directory.mark);
            assert!(
                named == stream.terminator || stream.records.iter().any(|r| r.at == named),
                "{where_} stroke {index}: the mark pointer is neither a record nor the end"
            );
            let marked: Vec<usize> = stream
                .records
                .iter()
                .filter(|r| r.mark)
                .map(|r| r.at)
                .collect();
            assert!(
                marked.is_empty() || marked == [named],
                "{where_} stroke {index}: marked {marked:?} but the header names {named}"
            );
            assert!(
                !stream.records.is_empty(),
                "{where_} stroke {index}: empty chain"
            );
            walked += 1;
        }
    }
    assert!(walked > 0, "no stream walked");
}

/// Every stroke decodes to audio in every generation, and the lattice accounting
/// is consistent: the chain's field counts sum to the sample length, whatever each
/// record's differencing order.
///
/// A stereo stroke is the one refusal — two streams share its header, which the
/// terminator gives away by cellwise doubling the layout's own size — and it says so
/// by name rather than interleaving them.
#[test]
fn nsmp_every_stroke_decodes() {
    let mut decoded = 0;
    let mut stereo = 0;
    for (s, layout, streams) in sample_streams() {
        let where_ = s.path.display();
        for (index, (at, stroke)) in streams.into_iter().enumerate() {
            let stream = nsmp::codec::walk(stroke, at, layout).unwrap();
            let audio = match nsmp::codec::decode(stroke, at, layout) {
                Ok(audio) => audio,
                Err(nsmp::codec::Unsupported::Stereo) => {
                    assert_eq!(
                        stream.cell,
                        Some(2 * layout.cell()),
                        "{where_} stroke {index}: refused as stereo without a stereo cell"
                    );
                    stereo += 1;
                    continue;
                }
                Err(e) => panic!("{where_} stroke {index}: {e}"),
            };
            assert_eq!(
                audio.samples.len(),
                stream.records.iter().map(|r| r.values.len()).sum::<usize>(),
                "{where_} stroke {index}: decoded length is not the chain's field total"
            );
            assert!(
                audio.samples.len() > 100,
                "{where_} stroke {index}: {} fields is too short to be a sample",
                audio.samples.len()
            );
            decoded += 1;
        }
    }
    assert!(decoded > 0, "no stroke decoded");
    assert!(
        stereo < decoded / 100,
        "stereo is meant to be the rare case"
    );
}

/// The same source rendered at v2, v3 and v4 decodes to the same audio.
///
/// The three generations differ only in units — 3-byte words against 4, 24-field
/// cells against 32, a 51-byte stroke header against 68 — so a triplet is the
/// sharpest check there is on the port: one wrong constant moves the field lattice
/// and the decoded lengths stop matching, which they do not on any twin.
///
/// Sample values agree to within a quantiser step, and never exactly, for two
/// reasons that are the codec working as designed: the wide generations sometimes
/// choose a finer shift, and a cell that falls under the coder's noise floor is
/// dropped whole — so with cells of different sizes the two renders drop different
/// material at the bottom bit. The comparison is therefore made only where all
/// three decode exactly; a differenced run carries no level to compare.
#[test]
fn nsmp_a_source_rendered_at_every_generation_decodes_the_same() {
    let audio = |s: &'static Specimen, layout| {
        let (at, stroke) = match &s.entity {
            Entity::Sample(Sample::V2(v)) => v.stroke_streams()[0],
            Entity::Sample(Sample::V3(v)) => v.stroke_streams()[0],
            other => panic!("{}: {other:?} is not a sample", s.path.display()),
        };
        (
            nsmp::codec::decode(stroke, at, layout)
                .unwrap_or_else(|e| panic!("{}: {e}", s.path.display())),
            nsmp::codec::shift(stroke, layout).unwrap(),
        )
    };
    let by_name = |name: &str| corpus().iter().find(|s| s.path.ends_with(name));

    let mut triplets = 0;
    let mut compared = 0;
    for wide in corpus()
        .iter()
        .filter(|s| s.path.extension().is_some_and(|e| e == "nsmp3"))
    {
        let stem = wide
            .path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let (Some(narrow), Some(widest)) = (
            by_name(&format!("{stem}.nsmp")),
            by_name(&format!("{stem}.nsmp4")),
        ) else {
            continue;
        };
        triplets += 1;

        let (v2, s2) = audio(narrow, nsmp::codec::Layout::V2);
        let (v3, s3) = audio(wide, nsmp::codec::Layout::V3);
        let (v4, s4) = audio(widest, nsmp::codec::Layout::V3);
        assert_eq!(
            (v2.samples.len(), v2.samples.len()),
            (v3.samples.len(), v4.samples.len()),
            "{stem}: the generations decode to different lengths, so the lattice moved"
        );

        compared += 1;
        // One step of the coarser grid, and never less than the few LSB the cell
        // sizes cost on their own at the noise floor.
        let step = |a: i32, b: i32| 4.max(1 << a.max(b).max(0));
        for (other, step) in [(&v3, step(s2, s3)), (&v4, step(s2, s4))] {
            let worst = v2
                .samples
                .iter()
                .zip(&other.samples)
                .map(|(&a, &b)| (i32::from(a) - i32::from(b)).abs())
                .max()
                .unwrap_or(0);
            assert!(
                worst <= step,
                "{stem}: the generations disagree by {worst}, past the {step} a \
                 quantiser step accounts for"
            );
        }
    }
    assert!(triplets > 0, "no v2/v3/v4 triplet");
    assert_eq!(compared, triplets, "every triplet is compared");
}

/// A sine specimen decodes to that sine: the editor was handed a C4 tone, and the
/// decoded audio's strongest partial lands on it within a few cents.
///
/// This is the one end-to-end check on the whole chain — lattice pitch, the header
/// shift, and the differencing order — against material whose source is known.
#[test]
fn nsmp_a_known_sine_decodes_to_its_own_pitch() {
    for name in ["A-sine-C4.nsmp", "F-sine-1s-C4.nsmp"] {
        let sample = v2_named(name);
        let (at, stroke) = sample.stroke_streams()[0];
        let audio = nsmp::codec::decode(stroke, at, nsmp::codec::Layout::V2).unwrap();
        let rate = f64::from(nsmp::codec::FIELD_RATE);
        // Middle C, which is what the specimen's file name says was recorded.
        let want = 440.0 * 2f64.powf((60.0 - 69.0) / 12.0);
        let window: Vec<f64> = audio
            .samples
            .iter()
            .skip(audio.samples.len() / 4)
            .take(2048)
            .map(|&v| f64::from(v))
            .collect();
        let power = |hz: f64| -> f64 {
            let w = std::f64::consts::TAU * hz / rate;
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &v) in window.iter().enumerate() {
                re += v * (w * i as f64).cos();
                im -= v * (w * i as f64).sin();
            }
            re * re + im * im
        };
        // Scan a semitone either side; the peak must be the tone, not a neighbour.
        let step = 2f64.powf(1.0 / 1200.0);
        let best = (-100..=100)
            .map(|c| want * step.powi(c))
            .max_by(|a, b| power(*a).total_cmp(&power(*b)))
            .unwrap();
        let cents = 1200.0 * (best / want).log2();
        assert!(
            cents.abs() < 10.0,
            "{name}: strongest partial is {best:.2} Hz, {cents:.1} cents off {want:.2}"
        );
    }
}

/// On every sample with a sidecar, the zone key ranges match what the editor lays
/// out from the root keys — unless the sidecar's `zone_top_notes_overridden` trait
/// says they were moved by hand, which the per-specimen sweep asserts from the
/// other side. A sample without a sidecar makes no claim about its layout.
///
/// So the derivation is the editor's default, not a rule the format enforces: the top
/// note really is stored, and a reader must take it as read rather than recompute it.
#[test]
fn nsmp_zone_ranges_are_the_editors_default_unless_overridden() {
    let mut checked = 0;
    for (s, sample) in v2_samples() {
        let where_ = s.path.display();
        let sidecar = sidecar::sidecar_of(&s.path);
        if !sidecar.exists() {
            continue;
        }
        let overridden = sidecar::load(&sidecar, sidecar::SPECIMEN_KEYS)
            .unwrap_or_else(|e| panic!("{where_}: {e}"))
            .get("traits")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|a| a.iter().any(|t| t == "zone_top_notes_overridden"));
        if overridden {
            continue; // the sweep's trait checker asserts the inequality
        }

        let roots: Vec<u8> = sample
            .strokes()
            .unwrap()
            .iter()
            .map(|s| s.root_key)
            .collect();
        let stored: Vec<u8> = sample.zones().unwrap().iter().map(|z| z.top_note).collect();
        let derived = nsmp::zone::derive_top_notes(&roots);
        assert_eq!(
            stored, derived,
            "{where_}: stored key ranges disagree with the ones its root keys {roots:?} imply"
        );
        checked += 1;
    }
    assert!(checked > 0, "no sample checked");
}

/// Rename and remap reproduce a specimen the editor itself wrote.
///
/// `D7-upperkey` is `D4-3zones` with the middle zone's upper key moved and a new name.
/// Making the same two edits must give back the same bytes — which also pins that a
/// remap leaves the encoded audio alone, and that the checksum is recomputed correctly.
#[test]
fn nsmp_edits_reproduce_the_editors_own_output() {
    let mut sample = v2_named("D4-3zones.nsmp");

    sample.set_name("D7-upperkey").unwrap();
    sample.set_zone_top_note(1, 60).unwrap();

    assert_eq!(
        sample.to_bytes().unwrap(),
        named("D7-upperkey.nsmp").bytes,
        "rename + remap did not reproduce the editor's own file"
    );
}

/// Retuning a zone moves the root key and nothing else but the checksum.
#[test]
fn nsmp_retune_touches_one_byte() {
    let before = &named("D1-one-zone.nsmp").bytes;
    let mut sample = v2_named("D1-one-zone.nsmp");

    sample.set_root_key(0, 48).unwrap();
    let after = sample.to_bytes().unwrap();

    let differing: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    // The checksum at 0x18..0x1c, plus the one root-key byte.
    assert_eq!(differing.len(), 5, "changed bytes: {differing:?}");
    assert!(differing[..4].iter().eq([0x18, 0x19, 0x1a, 0x1b].iter()));
    assert_eq!(sample.strokes().unwrap()[0].root_key, 48);
}

/// A name longer than the writer is willing to emit is refused, not truncated.
#[test]
fn nsmp_overlong_name_is_refused() {
    let mut sample = v2_named("D1-one-zone.nsmp");
    assert!(sample.set_name("a name that is far too long").is_err());
    assert_eq!(
        sample.name().unwrap(),
        "TEST",
        "a refused rename still changed it"
    );
}

/// A corrupted body is refused on read rather than decoded.
#[test]
fn nsmp_bad_checksum_is_refused() {
    let mut bytes = named("D1-one-zone.nsmp").bytes.clone();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    assert!(nord_format::from_stream(&mut Cursor::new(&bytes)).is_err());
}

/// The structural emitter against the editor's own output.
///
/// `T-sil.nsmp` is one second of silence as Nord Sample Editor wrote it. Encoding the
/// same second reproduces it byte for byte apart from four bytes of checksum and the
/// content the editor's source carried that a silent WAV does not: its stream opens
/// with a ±1 marker in the warmup's first payload word, which also sets the content
/// peak in the stroke header. Everything else — section chain, section versions,
/// preamble sizes, the allocation, the count laws' landmarks, the record headers, the
/// word directory — comes out identical.
#[test]
fn nsmp_a_silent_second_reproduces_the_editors_own_file() {
    let want = &named("T-sil.nsmp").bytes;
    let options = nsmp::encode::Options::new("T-sil").root_key(60);
    let got = nsmp::encode::instrument(&vec![0i16; 44_100], &options)
        .unwrap()
        .to_bytes()
        .unwrap();

    assert_eq!(got.len(), want.len());
    let differing: Vec<usize> = (0..want.len()).filter(|&i| got[i] != want[i]).collect();
    // 0x18..0x1c is the checksum; 0x410 is the content peak in the stroke header, and
    // 0x47d..0x47f the marker's own word.
    assert_eq!(
        differing,
        vec![0x18, 0x19, 0x1a, 0x1b, 0x410, 0x47d, 0x47e],
        "bytes that differ from the editor's file"
    );
}

/// The count laws generate the landmarks the editor put in its own files: the field
/// total, the resync position, and the two 1:1 run lengths around it.
#[test]
fn nsmp_the_count_laws_reproduce_the_editors_landmarks() {
    // (specimen, source frames) — the frame counts the generator wrote them from.
    for (name, frames) in [("T-sil.nsmp", 44_100usize), ("A-impulse-C4.nsmp", 4_410)] {
        let plan = nsmp::encode::Plan::new(frames).unwrap();
        let sample = v2_named(name);
        let (at, stroke) = sample.stroke_streams()[0];
        let stream = nsmp::codec::walk(stroke, at, nsmp::codec::Layout::V2).unwrap();

        assert_eq!(stream.fields, plan.fields, "{name}: total fields");
        let warmup: usize = stream
            .records
            .iter()
            .take_while(|r| r.one_to_one)
            .map(|r| r.values.len())
            .sum();
        assert_eq!(warmup, plan.warmup, "{name}: warmup fields");
        let resync = stream
            .records
            .iter()
            .skip_while(|r| r.one_to_one)
            .find(|r| r.one_to_one)
            .unwrap();
        assert_eq!(resync.first_field, plan.resync_at, "{name}: resync field");
        let resync_fields: usize = stream
            .records
            .iter()
            .filter(|r| r.one_to_one && r.first_field >= plan.resync_at)
            .map(|r| r.values.len())
            .sum();
        assert_eq!(resync_fields, plan.resync, "{name}: resync fields");
    }
}
