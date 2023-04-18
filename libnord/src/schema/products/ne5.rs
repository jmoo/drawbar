use crate::schema;

use binrw::{BinRead, BinWrite};

#[derive(BinRead, BinWrite, Debug)]
#[br(assert(data.schema() == "ne5t"))]
#[bw(little)]
pub struct Song {
    data: schema::song::SongV1,
}

impl Song {}

#[derive(BinRead, Debug)]
#[br(assert(data.schema() == "ne5p"))]
pub struct Program {
    data: schema::program::Program,
}

impl Program {}

#[derive(BinRead, Debug)]
#[br(assert(data.schema() == "ne5l"))]
pub struct Live {
    data: schema::live::Live,
}

impl Live {}

#[derive(BinRead, Debug)]
#[br(assert(data.schema() == "ne5s"))]
pub struct Settings {
    data: schema::settings::Settings,
}

impl Settings {}
