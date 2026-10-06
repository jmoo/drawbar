//! Binding files to entities.
//!
//! A binding is a pure function of the logged facts, the files a reader sees and
//! the app's cheap identity: by path, then by identity, with the copy rule. When an
//! entity's path is gone, a file holding its identity is a move; while the path
//! still holds it, such a file is a copy, a new file without an entity until an
//! intent says something about it. When several files could be the one, nothing is
//! bound and it is reported. Scans never write; every commit pins the bindings this
//! writer holds.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use std::collections::BTreeMap;

use crate::env::Identify;
use crate::error::Result;
use crate::ids::{EntityId, Identity};
use crate::io::Task;
use crate::layout::Layout;
use crate::log::Op;
use crate::merge::Folded;
use crate::path::RelPath;
use crate::report::ScanReport;
use crate::view::FileRef;

/// How the volume compares names: case and Unicode normalization.
pub trait Names {
    fn same(&self, a: &str, b: &str) -> bool;
}

/// Names are equal only byte for byte.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExactNames;

impl Names for ExactNames {
    fn same(&self, a: &str, b: &str) -> bool {
        a == b
    }
}

/// One library file as a scan found it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Scanned {
    pub len: u64,
    pub modified: Option<u64>,
    /// Read only where length and time could not decide.
    pub identity: Option<Identity>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Scan {
    pub files: BTreeMap<RelPath, Scanned>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Bindings {
    pub bound: BTreeMap<EntityId, FileRef>,
    pub unbound: Vec<RelPath>,
    pub report: ScanReport,
}

/// Lists every library file outside toshokan's root. A file is identified only
/// when its length and time match neither `previous` nor what `folded` says of
/// it. Requests only [`crate::Io::List`], [`crate::Io::Stat`] and
/// [`crate::Io::Read`].
pub fn scan<'a>(
    layout: &'a Layout,
    identify: &'a dyn Identify,
    folded: &'a Folded,
    previous: &'a Scan,
) -> Task<'a, Result<Scan>> {
    todo!()
}

pub fn bind(folded: &Folded, scan: &Scan, names: &dyn Names) -> Bindings {
    todo!()
}

/// File-register writes for the bindings that `folded` does not already say.
pub fn pins(folded: &Folded, bindings: &Bindings, scan: &Scan) -> Vec<Op> {
    todo!()
}
