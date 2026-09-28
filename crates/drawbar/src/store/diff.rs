//! Which file on disk is which asset, after changes drawbar did not make.
//!
//! A file is its asset where drawbar last knew it. A file at a path drawbar never knew is
//! the asset whose file went missing when their contents agree, one to one; that is a
//! rename made outside, and the asset's id, tags and origin follow it.

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
    /// At the path drawbar knew, holding what it knew. A file whose [`super::Stat`] was
    /// unchanged comes without its bytes.
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
        let same = match found.contents() {
            None => true,
            Some(contents) => known[&id]
                .fingerprint
                .is_some_and(|print| (print.len, print.crc) == contents),
        };
        match same {
            true => matched.same.push((id, found)),
            false => matched.changed.push((id, found)),
        }
    }

    // Contents, as length and CRC, to the strangers and the missing ids holding them.
    type Contents = (u64, u32);
    let mut strangers_by: BTreeMap<Contents, Vec<usize>> = BTreeMap::new();
    for (at, found) in strangers.iter().enumerate() {
        if let Some(contents) = found.contents() {
            strangers_by.entry(contents).or_default().push(at);
        }
    }
    let mut missing_by: BTreeMap<Contents, Vec<u64>> = BTreeMap::new();
    for (id, held) in known.iter().filter(|(id, _)| !claimed.contains(*id)) {
        match held.fingerprint {
            Some(print) => missing_by
                .entry((print.len, print.crc))
                .or_default()
                .push(*id),
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
