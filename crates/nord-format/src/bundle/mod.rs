//! Nord bundles: a stored ZIP of programs and set lists with the pianos and samples
//! they play, and a manifest saying which member needs which.
//!
//! [`archive`] reads and writes the container one member at a time, and [`manifest`]
//! the `meta.xml` it carries. [`Plan`] lays out a new Electro 5 bundle from its
//! members' headers and references.

pub mod archive;
pub mod manifest;

use crate::cbin::Header;
use crate::fields::Library;
use crate::{Entity, Program, Song};
use manifest::{Dependencies, Manifest};

/// The Electro 5 piano banks, which are the panel's categories, by zero-based index.
/// Confirmed on hardware.
pub const ELECTRO5_PIANO_BANKS: [&str; 6] = [
    "Grand", "Upright", "EPiano1", "EPiano2", "Clavinet", "Harps",
];

/// What one member provides to, or needs from, the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// A piano by the id programs know it by.
    Piano(u32),
    /// A sample by the id programs know it by.
    Sample(u32),
    /// A program by its zero-based `(bank, slot)`, which is how a set list names one.
    Program(u16, u16),
}

/// What a member is, from its file's tag. The order is the order NSM stores members in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Piano,
    Sample,
    Program,
    SetList,
}

impl Class {
    /// The class of an Electro 5 file a bundle carries, or `None` for any other.
    pub fn of(header: &Header) -> Option<Class> {
        match &header.tag {
            b"npno" => Some(Class::Piano),
            b"nsmp" => Some(Class::Sample),
            b"ne5p" => Some(Class::Program),
            b"ne5t" => Some(Class::SetList),
            _ => None,
        }
    }
}

/// The archive path NSM gives an Electro 5 file named `name`: its device class, its
/// bank, then the name with its format's extension. `None` for a file a bundle does
/// not carry, a name holding a `/`, or a piano bank past the six.
///
/// Inferred from specimens; not confirmed on hardware. A piano's header names its
/// bank, which is its category, as a program's names its bank.
pub fn electro5_path(header: &Header, name: &str) -> Option<String> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let bank = usize::from(header.slot().0);
    Some(match Class::of(header)? {
        Class::Piano => format!("Piano/{}/{name}.npno", ELECTRO5_PIANO_BANKS.get(bank)?),
        Class::Sample => format!("Samp Lib/Samp Lib/{name}.nsmp"),
        Class::Program => format!("Program/Bank {}/{name}.ne5p", bank + 1),
        Class::SetList => format!("Set List/Set List {}/{name}.ne5t", bank + 1),
    })
}

/// One file going into a new bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub class: Class,
    /// What the others can name this member by.
    pub provides: Option<Key>,
    /// What this member names, in the order its file names them.
    pub needs: Vec<Key>,
}

impl Item {
    /// The item for an Electro 5 file named `name`, or `None` for one a bundle does not
    /// carry. A program provides its slot and needs what it plays; a set list needs its
    /// programs. A piano or sample file does not hold the id programs know it by, so
    /// its item provides nothing until the caller sets that id from the instrument.
    pub fn of(header: &Header, name: &str, entity: Option<&Entity>) -> Option<Item> {
        let class = Class::of(header)?;
        let mut needs = Vec::new();
        let mut provides = None;
        match entity {
            Some(program @ Entity::Program(Program::Electro5(file))) => {
                let (bank, slot) = file.header.slot();
                provides = Some(Key::Program(bank, slot));
                needs.extend(program.plays().unwrap_or_default().into_iter().filter_map(
                    |reference| match reference.library {
                        Library::Piano => Some(Key::Piano(reference.id)),
                        Library::Sample => Some(Key::Sample(reference.id)),
                        Library::Program | Library::SetList => None,
                    },
                ));
            }
            Some(Entity::Song(Song::Electro5(file))) => {
                for program in file.body.programs() {
                    let key = Key::Program(program.x(), program.y());
                    if !needs.contains(&key) {
                        needs.push(key);
                    }
                }
            }
            _ => {}
        }
        Some(Item {
            path: electro5_path(header, name)?,
            class,
            provides,
            needs,
        })
    }
}

/// A new bundle's layout: its members in the order to store them, the manifest that
/// lists their dependencies, and every need no member provides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub members: Vec<Item>,
    pub manifest: Manifest,
    /// Each member path with a need nothing in the bundle provides.
    pub unmet: Vec<(String, Key)>,
}

impl Plan {
    /// Lays out `items` the way NSM does: pianos, samples, programs, then set lists,
    /// each in the order given; the manifest lists set lists first, then programs, and
    /// leaves out a member that needs nothing.
    ///
    /// Inferred from specimens; not confirmed on hardware.
    pub fn new(items: Vec<Item>, product_version: u32) -> Result<Plan, crate::error::ParseError> {
        let mut members = items;
        members.sort_by_key(|item| item.class);
        if let Some(path) = duplicate(members.iter().map(|item| item.path.as_str())) {
            return Err(crate::error::ParseError::AssertFail(format!(
                "two members of the bundle are both {path}"
            )));
        }
        let provider = |key: &Key| {
            members
                .iter()
                .find(|item| item.provides.as_ref() == Some(key))
                .map(|item| item.path.clone())
        };
        let mut unmet = Vec::new();
        let mut files = Vec::new();
        for class in [Class::SetList, Class::Program] {
            for item in members.iter().filter(|item| item.class == class) {
                let mut deps: Vec<String> = Vec::new();
                for key in &item.needs {
                    match provider(key) {
                        Some(path) if !deps.contains(&path) => deps.push(path),
                        Some(_) => {}
                        None => unmet.push((item.path.clone(), *key)),
                    }
                }
                if !deps.is_empty() {
                    files.push(Dependencies {
                        name: item.path.clone(),
                        deps,
                    });
                }
            }
        }
        Ok(Plan {
            manifest: Manifest::electro5(product_version, files),
            members,
            unmet,
        })
    }
}

fn duplicate<'a>(paths: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = std::collections::BTreeSet::new();
    paths.into_iter().find(|path| !seen.insert(*path))
}
