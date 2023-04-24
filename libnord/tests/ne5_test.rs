use libnord::common::bank::Item;
use libnord::common::song::Song;
use libnord::{electro5, Entity};
use std::fs::read;
use std::io::Cursor;
use libnord::electro5::program::{Instrument, SplitPoint};

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
        _ => panic!("Expected Electro5 song"),
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
        _ => panic!("Expected Electro5 song"),
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
        _ => panic!("Expected Electro5 song"),
    }
}

#[test]
fn test_ne5_read_write_new_song() {
    let mut song = electro5::Song::new(
        (0, 1).into(),
        (1, 2).into(),
        (2, 3).into(),
        (3, 4).into(),
        (4, 5).into(),
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
}

#[test]
fn test_ne5_update_song_program() {
    let mut song = electro5::Song::new(
        (0, 1).into(),
        (1, 2).into(),
        (2, 3).into(),
        (3, 4).into(),
        (4, 5).into(),
    );

    // Update program 1
    song.set(1, (5, 20).into());

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
}

#[test]
fn test_ne5_read_program() {
    const TEST_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/ne5/programs/",
    "o00_1_p00_0_0_0.ne5p"
    );

    let program = libnord::from_path(TEST_FILE.clone()).unwrap();

    match program {
        Entity::Program(libnord::Program::Electro5(program)) => {
            let program = program as electro5::Program;
            let coords = program.location();

            assert_eq!(coords, (7, 3));
            assert_eq!(program.left_part(), Instrument::Organ);
            assert_eq!(program.right_part(), Instrument::Piano);
            assert_eq!(program.left_octave_shift(), 1);
            assert_eq!(program.right_octave_shift(), 0);
            assert_eq!(program.left_sustain(), false);
            assert_eq!(program.right_sustain(), false);
            assert_eq!(program.left_control(), false);
            assert_eq!(program.right_control(), false);
            assert_eq!(program.split(), false);
            assert_eq!(program.split_point(), SplitPoint::F4);
        }
        _ => panic!("Expected Electro5 program"),
    }
}

#[test]
fn test_ne5_read_write_program() {
    const TEST_FILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/resources/ne5/programs/",
        "o00_1_p00_0_0_0.ne5p"
    );

    let read_contents = read(TEST_FILE.clone()).unwrap();
    let program = libnord::from_path(TEST_FILE.clone()).unwrap();

    match program {
        Entity::Program(libnord::Program::Electro5(mut program)) => {
            let mut write_contents: Vec<u8> = Vec::new();

            program.write_to(&mut Cursor::new(&mut write_contents)).unwrap();

            assert_eq!(read_contents.as_slice(), write_contents.as_slice());
        }
        _ => panic!("Expected Electro5 program"),
    }
}

#[test]
fn test_ne5_read_settings() {
    const TEST_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/ne5/",
    "settings.ne5s"
    );

    let program = libnord::from_path(TEST_FILE.clone()).unwrap();

    match program {
        Entity::Settings(libnord::Settings::Electro5(settings)) => {
            let settings = settings as electro5::Settings;
        }
        _ => panic!("Expected Electro5 settings"),
    }
}
