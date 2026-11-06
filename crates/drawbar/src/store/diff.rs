//! Whether a file drawbar read again holds what drawbar knew of it.
//!
//! It does while its length and time are the ones drawbar took, and otherwise only where
//! its CRC says so.

use super::{Fingerprint, Found};

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
