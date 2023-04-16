use crate::common::bank;

pub const BANK_COUNT: u16 = 8;
pub const BANK_SIZE: u16 = 50;

pub type Coordinates = bank::Coordinates<BANK_COUNT, BANK_SIZE>;