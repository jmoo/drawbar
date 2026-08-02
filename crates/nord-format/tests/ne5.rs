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
use nord_format::electro5::{
    EqualizerPart, Fx1Type, Fx2Type, Fx3Type, Fx5Type, Instrument, PianoCategory, Routing,
    SplitPoint,
};
use nord_format::{electro5, Entity};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
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
            assert_eq!(program.schema.center_panel.lower_part, Instrument::Organ);
            assert_eq!(program.schema.center_panel.upper_part, Instrument::Piano);
            assert_eq!(program.schema.center_panel.lower_octave_shift, 1);
            assert_eq!(program.schema.center_panel.upper_octave_shift, 0);
            assert!(!program.schema.center_panel.lower_sustain);
            assert!(!program.schema.center_panel.upper_sustain);
            assert!(!program.schema.center_panel.lower_control);
            assert!(!program.schema.center_panel.upper_control);
            assert!(!program.schema.center_panel.split);
            assert_eq!(program.schema.center_panel.split_point, SplitPoint::F4);
            assert_eq!(program.schema.center_panel.transpose, 1);
            assert!(!program.schema.center_panel.transpose_enabled);
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
fn test_ne5_write_settings() {
    let test_file = corpus_dir().join("settings.ne5s");

    let settings = nord_format::from_path(&test_file).unwrap();
    let contents = read(&test_file).unwrap();

    match settings {
        Entity::Settings(nord_format::Settings::Electro5(mut settings)) => {
            let mut output: Vec<u8> = Vec::new();

            settings.write_to(&mut Cursor::new(&mut output)).unwrap();

            assert_eq!(contents.as_slice(), output.as_slice());
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
                            program.schema.center_panel.lower_part, lower_instrument,
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
                            program.schema.center_panel.upper_part, upper_instrument,
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
                        program.schema.center_panel.lower_octave_shift, lower_octave_shift,
                        "lower octave shift mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.upper_octave_shift, upper_octave_shift,
                        "upper octave shift mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.lower_sustain, lower_sustain,
                        "lower sustain mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.upper_sustain, upper_sustain,
                        "upper sustain mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.lower_control, lower_control,
                        "lower control mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.upper_control, upper_control,
                        "upper control mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.split,
                        split != 0,
                        "split enabled mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.transpose_enabled, transpose_enabled,
                        "transpose enabled mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.part_mix.lower().round(),
                        part_mix.0.round(),
                        "lower part mix mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.part_mix.upper().round(),
                        part_mix.1.round(),
                        "upper part mix mismatch in file {}",
                        path
                    );
                    assert_eq!(
                        program.schema.center_panel.transpose, transpose,
                        "transpose mismatch in file {}",
                        path
                    );

                    if split != 0 {
                        assert_eq!(
                            program.schema.center_panel.split_point as u8,
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
                        program.schema.center_panel.gain,
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
                                Routing::from_panel(part_select)
                                    .unwrap_or_else(|| panic!("bad part select in {path}")),
                                "fx1 part select mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx1_control,
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
                                    0 => Fx1Type::Pan1,
                                    1 => Fx1Type::Pan2,
                                    2 => Fx1Type::Pan1And2,
                                    3 => Fx1Type::Wah,
                                    4 => Fx1Type::Rm,
                                    5 => Fx1Type::Trem1,
                                    6 => Fx1Type::Trem2,
                                    7 => Fx1Type::Trem1And2,
                                    a => panic!("unknown fx1 type {} in file {}", a, path),
                                },
                                "fx1 type mismatch in file {}",
                                path
                            );
                        }
                        2 => {
                            assert_eq!(
                                program.schema.effects_panel.fx2,
                                Routing::from_panel(part_select)
                                    .unwrap_or_else(|| panic!("bad part select in {path}")),
                                "fx2 part select mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx2_deep,
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
                                    0 => Fx2Type::Flanger,
                                    1 => Fx2Type::Chorus1,
                                    2 => Fx2Type::Chorus2,
                                    3 => Fx2Type::Vibe,
                                    4 => Fx2Type::Phaser1,
                                    5 => Fx2Type::Phaser2,
                                    a => panic!("unknown fx2 type {} in file {}", a, path),
                                },
                                "fx2 type mismatch in file {}",
                                path
                            );
                        }
                        3 => {
                            assert_eq!(
                                program.schema.effects_panel.fx3,
                                Routing::from_panel(part_select)
                                    .unwrap_or_else(|| panic!("bad part select in {path}")),
                                "fx3 part select mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx3_compression.as_u8() as f32,
                                fx_value,
                                "fx3 compression mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx3_compression.as_u8() > 0,
                                switch_enabled != 0,
                                "fx3 drive on mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx3_type,
                                match fx_type {
                                    0 => Fx3Type::None_,
                                    1 => Fx3Type::Twin,
                                    2 => Fx3Type::Rotary,
                                    3 => Fx3Type::Comp,
                                    4 => Fx3Type::Small,
                                    5 => Fx3Type::Jc,
                                    a => panic!("unknown fx3 type {} in file {}", a, path),
                                },
                                "fx3 type mismatch in file {}",
                                path
                            );
                        }
                        4 => {
                            assert_eq!(
                                program.schema.effects_panel.fx4,
                                Routing::from_panel(part_select)
                                    .unwrap_or_else(|| panic!("bad part select in {path}")),
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
                                program.schema.effects_panel.fx4_moisture.as_u8() as f32,
                                ((fx_value / 10_f32) * 127_f32).floor(),
                                "fx4 moisture mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx4_tempo.as_u8() as f32,
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
                                program.schema.effects_panel.fx5_moisture.as_u8() as f32,
                                fx_value,
                                "fx5 moisture mismatch in file {}",
                                path
                            );
                            assert_eq!(
                                program.schema.effects_panel.fx5_type,
                                match fx_type {
                                    0 => Fx5Type::Stage,
                                    1 => Fx5Type::HallSoft,
                                    2 => Fx5Type::Hall,
                                    3 => Fx5Type::Room,
                                    4 => Fx5Type::StageSoft,
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
        let organ = &program.schema.organ_panel;

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

/// b3+bass preset 1 keeps its two bass drawbars outside the nine-nibble block, so
/// `drawbars()` cannot see them. Check `b3_bass_drawbars()` against the filename for
/// every Type-B b3+bass specimen (`1 1 r s _ DDDDDDDDD`, model digit 1).
///
/// This is the real-data counterpart to the unit tests in `program.rs`: those pin the
/// bit layout, this pins it to specimens captured off the instrument.
#[test]
fn test_ne5_b3_bass_drawbars_match_filenames() {
    use nord_format::electro5::program::OrganModel;

    let dir = corpus_dir().join("programs/organ");
    let mut checked = 0;

    for entry in std::fs::read_dir(&dir).expect("organ corpus") {
        let path = entry.expect("dir entry").path();
        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        // Type-B: `abcd_fffffffff`, model digit (b) == 1 is b3+bass.
        let (head, bars) = match name.split_once('_') {
            Some(parts) => parts,
            None => continue,
        };
        if head.len() != 4 || bars.len() != 9 || !bars.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let mut head = head.chars();
        let preset = head
            .next()
            .and_then(|c| c.to_digit(10))
            .expect("preset digit") as u8;
        let model = head
            .next()
            .and_then(|c| c.to_digit(10))
            .expect("model digit");
        if model != 1 || preset != 1 {
            continue; // bass manual is preset 1 of b3+bass only
        }

        let program = match nord_format::from_path(&path).expect("parse") {
            nord_format::Entity::Program(nord_format::Program::Electro5(p)) => p,
            other => panic!("{name}: expected an electro5 program, got {other:?}"),
        };
        let got = program.schema.organ_panel.b3_bass_drawbars();
        let want = [bars.as_bytes()[0] - b'0', bars.as_bytes()[1] - b'0'];
        assert_eq!(got, want, "bass drawbars in {name}");

        // The main block's first two nibbles are stale in this mode and must not be
        // mistaken for the bass values — assert we are genuinely reading elsewhere.
        let main = program.schema.organ_panel.drawbars(OrganModel::B3, 1);
        if want != [0, 0] && (main[0], main[1]) == (want[0], want[1]) {
            panic!("{name}: bass values also appear in the main block — offsets may be wrong");
        }
        checked += 1;
    }

    assert!(
        checked >= 4,
        "expected several b3+bass preset-1 specimens, saw {checked}"
    );
}

// ---------------------------------------------------------------------------
// Piano / sample dependency ids
// ---------------------------------------------------------------------------
//
// Both panels carry a 32-bit reference to the piano (`.npno`) or sample
// (`.nsmp`) the program needs, in bits 41..=10 of the panel word. Everything
// else in those panels is a *slot coordinate* — the piano category + model
// dials, the Samp Lib number — which moves when the instrument's library
// changes. The id is the stable key: it is what resolves the song -> program ->
// piano chain, and what Nord Sound Manager checks before offering a Restore.
//
// The width and the shift are pinned from three independent angles:
//
//   * `programs/piano` and `programs/sample` are change-one-knob specimens, so
//     the id must be *invariant* under every neighbouring field, and must equal
//     a known value. The golden ids below are not self-referential: each was
//     read off the USB captures, where the vendor protocol transmits the same
//     id as a plain big-endian u32 immediately followed by a length-prefixed
//     piano name (see `usb/program/relink_piano_*`). Byte alignment there is
//     what fixes the shift — a decode that is off by even one bit produces
//     values that appear nowhere on the wire.
//   * across all 624 corpus programs each piano id must occupy exactly one
//     (category, model) slot and vice versa. A too-narrow id over-splits a slot
//     into several ids; a too-wide one merges distinct pianos under one id.
//   * the backup's own member list says how many pianos each category shipped,
//     which must equal the number of model slots the programs reference — bar
//     the single missing dependency documented below.

/// Piano `category` as stored, and the backup directory it corresponds to. The
/// dial order on disk starts at Grand, so the two are not in step.
const PIANO_CATEGORIES: [(PianoCategory, &str); 6] = [
    (PianoCategory::Grand, "Grand"),
    (PianoCategory::Upright, "Upright"),
    (PianoCategory::EPiano1, "EPiano1"),
    (PianoCategory::EPiano2, "EPiano2"),
    (PianoCategory::Clavinet, "Clavinet"),
    (PianoCategory::Harpsichord, "Harps"),
];

/// `(category, model, id, name)` for every piano slot the `programs/piano`
/// specimens select. Ids and names come from the USB captures, not from this
/// decoder; the names are the shipped `.npno` basenames.
const PIANO_IDS: [(PianoCategory, u8, u32, &str); 11] = [
    (
        PianoCategory::Grand,
        0,
        0xd303_b5f2,
        "Royal Grand 3D YaS6 XL 5.4",
    ),
    (
        PianoCategory::Grand,
        5,
        0x4ca6_ab08,
        "Electric Grand 1 CP80  5.3",
    ),
    (
        PianoCategory::Upright,
        0,
        0x645f_053e,
        "Grand Upright YaU3 Lrg 5.4",
    ),
    (
        PianoCategory::Upright,
        5,
        0x42a7_a3a3,
        "HonkyTonkUpright      Sml 5.3",
    ),
    (
        PianoCategory::EPiano1,
        0,
        0x6434_2577,
        "EPiano 1    Mk I Low Deep  5.3",
    ),
    (
        PianoCategory::EPiano1,
        9,
        0x19c5_f749,
        "EP8 Nefertiti Lrg 6.0",
    ),
    (
        PianoCategory::EPiano2,
        0,
        0xd1ac_cddd,
        "DX7 FullTines  Lrg 5.4",
    ),
    (
        PianoCategory::EPiano2,
        2,
        0x17a8_d178,
        "Ballad EP1  Sml 5.2",
    ),
    (PianoCategory::Clavinet, 0, 0x9bed_fa45, "Clavinet D6  5.0"),
    (
        PianoCategory::Harpsichord,
        0,
        0xf121_ce36,
        "Ital Harpsich 1B Long Stri 5.0",
    ),
    (
        PianoCategory::Harpsichord,
        2,
        0x1251_b69a,
        "Ital Harpsich 1D Lute 5.0",
    ),
];

/// `(samp lib number, id)` for the sample slots `programs/sample` selects. As
/// above, the ids are the ones the vendor protocol puts on the wire.
const SAMPLE_IDS: [(u8, u32); 3] = [(0, 0x47c8_b4f8), (41, 0x89be_e289), (158, 0x0ac2_1363)];

/// Every `.ne5p` under `dir`, recursively, sorted for deterministic failures.
fn ne5p_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();

            if entry.metadata().unwrap().is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "ne5p") {
                out.push(path);
            }
        }
    }

    out.sort();
    out
}

/// Read a program and assert it still round-trips byte-exact. The id fields are
/// read-only views over a verbatim `settings` word, so decoding more of it must
/// never cost the round-trip.
fn read_program_checked(path: &std::path::Path) -> electro5::Program {
    let name = path.display().to_string();
    let contents = read(path).unwrap();

    let Entity::Program(nord_format::Program::Electro5(mut program)) =
        nord_format::from_path(name.as_str()).unwrap()
    else {
        panic!("expected an electro5 program in {name}")
    };

    let mut output: Vec<u8> = Vec::new();
    program.write_to(&mut Cursor::new(&mut output)).unwrap();
    assert_eq!(
        contents.as_slice(),
        output.as_slice(),
        "read/write mismatch in {name}",
    );

    program
}

/// A two-character panel number as the Electro 5 displays it: the tens digit
/// runs `0`..`9` then `A`..`F`, the units digit `0`..`9`, so `F9` is 159.
fn panel_number(text: &str) -> u8 {
    let bytes = text.as_bytes();
    assert_eq!(bytes.len(), 2, "not a panel number: {text}");

    let tens = match bytes[0] {
        c @ b'0'..=b'9' => c - b'0',
        c @ b'A'..=b'F' => c - b'A' + 10,
        c @ b'a'..=b'f' => c - b'a' + 10,
        c => panic!("bad panel number digit: {}", c as char),
    };
    let units = bytes[1] - b'0';
    assert!(units < 10, "bad panel number digit: {text}");

    tens * 10 + units
}

#[test]
fn test_ne5_program_piano_id() {
    // `abcd_ee_ff.ne5p` — a: part, b: acoustics, c: mono, d: touch, ee: type,
    // ff: model. See `programs/piano/README.md`.
    let piano_re = Regex::new(r"([0-9])([0-3])([01])([0-3])_([0-9]{2})_([0-9A-Fa-f]{2})\.ne5p$")
        .expect("piano filename pattern");

    // Filename type digit -> the category it names.
    fn category_of(ee: u8) -> PianoCategory {
        match ee {
            1 => PianoCategory::EPiano1,
            2 => PianoCategory::EPiano2,
            3 => PianoCategory::Clavinet,
            4 => PianoCategory::Harpsichord,
            5 => PianoCategory::Grand,
            6 => PianoCategory::Upright,
            _ => panic!("bad piano type digit: {ee}"),
        }
    }

    let golden: BTreeMap<(PianoCategory, u8), (u32, &str)> = PIANO_IDS
        .iter()
        .map(|&(category, model, id, name)| ((category, model), (id, name)))
        .collect();

    let mut checks = 0usize;
    let mut covered: BTreeSet<(PianoCategory, u8)> = BTreeSet::new();

    for path in ne5p_files(&corpus_dir().join("programs/piano")) {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let matches = piano_re
            .captures(name.as_str())
            .unwrap_or_else(|| panic!("invalid file name: {name}"));

        let acoustics = u8::from_str(&matches[2]).unwrap();
        let mono = &matches[3] == "1";
        let touch = u8::from_str(&matches[4]).unwrap();
        let category = category_of(u8::from_str(&matches[5]).unwrap());

        let piano = &read_program_checked(&path).schema.piano_panel;

        assert_eq!(piano.category, category, "category in {name}");
        assert_eq!(piano.acoustics, acoustics, "acoustics in {name}");
        assert_eq!(piano.mono, mono, "mono in {name}");
        assert_eq!(piano.touch, touch, "touch in {name}");

        // Clav's model field is a variant code (`0A`, `0d`) rather than a slot
        // number — those two specimens differ only in `clav_model` and both sit
        // in model slot 0.
        if category != category_of(3) {
            // The panel shows a 1-based model number; the field stores the slot.
            let model = panel_number(&matches[6]) - 1;
            assert_eq!(piano.piano_model, model, "piano_model in {name}");
        }

        let slot = (piano.category, piano.piano_model.as_u8());
        let (id, piano_name) = golden
            .get(&slot)
            .unwrap_or_else(|| panic!("no golden id for slot {slot:?}, from {name}"));

        // The value, not just its stability: this is the number the instrument
        // puts on the wire for "{piano_name}".
        assert_eq!(
            piano.id, *id,
            "piano id in {name} should reference {piano_name}",
        );

        covered.insert(slot);
        checks += 1;
    }

    assert!(
        checks > 0,
        "no piano specimens found — is the corpus present?"
    );
    assert_eq!(
        covered.len(),
        PIANO_IDS.len(),
        "the golden table lists slots the corpus no longer covers",
    );
}

#[test]
fn test_ne5_program_sample_id() {
    // `abc_dd_eee_fggg.ne5p` — a: part, b: dynamics, c: filter, dd: sample
    // number, eee: attack, f/ggg: decay-release. See `programs/sample/README.md`.
    let sample_re =
        Regex::new(r"([0-9])([0-3])([01])_([0-9A-Fa-f]{2})_([0-9]{3})_([dsr])([0-9]{3})\.ne5p$")
            .expect("sample filename pattern");

    let golden: BTreeMap<u8, u32> = SAMPLE_IDS.iter().copied().collect();

    let mut checks = 0usize;
    let mut covered: BTreeSet<u8> = BTreeSet::new();

    for path in ne5p_files(&corpus_dir().join("programs/sample")) {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name.contains(".skip.") {
            continue;
        }
        let matches = sample_re
            .captures(name.as_str())
            .unwrap_or_else(|| panic!("invalid file name: {name}"));

        let dynamics = u8::from_str(&matches[2]).unwrap();
        let filter = &matches[3] == "1";
        // The panel shows a 1-based number; the field stores the slot index.
        let number = panel_number(&matches[4]) - 1;
        let decay_release = u8::from_str(&matches[7]).unwrap();

        let sample = &read_program_checked(&path).schema.sample_panel;

        assert_eq!(sample.dynamics, dynamics, "dynamics in {name}");
        assert_eq!(sample.filter, filter, "filter in {name}");
        assert_eq!(sample.number, number, "sample number in {name}");
        assert_eq!(
            sample.decay_release, decay_release,
            "decay_release in {name}",
        );

        // `number`, `id`, `dynamics` and `filter` are packed shoulder to
        // shoulder in one word, so a shift that is off by a bit smears one into
        // the next; asserting all four together catches that.
        let id = golden
            .get(&number)
            .unwrap_or_else(|| panic!("no golden id for sample number {number}, from {name}"));
        assert_eq!(sample.id, *id, "sample id in {name}");

        covered.insert(number);
        checks += 1;
    }

    assert!(
        checks > 0,
        "no sample specimens found — is the corpus present?"
    );
    assert_eq!(
        covered.len(),
        SAMPLE_IDS.len(),
        "the golden table lists samples the corpus no longer covers",
    );
}

#[test]
fn test_ne5_backup_dependency_ids() {
    let backup = corpus_dir().join("usb/backup/full_backup");

    // The backup's member list: what the instrument actually shipped. The blobs
    // themselves are private-tier and absent here, but the listing tells us how
    // many pianos each category held.
    let members = fs::read_to_string(backup.join("backup.members.tsv")).unwrap();
    let mut shipped: BTreeMap<&str, usize> = BTreeMap::new();
    let mut samples = 0usize;

    for line in members.lines().skip(1) {
        let Some(name) = line.split('\t').next() else {
            continue;
        };

        if name.ends_with(".nsmp") {
            samples += 1;
        } else if name.ends_with(".npno") {
            let mut parts = name.split('/');
            let (Some("Piano"), Some(category)) = (parts.next(), parts.next()) else {
                panic!("unexpected piano member path: {name}")
            };
            *shipped.entry(category).or_default() += 1;
        }
    }
    assert!(
        !shipped.is_empty() && samples > 0,
        "member list looks empty"
    );

    let mut slot_of: BTreeMap<u32, (PianoCategory, u8)> = BTreeMap::new();
    let mut id_of: BTreeMap<(PianoCategory, u8), u32> = BTreeMap::new();
    let mut sample_ids: BTreeSet<u32> = BTreeSet::new();
    let mut programs = 0usize;

    for path in ne5p_files(&backup.join("contents/Program")) {
        let name = path.display().to_string();
        let schema = read_program_checked(&path).schema;
        let (piano, sample) = (&schema.piano_panel, &schema.sample_panel);

        if piano.id != 0 {
            let slot = (piano.category, piano.piano_model.as_u8());

            // (category, model) and id are two names for the same piano, so the
            // map between them is a bijection across all 624 programs.
            assert_eq!(
                *slot_of.entry(piano.id).or_insert(slot),
                slot,
                "piano id {:#010x} spans more than one (category, model) slot, at {name}",
                piano.id,
            );
            assert_eq!(
                *id_of.entry(slot).or_insert(piano.id),
                piano.id,
                "slot {slot:?} names more than one piano id, at {name}",
            );
        }

        if sample.id != 0 {
            sample_ids.insert(sample.id);
        }

        programs += 1;
    }

    assert!(
        programs > 0,
        "no backup programs found — is the corpus present?"
    );

    // Per category: the model slots the programs reference must be exactly
    // `0..n`, and `n` must be what the backup shipped — with one exception. The
    // programs reference a seventh Upright that the instrument no longer holds,
    // and that single dangling reference is the whole reason this field matters:
    // it is what Nord Sound Manager sees as a missing dependency and gates
    // "Restore" on. If this trips, check whether the corpus gained or lost a
    // piano before assuming the decode moved.
    for (category, directory) in PIANO_CATEGORIES {
        let models: BTreeSet<u8> = id_of
            .keys()
            .filter(|(c, _)| *c == category)
            .map(|(_, model)| *model)
            .collect();
        let expected = shipped[directory] + usize::from(directory == "Upright");

        assert_eq!(
            models.len(),
            expected,
            "{directory}: programs reference {} model slots, expected {expected}",
            models.len(),
        );
        assert!(
            models.iter().copied().eq(0..expected as u8),
            "{directory}: model slots are not contiguous from 0: {models:?}",
        );
    }

    // Samples have no slot coordinate to cross-check against — `number` is a
    // volatile Samp Lib position that the corpus reuses across ids — so bound
    // the id count by the shipped library instead.
    assert!(
        sample_ids.len() <= samples,
        "{} distinct sample ids referenced but only {samples} `.nsmp` members shipped",
        sample_ids.len(),
    );
}

/// Re-encoding every panel untouched must reproduce the file byte for byte, including
/// the bits no field claims.
#[test]
fn test_ne5_every_panel_re_encodes_to_the_same_bytes() {
    let paths = ne5p_files(&corpus_dir().join("programs"));
    assert!(
        !paths.is_empty(),
        "no programs found — is the corpus present?"
    );

    for path in &paths {
        let name = path.display().to_string();
        let original = read(path).unwrap();
        let mut program = read_program_checked(path);

        let mut rewritten: Vec<u8> = Vec::new();
        program.write_to(&mut Cursor::new(&mut rewritten)).unwrap();

        assert_eq!(
            original.as_slice(),
            rewritten.as_slice(),
            "re-encoding changed {name}",
        );
    }
}

/// Assigning to a field reaches the bytes, reads back, and moves nothing outside that
/// panel's own byte span.
#[test]
fn test_ne5_a_mutation_in_every_panel_reaches_the_bytes() {
    type Case = (
        &'static str,
        std::ops::RangeInclusive<usize>,
        fn(&mut electro5::Program),
        fn(&electro5::Program),
    );

    let cases: Vec<Case> = vec![
        (
            "center_panel.gain",
            0x2e..=0x34,
            |p| p.schema.center_panel.gain = 96u8.try_into().unwrap(),
            |p| assert_eq!(p.schema.center_panel.gain, 96),
        ),
        (
            "piano_panel.touch",
            0x3a..=0x41,
            |p| p.schema.piano_panel.touch = 3u8.try_into().unwrap(),
            |p| assert_eq!(p.schema.piano_panel.touch, 3),
        ),
        (
            "sample_panel.number",
            0x46..=0x4d,
            |p| p.schema.sample_panel.number = 211,
            |p| assert_eq!(p.schema.sample_panel.number, 211),
        ),
        (
            "organ_panel.b3_perc_third",
            0x4e..=0x92,
            |p| p.schema.organ_panel.set_b3_perc_third(true),
            |p| assert!(p.schema.organ_panel.b3_perc_third()),
        ),
        (
            "effects_panel.fx3_compression",
            0x93..=0x9f,
            |p| p.schema.effects_panel.fx3_compression = 101u8.try_into().unwrap(),
            |p| assert_eq!(p.schema.effects_panel.fx3_compression, 101),
        ),
        (
            // Three bits in 0x9a, four in 0x9b.
            "effects_panel.equalizer_freq_gain",
            0x93..=0x9f,
            |p| p.schema.effects_panel.equalizer_freq_gain = 0x55u8.try_into().unwrap(),
            |p| assert_eq!(p.schema.effects_panel.equalizer_freq_gain, 0x55),
        ),
        (
            // Five bits in 0x9e, two in 0x9f.
            "effects_panel.fx5_moisture",
            0x93..=0x9f,
            |p| p.schema.effects_panel.fx5_moisture = 0x2au8.try_into().unwrap(),
            |p| assert_eq!(p.schema.effects_panel.fx5_moisture, 0x2a),
        ),
        (
            "effects_panel.equalizer_part",
            0xa1..=0xa4,
            |p| p.schema.effects_panel.equalizer_part = EqualizerPart::Both,
            |p| assert_eq!(p.schema.effects_panel.equalizer_part, EqualizerPart::Both),
        ),
    ];

    let paths = ne5p_files(&corpus_dir().join("programs"));
    assert!(
        !paths.is_empty(),
        "no programs found — is the corpus present?"
    );

    for path in &paths {
        let name = path.display().to_string();
        let original = read(path).unwrap();

        for (field, span, mutate, check) in &cases {
            let mut program = read_program_checked(path);
            mutate(&mut program);

            let mut mutated: Vec<u8> = Vec::new();
            program.write_to(&mut Cursor::new(&mut mutated)).unwrap();

            for (at, (before, after)) in original.iter().zip(&mutated).enumerate() {
                let allowed = span.contains(&at) || (0x18..=0x1b).contains(&at);
                assert!(
                    allowed || before == after,
                    "setting {field} changed byte {at:#04x} of {name}",
                );
            }

            let Entity::Program(nord_format::Program::Electro5(reread)) =
                nord_format::from_stream(&mut Cursor::new(&mut mutated)).unwrap()
            else {
                panic!("expected an electro5 program in {name}")
            };
            check(&reread);
        }
    }
}

/// Reading nine drawbar nibbles and writing them back is a no-op.
#[test]
fn test_ne5_organ_drawbars_survive_a_rewrite() {
    use electro5::OrganModel::*;

    for path in ne5p_files(&corpus_dir().join("programs")) {
        let name = path.display().to_string();
        let original = read(&path).unwrap();
        let mut program = read_program_checked(&path);

        let organ = &mut program.schema.organ_panel;
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
            original.as_slice(),
            rewritten.as_slice(),
            "rewriting the drawbars changed {name}",
        );
    }
}

/// No specimen decodes to an `Unknown` in a sparse component.
///
/// The decoder preserves unrecognized values rather than refusing them, so this is where
/// they get noticed. A failure means a specimen produced a value with no name yet — go
/// name it.
#[test]
fn test_ne5_no_corpus_program_holds_an_unrecognized_value() {
    let mut paths = ne5p_files(&corpus_dir().join("programs"));
    paths.extend(ne5p_files(
        &corpus_dir().join("usb/backup/full_backup/contents/Program"),
    ));
    assert!(
        !paths.is_empty(),
        "no programs found — is the corpus present?"
    );

    let mut unknowns: BTreeMap<&'static str, BTreeSet<u64>> = BTreeMap::new();

    for path in &paths {
        let Entity::Program(nord_format::Program::Electro5(p)) =
            nord_format::from_path(path.display().to_string().as_str()).unwrap()
        else {
            continue;
        };
        let (c, pi, fx) = (
            &p.schema.center_panel,
            &p.schema.piano_panel,
            &p.schema.effects_panel,
        );

        let mut check = |field: &'static str, unknown: bool, raw: u64| {
            if unknown {
                unknowns.entry(field).or_default().insert(raw);
            }
        };
        check(
            "center.organ_type",
            c.organ_type.is_unknown(),
            c.organ_type.raw().into(),
        );
        check(
            "piano.category",
            pi.category.is_unknown(),
            pi.category.raw().into(),
        );
        check(
            "fx.fx1_type",
            fx.fx1_type.is_unknown(),
            fx.fx1_type.raw().into(),
        );
        check(
            "fx.fx2_type",
            fx.fx2_type.is_unknown(),
            fx.fx2_type.raw().into(),
        );
        check(
            "fx.fx3_type",
            fx.fx3_type.is_unknown(),
            fx.fx3_type.raw().into(),
        );
        check(
            "fx.fx5_type",
            fx.fx5_type.is_unknown(),
            fx.fx5_type.raw().into(),
        );
        check(
            "effects_panel.equalizer_part",
            fx.equalizer_part.is_unknown(),
            fx.equalizer_part.raw().into(),
        );
    }

    assert!(
        unknowns.is_empty(),
        "corpus holds values no component can name — worth investigating, not \
         suppressing: {unknowns:?}",
    );
}
