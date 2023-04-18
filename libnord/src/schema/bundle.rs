use crate::schema::Schema;
use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Debug)]
pub struct Bundle {
    pub files: HashMap<String, Schema>,
}

impl Bundle {
    pub fn new() -> Bundle {
        return Bundle {
            files: HashMap::new(),
        };
    }
}

#[derive(Debug)]
pub struct Meta {}
