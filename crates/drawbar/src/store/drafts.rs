//! An unsaved edit as toshokan keeps it, in this install's own state for the library:
//! the bytes of an asset held whole, or the edit an editor holds of a piano or sample
//! instrument resting in its file.

use crate::rewrite::Edit;

/// What a draft holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Draft {
    Bytes(Vec<u8>),
    Edit(Edit),
}

/// The line a draft of each kind starts with.
const BYTES: &[u8] = b"bytes\n";
const EDIT: &[u8] = b"edit\n";

impl Draft {
    /// The draft's bytes as toshokan keeps them: the line that says its kind, then what
    /// it holds, an edit as its working copy.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let (kind, held) = match self {
            Draft::Bytes(bytes) => (BYTES, bytes.clone()),
            Draft::Edit(edit) => (EDIT, edit.working()?),
        };
        Ok([kind, &held].concat())
    }

    /// The draft `bytes` keep, or why they do not read as one.
    pub fn decode(bytes: &[u8]) -> Result<Draft, String> {
        if let Some(held) = bytes.strip_prefix(BYTES) {
            return Ok(Draft::Bytes(held.to_vec()));
        }
        match bytes.strip_prefix(EDIT) {
            Some(working) => Edit::from_working(working).map(Draft::Edit),
            None => Err("it is not a draft this build reads".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_reads_back_as_it_was_kept() {
        let edit = Edit::Sample(vec![("name".into(), "Vibes".into())]);
        for draft in [Draft::Bytes(b"CBIN\nbytes\n".to_vec()), Draft::Edit(edit)] {
            let kept = draft.encode().unwrap();
            assert_eq!(Draft::decode(&kept), Ok(draft));
        }
        assert!(Draft::decode(b"CBIN").is_err());
    }
}
