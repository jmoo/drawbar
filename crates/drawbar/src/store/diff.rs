//! Which file on disk is which asset, after changes drawbar did not make.
//!
//! A file is its asset where drawbar last knew it. It holds what drawbar knew while its
//! length and time are the ones drawbar took, and otherwise only where its CRC says so. A
//! file at a path drawbar never knew is the asset whose file went missing when their
//! lengths and CRCs agree, one to one; that is a rename made outside, and the asset's id,
//! tags and origin follow it. A file whose CRC was never taken is matched by path alone.

use std::collections::BTreeMap;

use super::{Fingerprint, Found, LibPath};

/// What drawbar last knew of one asset's file.
pub struct Known {
    pub path: LibPath,
    pub fingerprint: Option<Fingerprint>,
}

/// Where each file landed.
#[derive(Default)]
pub struct Matched {
    /// At the path drawbar knew, holding what it knew.
    pub same: Vec<(u64, Found)>,
    /// At the path drawbar knew, holding something else.
    pub changed: Vec<(u64, Found)>,
    /// At a path drawbar did not know, holding what the asset's missing file held.
    pub renamed: Vec<(u64, Found)>,
    /// Ids whose file is nowhere.
    pub vanished: Vec<u64>,
    /// Files drawbar did not know.
    pub arrived: Vec<Found>,
}

/// Whether a file at the path drawbar knew holds what `known` says it did: its
/// [`super::Stat`] is the known one, or else its CRC is. A file whose [`super::Stat`] moved
/// and whose CRC was not taken is not known to.
pub fn same(known: Option<Fingerprint>, found: &Found) -> bool {
    let Some(print) = known else {
        return false;
    };
    if print.stat() == found.stat {
        return true;
    }
    print.contents().is_some() && print.contents() == found.fingerprint().contents()
}

/// What drawbar knows of a file found where it was: its fingerprint, keeping the CRC known
/// before where the contents are the same.
pub fn kept(known: Option<Fingerprint>, found: &Found) -> Fingerprint {
    let print = found.fingerprint();
    match (same(known, found), known) {
        (true, Some(known)) => Fingerprint {
            crc: print.crc.or(known.crc),
            ..print
        },
        _ => print,
    }
}

pub fn match_files(known: &BTreeMap<u64, Known>, files: Vec<Found>) -> Matched {
    let mut by_path: BTreeMap<&LibPath, u64> = BTreeMap::new();
    for (id, held) in known {
        by_path.entry(&held.path).or_insert(*id);
    }
    let mut matched = Matched::default();
    let mut claimed = std::collections::BTreeSet::new();
    let mut strangers = Vec::new();
    for found in files {
        let Some(id) = by_path.get(&found.path).copied() else {
            strangers.push(found);
            continue;
        };
        claimed.insert(id);
        match same(known[&id].fingerprint, &found) {
            true => matched.same.push((id, found)),
            false => matched.changed.push((id, found)),
        }
    }

    // Contents, as length and CRC, to the strangers and the missing ids holding them.
    type Contents = (u64, u32);
    let mut strangers_by: BTreeMap<Contents, Vec<usize>> = BTreeMap::new();
    for (at, found) in strangers.iter().enumerate() {
        if let Some(contents) = found.fingerprint().contents() {
            strangers_by.entry(contents).or_default().push(at);
        }
    }
    let mut missing_by: BTreeMap<Contents, Vec<u64>> = BTreeMap::new();
    for (id, held) in known.iter().filter(|(id, _)| !claimed.contains(*id)) {
        match held.fingerprint.and_then(|print| print.contents()) {
            Some(contents) => missing_by.entry(contents).or_default().push(*id),
            None => matched.vanished.push(*id),
        }
    }
    let mut strangers: Vec<Option<Found>> = strangers.into_iter().map(Some).collect();
    for (contents, ids) in missing_by {
        let found = match (
            ids.as_slice(),
            strangers_by.get(&contents).map(Vec::as_slice),
        ) {
            ([id], Some([at])) => strangers[*at].take().map(|found| (*id, found)),
            _ => None,
        };
        match found {
            Some(renamed) => matched.renamed.push(renamed),
            None => matched.vanished.extend(ids),
        }
    }
    matched.vanished.sort_unstable();
    matched.arrived = strangers.into_iter().flatten().collect();
    matched
}
