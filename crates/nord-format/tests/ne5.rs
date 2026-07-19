#![cfg(feature = "corpus")]
//! Corpus-backed decode + byte-exact round-trip tests for the Electro 5 formats.
//!
//! Gated behind the `corpus` cargo feature: these need the specimen corpus,
//! which lives in the private `jmoo/nord-corpus` repo (it grows to hold
//! proprietary piano/sample data). Without the feature the whole file compiles
//! out, so the default `cargo test` runs only the open minimal suite. The Nix
//! `nord-format-corpus` check enables the feature and sets `NORD_CORPUS_DIR`.
//!
//! ```sh
//! NORD_CORPUS_DIR=/path/to/nord-corpus/ne5 cargo test -p nord-format --features corpus
//! ```
//!
//! TODO: migrate these hand-rolled `read_dir` loops to a data-driven harness
//! (`datatest-stable` / `libtest-mimic`) so each specimen is its own reported
//! test case — see `Projects/Nord Utils.md`.

use nord_format::common::bank::Item;
use nord_format::electro5::{Instrument, SplitPoint};
use nord_format::{electro5, Entity};
use regex::Regex;
use std::fs;
use std::fs::read;
use std::io::Cursor;
use std::path::PathBuf;
use std::str::FromStr;

/// Root of the Electro 5 specimen corpus, taken from `NORD_CORPUS_DIR` (point it
/// at a `nord-corpus/ne5` checkout). Since these tests only compile under the
/// `corpus` feature, a missing `NORD_CORPUS_DIR` is a hard error, not a skip.
fn corpus_dir() -> PathBuf {
    std::env::var_os("NORD_CORPUS_DIR")
        .map(PathBuf::from)
        .expect("set NORD_CORPUS_DIR to a nord-corpus/ne5 checkout for --features corpus")
}

#[test]
fn test_ne5_read_song_macro() {
    let test_file = corpus_dir().join("song.ne5t");

    let song = nord_format::from_path(&test_file).unwrap();

    match song {
        Entity::Song(nord_format::Song::Electro5(song)) => {
            let song = song as electro5::Song;
            let coords = song.location();

            assert_eq!(coords, (0, 2));
            assert_eq!(song.get(0), (5, 9));
            assert_eq!(song.get(1), (0, 1));
            assert_eq!(song.get(2), (0, 2));
            assert_eq!(song.get(3), (5, 8));
        }
        _ => panic!("expected electro5 song"),
    }
}

#[test]
fn test_ne5_read_song_bank() {
    let test_file = corpus_dir().join("song.ne5t");

    let song = nord_format::from_path(&test_file).unwrap();

    match song {
        Entity::Song(nord_format::Song::Electro5(song)) => {
            let song = song as electro5::Song;
            let coords = song.location();

            assert_eq!(coords, (0, 2));
        }
        _ => panic!("expected electro5 song"),
    }
}

#[test]
fn test_ne5_read_song_programs() {
    let test_file = corpus_dir().join("song.ne5t");

    let song = nord_format::from_path(&test_file).unwrap();

    match song {
        Entity::Song(nord_format::Song::Electro5(song)) => {
            assert_eq!(song.get(0), (5, 9));
            assert_eq!(song.get(1), (0, 1));
            assert_eq!(song.get(2), (0, 2));
            assert_eq!(song.get(3), (5, 8));
        }
        _ => panic!("expected electro5 song"),
    }
}

#[test]
fn test_ne5_write_song() {
    let test_file = corpus_dir().join("song.ne5t");

    let song = nord_format::from_path(&test_file).unwrap();
    let contents = read(&test_file).unwrap();

    match song {
        Entity::Song(nord_format::Song::Electro5(mut song)) => {
            let mut output: Vec<u8> = Vec::new();

            song.write_to(&mut Cursor::new(&mut output)).unwrap();

            assert_eq!(contents.as_slice(), output.as_slice());
        }
        _ => panic!("expected electro5 song"),
    }
}

#[test]
fn test_ne5_read_program() {
    let test_file = corpus_dir().join("programs/center_panel/o00_1_p000_0_1_0_50_50.ne5p");

    let program = nord_format::from_path(&test_file).unwrap();

    match program {
        Entity::Program(nord_format::Program::Electro5(program)) => {
            let program = program as electro5::Program;
            let coords = program.location();

            assert_eq!(coords, (7, 3));
            assert_eq!(program.lower_part(), Instrument::Organ);
            assert_eq!(program.upper_part(), Instrument::Piano);
            assert_eq!(program.lower_octave_shift(), 1);
            assert_eq!(program.upper_octave_shift(), 0);
            assert_eq!(program.lower_sustain(), false);
            assert_eq!(program.upper_sustain(), false);
            assert_eq!(program.lower_control(), false);
            assert_eq!(program.upper_control(), false);
            assert_eq!(program.split(), false);
            assert_eq!(program.split_point(), SplitPoint::F4);
            assert_eq!(program.transpose(), 1);
            assert_eq!(program.transpose_enabled(), false);
        }
        _ => panic!("expected electro5 program"),
    }
}

#[test]
fn test_ne5_read_write_program() {
    let test_file = corpus_dir().join("programs/center_panel/o00_1_p000_0_1_0_50_50.ne5p");

    let read_contents = read(&test_file).unwrap();
    let program = nord_format::from_path(&test_file).unwrap();

    match program {
        Entity::Program(nord_format::Program::Electro5(mut program)) => {
            let mut write_contents: Vec<u8> = Vec::new();

            program
                .write_to(&mut Cursor::new(&mut write_contents))
                .unwrap();

            assert_eq!(read_contents.as_slice(), write_contents.as_slice());
        }
        _ => panic!("expected electro5 program"),
    }
}

#[test]
fn test_ne5_read_settings() {
    let test_file = corpus_dir().join("settings.ne5s");

    let program = nord_format::from_path(&test_file).unwrap();

    match program {
        Entity::Settings(nord_format::Settings::Electro5(settings)) => {
            let _settings = settings as electro5::Settings;
        }
        _ => panic!("expected electro5 settings"),
    }
}

#[test]
fn test_ne5_program_read_write_center_panel() {
    let test_files = corpus_dir().join("programs/center_panel");

    let paths = fs::read_dir(&test_files).unwrap();

    let center_panel_re = Regex::new(r"([ospx])([01])([01])_([0-9.-]+)_([ospx])([01])([01])([01])_([0-9.-]+)_([0-9.-]+)_([0-9.-]+)_([0-9.-]+)_([0-9.-]+)[.](skip[.])?ne5p$").unwrap();

    for path in paths {
        let inner = path.unwrap();

        if !inner.metadata().unwrap().is_file() {
            continue;
        }

        let path = inner.path().display().to_string();

        if let Some(matches) = center_panel_re.captures(path.as_str()) {
            let program = nord_format::from_path(path.as_str()).unwrap();
            let contents = read(path.as_str()).unwrap();

            let lower_instrument = match matches.get(1).unwrap().as_str() {
                "o" => Some(Instrument::Organ),
                "s" => Some(Instrument::Sample),
                "p" => Some(Instrument::Piano),
                "x" => None,
                _ => panic!("invalid instrument in file {}", path),
            };

            let lower_sustain = match matches.get(2).unwrap().as_str() {
                "0" => false,
                "1" => true,
                _ => panic!("invalid sustain in file {}", path),
            };

            let lower_control = match matches.get(3).unwrap().as_str() {
                "0" => false,
                "1" => true,
                _ => panic!("invalid control in file {}", path),
            };

            let lower_octave_shift = i8::from_str(matches.get(4).unwrap().as_str()).unwrap();

            let upper_instrument = match matches.get(5).unwrap().as_str() {
                "o" => Some(Instrument::Organ),
                "s" => Some(Instrument::Sample),
                "p" => Some(Instrument::Piano),
                "x" => None,
                _ => panic!("invalid instrument in file {}", path),
            };

            let upper_sustain = match matches.get(6).unwrap().as_str() {
                "0" => false,
                "1" => true,
                _ => panic!("invalid sustain in file {}", path),
            };

            let upper_control = match matches.get(7).unwrap().as_str() {
                "0" => false,
                "1" => true,
                _ => panic!("invalid control in file {}", path),
            };

            let transpose_enabled = match matches.get(8).unwrap().as_str() {
                "0" => false,
                "1" => true,
                _ => panic!("invalid transpose enabled in file {}", path),
            };

            let upper_octave_shift = i8::from_str(matches.get(9).unwrap().as_str()).unwrap();
            let transpose = i8::from_str(matches.get(10).unwrap().as_str()).unwrap();
            let split = u8::from_str(matches.get(11).unwrap().as_str()).unwrap();

            let part_mix = (
                f32::from_str(matches.get(12).unwrap().as_str()).unwrap(),
                f32::from_str(matches.get(13).unwrap().as_str()).unwrap(),
            );

            if matches.get(14).is_some() {
                continue;
            };

            match program {
                Entity::Program(nord_format::Program::Electro5(mut program)) => {
                    let mut output: Vec<u8> = Vec::new();
                    program.write_to(&mut Cursor::new(&mut output)).unwrap();

                    if let Some(lower_instrument) = lower_instrument {
                        assert_eq!(
                            program.lower_part(),
                            lower_instrument,
                            "lower instrument mismatch in file {}",
                            path
                        );
                        assert!(
                            program.schema.center_panel.lower_enabled,
                            "lower part enabled mismatch in file {}",
                            path
                        );
                    } else {
                        assert!(
                            !program.schema.center_panel.lower_enabled,
                            "lower part enabled mismatch in file {}",
                            path
                        );
                    }

                    if let Some(upper_instrument) = upper_instrument {
                        assert_eq!(
                            program.upper_part(),
                            upper_instrument,
                            "upper instrument mismatch in file {}",
                            path
                        );
                        assert!(
                            program.schema.center_panel.upper_enabled,
                            "upper part enabled mismatch in file {}",
                            path
                        );
                    } else {
                        assert!(
                            !program.schema.center_panel.upper_enabled,
                            "upper part enabled mismatch in file {}",
                            path
                        );
                    }

                    assert_eq!(
                        contents.as_slice(),
                        output.as_slice(),
                        "read/write mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.lower_octave_shift(),
                        lower_octave_shift,
                        "lower octave shift mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.upper_octave_shift(),
                        upper_octave_shift,
                        "upper octave shift mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.lower_sustain(),
                        lower_sustain,
                        "lower sustain mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.upper_sustain(),
                        upper_sustain,
                        "upper sustain mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.lower_control(),
                        lower_control,
                        "lower control mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.upper_control(),
                        upper_control,
                        "upper control mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.split(),
                        split != 0,
                        "split enabled mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.transpose_enabled(),
                        transpose_enabled,
                        "transpose enabled mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.part_mix().lower().round(),
                        part_mix.0.round(),
                        "lower part mix mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.part_mix().upper().round(),
                        part_mix.1.round(),
                        "upper part mix mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.transpose(),
                        transpose,
                        "transpose mismatch in file {}",
                        path
                    );

                    if split != 0 {
                        assert_eq!(
                            program.split_point() as u8,
                            split - 1,
                            "split point mismatch in file {}",
                            path
                        );
                    }
                }
                _ => panic!("expected electro5 song in file {}", path),
            }
        } else if !path.contains("README.md") {
            panic!("invalid file name: {}", path)
        }
    }
}

#[test]
fn test_ne5_program_read_write_gain() {
    let test_files = corpus_dir().join("programs/gain");

    let paths = fs::read_dir(&test_files).unwrap();

    let gain_re = Regex::new(r"([0-9.-]+)[.](skip[.])?ne5p$").unwrap();

    for path in paths {
        let inner = path.unwrap();

        if !inner.metadata().unwrap().is_file() {
            continue;
        }

        let path = inner.path().display().to_string();

        if let Some(matches) = gain_re.captures(path.as_str()) {
            let program = nord_format::from_path(path.as_str()).unwrap();
            let contents = read(path.as_str()).unwrap();

            let gain = f32::from_str(matches.get(1).unwrap().as_str()).unwrap();

            if matches.get(3).is_some() {
                continue;
            };

            match program {
                Entity::Program(nord_format::Program::Electro5(mut program)) => {
                    let mut output: Vec<u8> = Vec::new();
                    program.write_to(&mut Cursor::new(&mut output)).unwrap();

                    assert_eq!(
                        contents.as_slice(),
                        output.as_slice(),
                        "read/write mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.gain(),
                        ((gain / 10_f32) * 127_f32).round() as u8,
                        "gain mismatch in file {}",
                        path
                    );
                }
                _ => panic!("expected electro5 song in file {}", path),
            }
        } else if !path.contains("README.md") {
            panic!("invalid file name: {}", path)
        }
    }
}

#[test]
fn test_ne5_program_read_write_fx() {
    let test_files = corpus_dir().join("programs/fx");

    let paths = fs::read_dir(&test_files).unwrap();

    let gain_re =
        Regex::new(r"fx([0-9])_([0-9])([0-9])([0-9])_([0-9.-]+)_?([0-9.-]+)?[.](skip[.])?ne5p$")
            .unwrap();

    for path in paths {
        let inner = path.unwrap();

        if !inner.metadata().unwrap().is_file() {
            continue;
        }

        let path = inner.path().display().to_string();

        if let Some(matches) = gain_re.captures(path.as_str()) {
            let program = nord_format::from_path(path.as_str()).unwrap();
            let contents = read(path.as_str()).unwrap();

            match program {
                Entity::Program(nord_format::Program::Electro5(mut program)) => {
                    if matches.get(7).is_some() {
                        continue;
                    };

                    let fx = u8::from_str(matches.get(1).unwrap().as_str()).unwrap();
                    let part_select = u8::from_str(matches.get(2).unwrap().as_str()).unwrap();
                    let switch_enabled = u8::from_str(matches.get(3).unwrap().as_str()).unwrap();
                    let fx_type = u8::from_str(matches.get(4).unwrap().as_str()).unwrap();
                    let fx_value = f32::from_str(matches.get(5).unwrap().as_str()).unwrap();

                    let fx_value2 = matches
                        .get(6)
                        .map(|value| f32::from_str(value.as_str()).unwrap());

                    match fx {
                        1 => {
                            assert_eq!(
                                program.schema.effects_panel.fx1,
                                part_select + 1,
                                "fx1 part select mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.extra.fx1_control,
                                switch_enabled != 0,
                                "fx1 control mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx1_rate,
                                ((fx_value / 10_f32) * 127_f32).floor() as u8,
                                "fx1 rate mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx1_type,
                                match fx_type {
                                    0 => 3, // pan 1
                                    1 => 4, // pan 2
                                    2 => 5, // pan 1&2
                                    3 => 6, // wah
                                    4 => 7, // rm
                                    5 => 0, // trem 1
                                    6 => 1, // trem 2
                                    7 => 2, // trem 1&2
                                    a => panic!("unknown fx1 type {} in file {}", a, path),
                                },
                                "fx1 type mismatch in file {}",
                                path
                            );
                        }
                        2 => {
                            assert_eq!(
                                program.schema.effects_panel.fx2,
                                part_select + 1,
                                "fx2 part select mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.extra.fx2_deep,
                                switch_enabled != 0,
                                "fx2 deep mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx2_rate,
                                fx_value.floor() as u8,
                                "fx2 rate mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx2_type,
                                match fx_type {
                                    0 => 2, // flang
                                    1 => 3, // choir1
                                    2 => 4, // choir2
                                    3 => 5, // vibe
                                    4 => 0, // phas1
                                    5 => 1, // phas2
                                    a => panic!("unknown fx2 type {} in file {}", a, path),
                                },
                                "fx2 type mismatch in file {}",
                                path
                            );
                        }
                        3 => {
                            assert_eq!(
                                program.schema.effects_panel.fx3,
                                part_select + 1,
                                "fx3 part select mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx3_compression as f32, fx_value,
                                "fx3 compression mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx3_compression > 0,
                                switch_enabled != 0,
                                "fx3 drive on mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx3_type,
                                match fx_type {
                                    0 => 0, // none
                                    1 => 3, // twin
                                    2 => 4, // rotary
                                    3 => 5, // comp
                                    4 => 1, // small
                                    5 => 2, // jc
                                    a => panic!("unknown fx3 type {} in file {}", a, path),
                                },
                                "fx3 type mismatch in file {}",
                                path
                            );
                        }
                        4 => {
                            assert_eq!(
                                program.schema.effects_panel.fx4,
                                part_select + 1,
                                "fx4 part select mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx4_ping_pong,
                                switch_enabled != 0,
                                "fx4 ping pong mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx4_moisture as f32,
                                ((fx_value / 10_f32) * 127_f32).floor(),
                                "fx4 moisture mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx4_tempo as f32,
                                fx_value2.unwrap().floor(),
                                "fx4 tempo mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx4_feedback, fx_type,
                                "fx4 type mismatch in file {}",
                                path
                            );
                        }
                        5 => {
                            assert_eq!(
                                program.schema.effects_panel.fx5,
                                part_select == 1,
                                "fx5 part select mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx5_moisture as f32, fx_value,
                                "fx5 moisture mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx5_type,
                                match fx_type {
                                    0 => 2, // stage
                                    1 => 3, // hall-soft
                                    2 => 4, // hall
                                    3 => 0, // room
                                    4 => 1, // stage-soft
                                    a => panic!("unknown fx5 type {} in file {}", a, path),
                                },
                                "fx5 type mismatch in file {}",
                                path
                            );
                        }
                        _ => panic!("unknown fx {} in file {}", fx, path),
                    }

                    let mut output: Vec<u8> = Vec::new();
                    program.write_to(&mut Cursor::new(&mut output)).unwrap();
                    assert_eq!(
                        contents.as_slice(),
                        output.as_slice(),
                        "read/write mismatch in file {}",
                        path
                    );
                }
                _ => panic!("expected electro5 song in file {}", path),
            }
        } else if !path.contains("README.md") {
            panic!("invalid file name: {}", path)
        }
    }
}

#[test]
fn test_ne5_program_read_write_equalizer() {
    let test_files = corpus_dir().join("programs/equalizer");

    let paths = fs::read_dir(&test_files).unwrap();

    let equalizer_re =
        Regex::new(r"([0-9]+)_([0-9]{3})([0-9]{3})([0-9]{3})([0-9]{3})[.](skip[.])?ne5p$").unwrap();

    for path in paths {
        let inner = path.unwrap();

        if !inner.metadata().unwrap().is_file() {
            continue;
        }

        let path = inner.path().display().to_string();

        if let Some(matches) = equalizer_re.captures(path.as_str()) {
            let program = nord_format::from_path(path.as_str()).unwrap();
            let contents = read(path.as_str()).unwrap();

            if matches.get(6).is_some() {
                continue;
            };

            let _part_select = u8::from_str(matches.get(1).unwrap().as_str()).unwrap();
            let _bass = u8::from_str(matches.get(2).unwrap().as_str()).unwrap();
            let _freq = u8::from_str(matches.get(3).unwrap().as_str()).unwrap();
            let _freq_gain = u8::from_str(matches.get(4).unwrap().as_str()).unwrap();
            let _treble = u8::from_str(matches.get(5).unwrap().as_str()).unwrap();

            match program {
                Entity::Program(nord_format::Program::Electro5(mut program)) => {
                    let mut output: Vec<u8> = Vec::new();
                    program.write_to(&mut Cursor::new(&mut output)).unwrap();

                    assert_eq!(
                        contents.as_slice(),
                        output.as_slice(),
                        "read/write mismatch in file {}",
                        path
                    );
                }
                _ => panic!("expected electro5 song in file {}", path),
            }
        } else if !path.contains("README.md") {
            panic!("invalid file name: {}", path)
        }
    }
}

#[test]
fn test_ne5_program_read_sample() {
    let test_files = corpus_dir().join("programs/sample");

    let paths = fs::read_dir(&test_files).unwrap();

    let sample_re = Regex::new(
        r"([0-9])([0-9])([0-9])_([a-fA-F0-9]{2})_([0-9]{3})_([dsr])([0-9]{3})[.](skip[.])?ne5p$",
    )
    .unwrap();

    for path in paths {
        let inner = path.unwrap();

        if !inner.metadata().unwrap().is_file() {
            continue;
        }

        let path = inner.path().display().to_string();

        if let Some(matches) = sample_re.captures(path.as_str()) {
            let program = nord_format::from_path(path.as_str()).unwrap();
            let contents = read(path.as_str()).unwrap();

            if matches.get(8).is_some() {
                continue;
            };

            let _part_select = u8::from_str(matches.get(1).unwrap().as_str()).unwrap();
            let _dynamics = u8::from_str(matches.get(2).unwrap().as_str()).unwrap();
            let _filter = u8::from_str(matches.get(3).unwrap().as_str()).unwrap();
            let _sample_id = matches.get(4).unwrap().as_str();
            let _attack = u8::from_str(matches.get(5).unwrap().as_str()).unwrap();
            let _decay_release_type = matches.get(6).unwrap().as_str();
            let _decay_release = u8::from_str(matches.get(7).unwrap().as_str()).unwrap();

            match program {
                Entity::Program(nord_format::Program::Electro5(mut program)) => {
                    let mut output: Vec<u8> = Vec::new();
                    program.write_to(&mut Cursor::new(&mut output)).unwrap();

                    assert_eq!(
                        contents.as_slice(),
                        output.as_slice(),
                        "read/write mismatch in file {}",
                        path
                    );
                }
                _ => panic!("expected electro5 song in file {}", path),
            }
        } else if !path.contains("README.md") {
            panic!("invalid file name: {}", path)
        }
    }
}

#[test]
fn test_ne5_program_read_write_organ() {
    use nord_format::electro5::{OrganModel, PercSpeed, VibChorus};

    let test_files = corpus_dir().join("programs/organ");

    // Filename drawbar char -> physical position (0..=8). Digits and the two
    // letter ranges all encode the same nine physical positions; only the
    // display "real" value differs (a..i => real 0, j..r => real 1).
    fn physical(c: u8) -> u8 {
        match c {
            b'0'..=b'8' => c - b'0',
            b'a'..=b'i' => c - b'a',
            b'j'..=b'r' => c - b'j',
            _ => panic!("bad drawbar char: {}", c as char),
        }
    }

    // Filename model digit -> (model, on-disk value == physical position?).
    // B3/Vox/Pipe store the physical bar position, so their drawbars are
    // asserted directly. B3-bass (1) remaps its bass bars and Farfisa (4)
    // quantizes intermediate values on disk (e.g. physical 5 -> 4), so their
    // exact values aren't asserted yet — the bytes still round-trip.
    fn model_of(d: u8) -> (OrganModel, bool) {
        match d {
            0 => (OrganModel::B3, true),
            1 => (OrganModel::B3, false),
            2 => (OrganModel::Pipe, true),
            3 => (OrganModel::Vox, true),
            4 => (OrganModel::Farfisa, false),
            _ => panic!("unknown organ model digit {d}"),
        }
    }

    // Filename vib-type digit (0..5) -> mode. Filename order is v2,c2,v3,c3,v1,c1.
    fn vib_of(i: u8) -> VibChorus {
        use VibChorus::*;
        [V2, C2, V3, C3, V1, C1][i as usize]
    }
    // Filename perc-speed digit (0..3) -> speed.
    fn speed_of(e: u8) -> PercSpeed {
        use PercSpeed::*;
        [Off, Soft, Fast, Both][e as usize]
    }
    let dig = |m: &regex::Captures, i: usize| m[i].parse::<u8>().unwrap();

    // Per-file vib/perc toggles decoded from the filename, if it encodes them.
    enum Toggles {
        // type-A: full B3 percussion + vibrato state
        B3 {
            perc_on: bool,
            perc_third: bool,
            perc_speed: PercSpeed,
            vib_on: bool,
            vib_type: VibChorus,
        },
        // type-C: Vox/Farfisa vibrato only (they have no percussion)
        Vib {
            vib_on: bool,
            vib_type: VibChorus,
        },
        // type-B drawbar specimens carry no toggle info
        None,
    }

    // The three filename shapes actually present in the corpus:
    //   type-A  P d c t s v y DDDDDDDDD   (16 digits: 7 B3 toggle fields + 9 drawbars)
    //   type-B  PMrs_DDDDDDDDD            (preset, model, rot_speed, rot_stop + 9 drawbars)
    //   type-C  PMrs_ctsvy               (preset, model, rot_speed, rot_stop + 5 perc/vib)
    let type_a = Regex::new(r"^(\d)(\d)(\d)(\d)(\d)(\d)(\d)([0-8]{9})\.ne5p$").unwrap();
    let type_b = Regex::new(r"^(\d)(\d)(\d)(\d)_([0-8a-r]{9})\.ne5p$").unwrap();
    let type_c = Regex::new(r"^(\d)(\d)(\d)(\d)_(\d)(\d)(\d)(\d)(\d)\.ne5p$").unwrap();

    let mut drawbar_checks = 0usize;
    let mut toggle_checks = 0usize;

    for entry in fs::read_dir(&test_files).unwrap() {
        let entry = entry.unwrap();
        if !entry.metadata().unwrap().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "README.md" || name.contains(".skip.") {
            continue;
        }
        let path = entry.path();

        // (model, preset, expected drawbar chars if any, storage==physical, toggles)
        let (model, preset, drawbars, physical_storage, toggles): (
            OrganModel,
            u8,
            Option<String>,
            bool,
            Toggles,
        ) = if let Some(m) = type_a.captures(&name) {
            let toggles = Toggles::B3 {
                perc_on: dig(&m, 3) == 1,
                perc_third: dig(&m, 4) == 1,
                perc_speed: speed_of(dig(&m, 5)),
                vib_on: dig(&m, 6) == 1,
                vib_type: vib_of(dig(&m, 7)),
            };
            (
                OrganModel::B3,
                dig(&m, 1),
                Some(m[8].to_string()),
                true,
                toggles,
            )
        } else if let Some(m) = type_b.captures(&name) {
            let (model, phys) = model_of(dig(&m, 2));
            (
                model,
                dig(&m, 1),
                Some(m[5].to_string()),
                phys,
                Toggles::None,
            )
        } else if let Some(m) = type_c.captures(&name) {
            let (model, _) = model_of(dig(&m, 2));
            let toggles = Toggles::Vib {
                vib_on: dig(&m, 8) == 1,
                vib_type: vib_of(dig(&m, 9)),
            };
            (model, dig(&m, 1), None, false, toggles)
        } else {
            panic!("unrecognized organ file name: {name}");
        };

        let contents = read(&path).unwrap();
        let mut program = match nord_format::from_path(&path).unwrap() {
            Entity::Program(nord_format::Program::Electro5(p)) => p,
            _ => panic!("expected electro5 program in file {name}"),
        };
        let organ = program.organ();

        // Preset selection decodes to the value encoded in the filename.
        assert_eq!(organ.preset(model), preset, "preset mismatch in {name}");

        // Drawbars decode to the filename's physical positions (except B3-bass).
        if let (Some(chars), true) = (drawbars.as_ref(), physical_storage) {
            let expected: Vec<u8> = chars.bytes().map(physical).collect();
            assert_eq!(
                organ.drawbars(model, preset).as_slice(),
                expected.as_slice(),
                "drawbar decode mismatch in {name} ({model:?} preset {preset})",
            );
            drawbar_checks += 1;
        }

        // Vibrato / percussion toggles.
        match toggles {
            Toggles::B3 {
                perc_on,
                perc_third,
                perc_speed,
                vib_on,
                vib_type,
            } => {
                assert_eq!(organ.b3_perc_on(preset), perc_on, "b3 perc_on in {name}");
                assert_eq!(organ.b3_perc_third(), perc_third, "b3 perc_third in {name}");
                assert_eq!(organ.b3_perc_speed(), perc_speed, "b3 perc_speed in {name}");
                assert_eq!(organ.vib_on(model, preset), vib_on, "b3 vib_on in {name}");
                assert_eq!(
                    organ.vib_type(model),
                    Some(vib_type),
                    "b3 vib_type in {name}"
                );
                toggle_checks += 1;
            }
            Toggles::Vib { vib_on, vib_type } => {
                assert_eq!(organ.vib_on(model, preset), vib_on, "vib_on in {name}");
                assert_eq!(organ.vib_type(model), Some(vib_type), "vib_type in {name}");
                toggle_checks += 1;
            }
            Toggles::None => {}
        }

        // Round-trip stays byte-exact regardless of how much is decoded.
        let mut output: Vec<u8> = Vec::new();
        program.write_to(&mut Cursor::new(&mut output)).unwrap();
        assert_eq!(
            contents.as_slice(),
            output.as_slice(),
            "read/write mismatch in {name}",
        );
    }

    assert!(
        drawbar_checks > 0 && toggle_checks > 0,
        "no organ assertions ran — is the organ corpus present?"
    );
}
