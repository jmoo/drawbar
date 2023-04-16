use libnord::common::song::Song;
use libnord::{electro5, Entity};
use std::fs::read;
use std::io::Cursor;
use libnord::common::bank::Item;

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
            let coords = song.location();

            assert_eq!(coords, (0, 2));
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
            assert_eq!(song.get(0), (5, 9));
            assert_eq!(song.get(1), (0, 1));
            assert_eq!(song.get(2), (0, 2));
            assert_eq!(song.get(3), (5, 8));
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