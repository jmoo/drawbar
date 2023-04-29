use std::fs;
use libnord::common::bank::Item;

use libnord::electro5::{Instrument, SplitPoint};
use libnord::error::Error;
use libnord::{electro5, Entity};
use std::fs::read;
use std::io::Cursor;
use std::str::FromStr;
use regex::Regex;
use libnord::common::PartMix;

#[test]
fn test_ne5_read_song_bank() {
    const TEST_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/", "song.ne5t");

    let song = libnord::from_path(TEST_FILE.clone()).unwrap();

    match song {
        Entity::Song(libnord::Song::Electro5(song)) => {
            let song = song as electro5::Song;
            let coords = song.location();

            assert_eq!(coords, (0, 2));
        }
        _ => panic!("expected electro5 song"),
    }
}

#[test]
fn test_ne5_read_song_programs() {
    const TEST_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/", "song.ne5t");

    let song = libnord::from_path(TEST_FILE.clone()).unwrap();

    match song {
        Entity::Song(libnord::Song::Electro5(song)) => {
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
    const TEST_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/", "song.ne5t");

    let song = libnord::from_path(TEST_FILE.clone()).unwrap();
    let contents = read(TEST_FILE.clone()).unwrap();

    match song {
        Entity::Song(libnord::Song::Electro5(mut song)) => {
            let mut output: Vec<u8> = Vec::new();

            song.write_to(&mut Cursor::new(&mut output)).unwrap();

            assert_eq!(contents.as_slice(), output.as_slice());
        }
        _ => panic!("expected electro5 song"),
    }
}

#[test]
fn test_ne5_read_write_new_song() -> Result<(), Error> {
    let mut song = electro5::Song::new(
        (0, 1).try_into()?,
        [
            (1, 2).try_into()?,
            (2, 3).try_into()?,
            (3, 4).try_into()?,
            (4, 5).try_into()?,
        ],
    );

    // Assert song was created with correct values
    assert_eq!(song.location(), (0, 1));
    assert_eq!(song.get(0), (1, 2));
    assert_eq!(song.get(1), (2, 3));
    assert_eq!(song.get(2), (3, 4));
    assert_eq!(song.get(3), (4, 5));

    // Read/Write song to result
    let mut write_result = Vec::new();
    song.write_to(&mut Cursor::new(&mut write_result)).unwrap();

    let result = electro5::Song::read_from(&mut Cursor::new(&mut write_result)).unwrap();

    // Assert those values are the same after writing and reading
    assert_eq!(song.location(), result.location());
    assert_eq!(song.get(0), result.get(0));
    assert_eq!(song.get(1), result.get(1));
    assert_eq!(song.get(2), result.get(2));
    assert_eq!(song.get(3), result.get(3));

    Ok(())
}

#[test]
fn test_ne5_update_song_program() -> Result<(), Error> {
    let mut song = electro5::Song::new(
        (0, 1).try_into()?,
        [
            (1, 2).try_into()?,
            (2, 3).try_into()?,
            (3, 4).try_into()?,
            (4, 5).try_into()?,
        ],
    );

    // Update program 1
    song.set(1, (5, 20).try_into()?);

    // Assert song was updated with correct values
    assert_eq!(song.location(), (0, 1));
    assert_eq!(song.get(0), (1, 2));
    assert_eq!(song.get(1), (5, 20));
    assert_eq!(song.get(2), (3, 4));
    assert_eq!(song.get(3), (4, 5));

    // Read/Write song to result
    let mut write_result = Vec::new();
    song.write_to(&mut Cursor::new(&mut write_result)).unwrap();

    let result = electro5::Song::read_from(&mut Cursor::new(&mut write_result)).unwrap();

    // Assert those values are the same after writing and reading
    assert_eq!(song.location(), result.location());
    assert_eq!(song.get(0), result.get(0));
    assert_eq!(song.get(1), result.get(1));
    assert_eq!(song.get(2), result.get(2));
    assert_eq!(song.get(3), result.get(3));

    Ok(())
}

#[test]
fn test_ne5_read_program() {
    const TEST_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/programs/center_panel/o00_1_p00_0_0_0_50_50.ne5p");

    let program = libnord::from_path(TEST_FILE.clone()).unwrap();

    match program {
        Entity::Program(libnord::Program::Electro5(program)) => {
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
    const TEST_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/programs/center_panel/o00_1_p00_0_0_0_50_50.ne5p");

    let read_contents = read(TEST_FILE.clone()).unwrap();
    let program = libnord::from_path(TEST_FILE.clone()).unwrap();

    match program {
        Entity::Program(libnord::Program::Electro5(mut program)) => {
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
    const TEST_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/settings.ne5s");

    let program = libnord::from_path(TEST_FILE.clone()).unwrap();

    match program {
        Entity::Settings(libnord::Settings::Electro5(settings)) => {
            let _settings = settings as electro5::Settings;
        }
        _ => panic!("expected electro5 settings"),
    }
}

#[test]
fn test_ne5_program_read_write_center_panel() {
    const TEST_FILES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/programs/center_panel");

    let paths = fs::read_dir(TEST_FILES.clone()).unwrap();

    let center_panel_re = Regex::new(r"([osp])([0-9])([0-9])_([0-9.-]+)_([osp])([0-9])([0-9])_([0-9.-]+)_([0-9.-]+)_([0-9.-]+)_([0-9.-]+)_([0-9.-]+)[.](skip[.])?ne5p$").unwrap();

    for path in paths {
        let inner = path.unwrap();

        if !inner.metadata().unwrap().is_file() {
            continue;
        }

        let path = inner.path().display().to_string();

        if let Some(matches) = center_panel_re.captures(path.as_str()) {
            let program = libnord::from_path(path.as_str()).unwrap();
            let contents = read(path.as_str()).unwrap();

            let lower_instrument = match matches.get(1).unwrap().as_str() {
                "o" => Instrument::Organ,
                "s" => Instrument::Sample,
                "p" => Instrument::Piano,
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
                "o" => Instrument::Organ,
                "s" => Instrument::Sample,
                "p" => Instrument::Piano,
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

            let upper_octave_shift = i8::from_str(matches.get(8).unwrap().as_str()).unwrap();
            let transpose = i8::from_str(matches.get(9).unwrap().as_str()).unwrap();
            let split = u8::from_str(matches.get(10).unwrap().as_str()).unwrap();

            let part_mix = (
                f32::from_str(matches.get(11).unwrap().as_str()).unwrap(),
                f32::from_str(matches.get(12).unwrap().as_str()).unwrap()
            );

            if let Some(skip) = matches.get(13) {
                continue;
            };

            match program {
                Entity::Program(libnord::Program::Electro5(mut program)) => {
                    println!(
                        "test test_ne5_program_read_write_center_panel: {} (\n  \
                            lower_ins:\t{:?}\n  \
                            lower_sus:\t{}\n  \
                            lower_ctr:\t{}\n  \
                            lower_oct:\t{}\n  \
                            upper_ins:\t{:?}\n  \
                            upper_sus:\t{}\n  \
                            upper_ctr:\t{}\n  \
                            upper_oct:\t{}\n  \
                            transpose:\t{}\n  \
                            split_pnt:\t{}\n  \
                            part_mix:\t{:?}\n\
                        )",
                        path,
                        lower_instrument,
                        lower_sustain,
                        lower_control,
                        lower_octave_shift,
                        upper_instrument,
                        upper_sustain,
                        upper_control,
                        upper_octave_shift,
                        transpose,
                        split,
                        part_mix
                    );

                    let mut output: Vec<u8> = Vec::new();
                    program.write_to(&mut Cursor::new(&mut output)).unwrap();

                    assert_eq!(contents.as_slice(), output.as_slice(), "read/write mismatch in file {}", path);
                    assert_eq!(program.lower_part(), lower_instrument, "lower instrument mismatch in file {}", path);
                    assert_eq!(program.upper_part(), upper_instrument, "upper instrument mismatch in file {}", path);
                    assert_eq!(program.lower_octave_shift(), lower_octave_shift, "lower octave shift mismatch in file {}", path);
                    assert_eq!(program.upper_octave_shift(), upper_octave_shift, "upper octave shift mismatch in file {}", path);
                    assert_eq!(program.lower_sustain(), lower_sustain, "lower sustain mismatch in file {}", path);
                    assert_eq!(program.upper_sustain(), upper_sustain, "upper sustain mismatch in file {}", path);
                    assert_eq!(program.lower_control(), lower_control, "lower control mismatch in file {}", path);
                    assert_eq!(program.upper_control(), upper_control, "upper control mismatch in file {}", path);
                    assert_eq!(program.split(), split != 0, "split enabled mismatch in file {}", path);
                    assert_eq!(program.transpose_enabled(), transpose != 0, "transpose enabled mismatch in file {}", path);
                    assert_eq!(program.part_mix().lower().round(), part_mix.0.round(), "lower part mix mismatch in file {}", path);
                    assert_eq!(program.part_mix().upper().round(), part_mix.1.round(), "upper part mix mismatch in file {}", path);

                    if transpose != 0 {
                        assert_eq!(program.transpose(), transpose, "transpose mismatch in file {}", path);
                    }

                    if split != 0 {
                        assert_eq!(program.split_point() as u8, split - 1, "split point mismatch in file {}", path);
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
    const TEST_FILES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/programs/gain");

    let paths = fs::read_dir(TEST_FILES.clone()).unwrap();

    let gain_re = Regex::new(r"([0-9.-]+)[.](skip[.])?ne5p$").unwrap();

    for path in paths {
        let inner = path.unwrap();

        if !inner.metadata().unwrap().is_file() {
            continue;
        }

        let path = inner.path().display().to_string();

        if let Some(matches) = gain_re.captures(path.as_str()) {
            let program = libnord::from_path(path.as_str()).unwrap();
            let contents = read(path.as_str()).unwrap();

            let gain = f32::from_str(matches.get(1).unwrap().as_str()).unwrap();

            if let Some(skip) = matches.get(3) {
                continue;
            };

            match program {
                Entity::Program(libnord::Program::Electro5(mut program)) => {
                    println!("test test_ne5_program_read_write_gain: {} (\n  gain:\t{}\n)", path, gain);

                    let mut output: Vec<u8> = Vec::new();
                    program.write_to(&mut Cursor::new(&mut output)).unwrap();

                    assert_eq!(contents.as_slice(), output.as_slice(), "read/write mismatch in file {}", path);
                    assert_eq!(program.gain(), ((gain / 10_f32) * 127_f32).round() as u8, "gain mismatch in file {}", path);
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
    const TEST_FILES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/programs/fx");

    let paths = fs::read_dir(TEST_FILES.clone()).unwrap();

    let gain_re = Regex::new(r"fx([0-9])_([0-9])([0-9])([0-9])_([0-9.-]+)_?([0-9.-]+)?[.](skip[.])?ne5p$").unwrap();

    for path in paths {
        let inner = path.unwrap();

        if !inner.metadata().unwrap().is_file() {
            continue;
        }

        let path = inner.path().display().to_string();

        if let Some(matches) = gain_re.captures(path.as_str()) {
            let program = libnord::from_path(path.as_str()).unwrap();
            let contents = read(path.as_str()).unwrap();

            match program {
                Entity::Program(libnord::Program::Electro5(mut program)) => {
                    if let Some(skip) = matches.get(7) {
                        continue;
                    };
                    
                    let fx = u8::from_str(matches.get(1).unwrap().as_str()).unwrap();
                    let part_select = u8::from_str(matches.get(2).unwrap().as_str()).unwrap();
                    let switch_enabled = u8::from_str(matches.get(3).unwrap().as_str()).unwrap();;
                    let fx_type = u8::from_str(matches.get(4).unwrap().as_str()).unwrap();
                    let fx_value = f32::from_str(matches.get(5).unwrap().as_str()).unwrap();

                    let fx_value2 = match matches.get(6) {
                        Some(value) => Some(f32::from_str(value.as_str()).unwrap()),
                        None => None,
                    };

                    print!("test test_ne5_program_read_write_fx{}: {} (\n  part_sel:\t{}  \n", fx, path, part_select);
                    
                    match fx {
                        1 => {
                            println!("  ctrl_on:\t{}", switch_enabled);
                            println!("  fx_type:\t{}", fx_type);
                            println!("  fx_rate:\t{}", fx_value);
                            assert_eq!(program.schema.effects_panel.fx1, part_select + 1, "fx1 part select mismatch in file {}", path);
                            assert_eq!(program.schema.extra.fx1_control, switch_enabled != 0, "fx1 control mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx1_rate, ((fx_value / 10_f32) * 127_f32).floor() as u8, "fx1 rate mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx1_type, match fx_type {
                                0 => 3, // pan 1
                                1 => 4, // pan 2
                                2 => 5, // pan 1&2
                                3 => 6, // wah
                                4 => 7, // rm
                                5 => 0, // trem 1
                                6 => 1, // trem 2
                                7 => 2, // trem 1&2
                                a => panic!("unknown fx1 type {} in file {}", a, path)
                            }, "fx1 type mismatch in file {}", path);
                        },
                        2 => {
                            println!("  deep_on:\t{}", switch_enabled);
                            println!("  fx_type:\t{}", fx_type);
                            println!("  fx_rate:\t{}", fx_value);
                            assert_eq!(program.schema.effects_panel.fx2, part_select + 1, "fx2 part select mismatch in file {}", path);
                            assert_eq!(program.schema.extra.fx2_deep, switch_enabled != 0, "fx2 deep mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx2_rate, fx_value.floor() as u8, "fx2 rate mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx2_type, match fx_type {
                                0 => 2, // flang
                                1 => 3, // choir1
                                2 => 4, // choir2
                                3 => 5, // vibe
                                4 => 0, // phas1
                                5 => 1, // phas2
                                a => panic!("unknown fx2 type {} in file {}", a, path)
                            }, "fx2 type mismatch in file {}", path);
                        },
                        3 => {
                            println!("  drive_on:\t{}", switch_enabled);
                            println!("  fx_type:\t{}", fx_type);
                            println!("  fx_compr:\t{}", fx_value);
                            assert_eq!(program.schema.effects_panel.fx3, part_select + 1, "fx3 part select mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx3_compression as f32, fx_value, "fx3 compression mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx3_compression > 0, switch_enabled != 0, "fx3 drive on mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx3_type, match fx_type {
                                0 => 0, // none
                                1 => 3, // twin
                                2 => 4, // rotary
                                3 => 5, // comp
                                4 => 1, // small
                                5 => 2, // jc
                                a => panic!("unknown fx3 type {} in file {}", a, path)
                            }, "fx3 type mismatch in file {}", path);
                        },
                        4 => {
                            println!("  ping_png:\t{}", switch_enabled);
                            println!("  feedback:\t{}", fx_type);
                            println!("  moisture:\t{}", fx_value);
                            println!("  tempo:\t{:?}", fx_value2.unwrap());
                            assert_eq!(program.schema.effects_panel.fx4, part_select + 1, "fx4 part select mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx4_ping_pong, switch_enabled != 0, "fx4 ping pong mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx4_moisture as f32, ((fx_value / 10_f32) * 127_f32).floor(), "fx4 moisture mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx4_tempo as f32, fx_value2.unwrap().floor(), "fx4 tempo mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx4_feedback, fx_type, "fx4 type mismatch in file {}", path);
                        }
                        5 => {
                            println!("  enabled:\t{}", part_select);
                            println!("  type:\t{}", fx_type);
                            println!("  moisture:\t{}", fx_value);
                            assert_eq!(program.schema.effects_panel.fx5, part_select == 1, "fx5 part select mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx5_moisture as f32, fx_value, "fx5 moisture mismatch in file {}", path);
                            assert_eq!(program.schema.effects_panel.fx5_type, match fx_type {
                                0 => 2, // stage
                                1 => 3, // hall-soft
                                2 => 4, // hall
                                3 => 0, // room
                                4 => 1, // stage-soft
                                a => panic!("unknown fx5 type {} in file {}", a, path)
                            }, "fx5 type mismatch in file {}", path);
                        }
                        _ => panic!("unknown fx {} in file {}", fx, path),
                    }
                    
                    println!(")");

                    let mut output: Vec<u8> = Vec::new();
                    program.write_to(&mut Cursor::new(&mut output)).unwrap();
                    assert_eq!(contents.as_slice(), output.as_slice(), "read/write mismatch in file {}", path);
                }
                _ => panic!("expected electro5 song in file {}", path),
            }
        } else if !path.contains("README.md") {
            panic!("invalid file name: {}", path)
        }
    }
}
