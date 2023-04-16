use crate::schema::bundle::{Bundle, Meta};
use binrw::{BinRead, BinReaderExt, BinResult, BinWrite};
use byteorder::ReadBytesExt;

use std::fmt::Debug;
use std::io::{Cursor, Read};

pub mod bundle;
pub mod header;
pub mod live;
pub mod piano;
pub mod products;
pub mod program;
pub mod sample;
pub mod settings;
pub mod song;

#[derive(Debug)]
pub enum Schema {
    Meta(Meta),
    Sample(sample::Sample),
    Piano(piano::Piano),
    Bundle(Bundle),
    NE5Song(products::ne5::Song),
    NE5Program(products::ne5::Program),
    NE5Live(products::ne5::Live),
    NE5Settings(products::ne5::Settings),
}

trait Writer: std::io::Write + std::io::Seek {}

pub(crate) trait Reader: std::io::Read + std::io::Seek {}

impl Schema {
    pub fn read(reader: &mut dyn Read) -> Schema {
        let mut buffer: Vec<u8> = Vec::new();

        let head = reader.read_u8().unwrap();
        buffer.push(head);

        match head {
            // Parse bundle and backup files
            0x50 => {
                reader.read_to_end(&mut buffer).unwrap();
                let cursor = std::io::Cursor::new(buffer);
                let mut zip = zip::ZipArchive::new(cursor).unwrap();
                let mut bundle = Bundle::new();

                for i in 0..zip.len() {
                    let mut file = zip.by_index(i).unwrap();
                    let mut buffer: Vec<u8> = Vec::new();
                    file.read_to_end(&mut buffer).unwrap();
                    let mut cursor = std::io::Cursor::new(buffer);
                    let schema = Schema::read(&mut cursor);
                    bundle.files.insert(file.name().to_string(), schema);
                }

                return Schema::Bundle(bundle);
            }

            // Parse meta.xml
            0x3c => {
                return Schema::Meta(Meta {});
            }

            // Parse cbin files
            0x43 => {
                for _i in 0..11 {
                    buffer.push(reader.read_u8().unwrap());
                }

                let preamble: header::Preamble =
                    (std::io::Cursor::new(&mut buffer)).read_be().unwrap();
                let schema = preamble.schema();

                reader.read_to_end(&mut buffer).unwrap();

                let mut cursor = std::io::Cursor::new(buffer);

                match schema {
                    "nsmp" => Schema::Sample(cursor.read_be::<sample::Sample>().unwrap()),
                    "npno" => Schema::Piano(cursor.read_be::<piano::Piano>().unwrap()),
                    "ne5p" => {
                        Schema::NE5Program(cursor.read_be::<products::ne5::Program>().unwrap())
                    }
                    "ne5t" => Schema::NE5Song(cursor.read_be::<products::ne5::Song>().unwrap()),
                    "ne5l" => Schema::NE5Live(cursor.read_be::<products::ne5::Live>().unwrap()),
                    "ne5s" => {
                        Schema::NE5Settings(cursor.read_be::<products::ne5::Settings>().unwrap())
                    }
                    _ => panic!("Unknown schema: {}", schema),
                }
            }
            _ => panic!("Unknown header: {}", head),
        }
    }

    pub fn write(&self, buffer: &mut dyn std::io::Write) -> BinResult<()> {
        let _stream = Cursor::new(buffer);
        let mut writer = Cursor::new(Vec::new());

        match self {
            Schema::NE5Song(song) => song.write(&mut writer),
            _ => panic!("ahhhh"),
        }
    }
}
