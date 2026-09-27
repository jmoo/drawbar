//! Writes the fixtures that come from this crate's encoders and carriers:
//! `nsmp/`, `npno/`, `zip/`, `sysex/`, `midi/` and `cn3/` under `tests/fixtures`.
//!
//! ```sh
//! cargo test -p nord-format --features bundle --test generate_fixtures -- --ignored
//! ```
#![cfg(feature = "bundle")]

use nord_format::formats::npno::encode::{self as piano, Donor, Kind, Recording, Rules};
use nord_format::formats::npno::Bank;
use nord_format::formats::nsmp::codec::Layout;
use nord_format::formats::nsmp::encode as sample;
use nord_format::formats::{cn3, midi, sysex};
use nord_format::Entity;
use std::fs;
use std::io::{Cursor, Write};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn write(name: &str, bytes: &[u8]) {
    let path = root().join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn read(name: &str) -> Vec<u8> {
    fs::read(root().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// A triangle wave in integer arithmetic, so the input is the same on every platform.
fn tone(frames: usize) -> Vec<i16> {
    const PERIOD: usize = 100;
    const PEAK: i32 = 8_000;
    (0..frames)
        .map(|i| {
            let phase = (i % PERIOD) as i32;
            let half = PERIOD as i32 / 2;
            let rising = if phase < half {
                phase
            } else {
                PERIOD as i32 - phase
            };
            (rising * 4 * PEAK / PERIOD as i32 - PEAK) as i16
        })
        .collect()
}

fn sample_bytes(layout: Layout) -> Vec<u8> {
    let options = sample::Options::new("Tone").root_key(60).layout(layout);
    sample::instrument(&tone(2_048), &options)
        .unwrap()
        .to_bytes()
        .unwrap()
}

fn piano_bytes() -> Vec<u8> {
    let recording = Recording {
        root: 60,
        bank: Bank::Attack,
        layer: 0,
        channels: vec![tone(1_024)],
    };
    let library = piano::build(
        &Donor::Rules(Rules::new(Kind::Grand)),
        &piano::Options::new("Tone"),
        &[recording],
    )
    .unwrap();
    nord_format::to_bytes(&Entity::Piano(library.to_piano().unwrap())).unwrap()
}

/// A stored archive with a fixed timestamp, so the bytes depend only on the members.
fn archive(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    for (name, bytes) in members {
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn carried(
    write_to: impl FnOnce(&mut Vec<u8>) -> Result<(), nord_format::error::Error>,
) -> Vec<u8> {
    let mut out = Vec::new();
    write_to(&mut out).unwrap();
    out
}

/// The MIDI 1.0 Universal Non-Real-Time Identity Request, a public standard message
/// that names no manufacturer.
const IDENTITY_REQUEST: [u8; 6] = [0xf0, 0x7e, 0x7f, 0x06, 0x01, 0xf7];

/// A format 0 Standard MIDI File with one track: the identity request at tick 0, then
/// end of track.
fn standard_midi_file() -> Vec<u8> {
    let mut track = vec![0x00, 0xf0, IDENTITY_REQUEST.len() as u8 - 1];
    track.extend_from_slice(&IDENTITY_REQUEST[1..]);
    track.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let mut file = midi::MAGIC.to_vec();
    file.extend_from_slice(&[0, 0, 0, 6, 0, 0, 0, 1, 0, 96]);
    file.extend_from_slice(b"MTrk");
    file.extend_from_slice(&(track.len() as u32).to_be_bytes());
    file.extend_from_slice(&track);
    file
}

/// How many leading bytes `util::peek` reads to tell CBIN from CNE3. A shorter file
/// is not recognized.
const SNIFFED: usize = 12;

#[test]
#[ignore = "writes tests/fixtures; run to regenerate"]
fn write_encoded_and_carried_fixtures() {
    write("nsmp/tone.nsmp3", &sample_bytes(Layout::V3));
    write("npno/tone.npno", &piano_bytes());

    write(
        "zip/electro5-bundle.zip",
        &archive(&[
            ("Bank A/default.ne5p", read("ne5/default.ne5p")),
            ("song.ne5t", read("ne5/song.ne5t")),
            ("tone.nsmp", sample_bytes(Layout::V2)),
            ("tone.npno", piano_bytes()),
        ]),
    );
    write(
        "zip/drum2-bank.zip",
        &archive(&[
            ("1.nd2p", read("cbin/nd2p.g0.cbin")),
            ("2.nd2p", read("cbin/nd2p.g1.cbin")),
        ]),
    );
    write(
        "zip/drum3-bank.zip",
        &archive(&[
            ("1.nd3k", read("cbin/nd3k.g0.cbin")),
            ("2.nd3k", read("cbin/nd3k.g1.cbin")),
        ]),
    );
    write(
        "zip/members.zip",
        &archive(&[
            ("program.ne6p", read("cbin/ne6p.g0.cbin")),
            ("settings.ne6t", read("cbin/ne6t.g1.cbin")),
        ]),
    );

    let dump = sysex::Sysex {
        data: IDENTITY_REQUEST.to_vec(),
    };
    write(
        "sysex/identity-request.syx",
        &carried(|out| dump.write_to(out)),
    );
    let file = midi::Midi {
        data: standard_midi_file(),
    };
    write(
        "midi/identity-request.mid",
        &carried(|out| file.write_to(out)),
    );
    let mut data = cn3::MAGIC.to_vec();
    data.resize(SNIFFED, 0);
    let library = cn3::Cne3 { data };
    write("cn3/magic.cn3", &carried(|out| library.write_to(out)));
}
