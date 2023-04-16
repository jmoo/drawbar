
use libnord::common::song::Song;
use libnord::{electro5, Entity};
use std::fs::read;
use std::io::Cursor;

#[test]
fn test_ne5_read_song_bank() {
    const TEST_FILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/resources/ne5/",
        "song_0610_0102_0103_0609.ne5t"
    );

    let song = libnord::from_path(TEST_FILE.clone()).unwrap();

    match song {
        Entity::Song(libnord::Song::Electro5(song)) => {
            let song = song as electro5::Song;

            assert_eq!(song.location().bank(), 0);
            assert_eq!(song.location().slot(), 2);
        }
    }
}

#[test]
fn test_ne5_read_song_programs() {
    const TEST_FILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/resources/ne5/",
        "song_0610_0102_0103_0609.ne5t"
    );

    let song = libnord::from_path(TEST_FILE.clone()).unwrap();

    match song {
        Entity::Song(libnord::Song::Electro5(song)) => {
            let programs = (song as electro5::Song).programs();

            assert_eq!(programs.len(), 4);

            assert_eq!(programs[0].bank(), 5);
            assert_eq!(programs[0].slot(), 9);

            assert_eq!(programs[1].bank(), 0);
            assert_eq!(programs[1].slot(), 1);

            assert_eq!(programs[2].bank(), 0);
            assert_eq!(programs[2].slot(), 2);

            assert_eq!(programs[3].bank(), 5);
            assert_eq!(programs[3].slot(), 8);
        }
    }
}

#[test]
fn test_ne5_write_song() {
    const TEST_FILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/resources/ne5/",
        "song_0610_0102_0103_0609.ne5t"
    );

    let song = libnord::from_path(TEST_FILE.clone()).unwrap();
    let contents = read(TEST_FILE.clone()).unwrap();

    match song {
        Entity::Song(libnord::Song::Electro5(mut song)) => {
            let mut output = Vec::new();

            song.write_to(&mut Cursor::new(&mut output)).unwrap();

            assert_eq!(contents.as_slice(), output.as_slice());
        }
    }
}

#[test]
fn test_ne5_read_write_new_song() {
    let mut song = electro5::Song::new((1, 2), (3, 4), (5, 6), (7, 8), (9, 10));

    let song_location = song.location();
    let song_programs = song.programs();

    // Assert song was created with correct values
    assert_eq!(song_location.bank(), 1);
    assert_eq!(song_location.slot(), 2);
    assert_eq!(song_programs.len(), 4);
    assert_eq!(song_programs[0].bank(), 3);
    assert_eq!(song_programs[0].slot(), 4);
    assert_eq!(song_programs[1].bank(), 5);
    assert_eq!(song_programs[1].slot(), 6);
    assert_eq!(song_programs[2].bank(), 7);
    assert_eq!(song_programs[2].slot(), 8);
    assert_eq!(song_programs[3].bank(), 9);
    assert_eq!(song_programs[3].slot(), 10);

    // Read/Write song to result
    let mut write_result = Vec::new();
    song.write_to(&mut Cursor::new(&mut write_result)).unwrap();

    let result = electro5::Song::read_from(&mut Cursor::new(&mut write_result)).unwrap();
    let result_location = result.location();
    let result_programs = result.programs();

    // Assert those values are the same after writing and reading
    assert_eq!(song_location.value(), result_location.value());
    assert_eq!(result_programs.len(), 4);
    assert_eq!(song_programs[0].value(), result_programs[0].value());
    assert_eq!(song_programs[1].value(), result_programs[1].value());
    assert_eq!(song_programs[2].value(), result_programs[2].value());
    assert_eq!(song_programs[3].value(), result_programs[3].value());
}
