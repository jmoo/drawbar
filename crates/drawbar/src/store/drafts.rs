//! What an asset's unsaved edit holds, as [`super::unsaved`] keeps it: the bytes of an
//! asset held whole, or the edit an editor holds of a piano or sample instrument resting
//! in its file.

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
    /// A draft of `bytes` as it is kept: the line that says its kind, then the bytes.
    pub fn encode_bytes(bytes: &[u8]) -> Vec<u8> {
        [BYTES, bytes].concat()
    }

    /// A draft of `edit` as it is kept: the line that says its kind, then the edit as
    /// its working copy.
    pub fn encode_edit(edit: &Edit) -> Result<Vec<u8>, String> {
        Ok([EDIT, &edit.working()?].concat())
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
        let bytes = b"CBIN\nbytes\n".to_vec();
        let kept = Draft::encode_bytes(&bytes);
        assert_eq!(Draft::decode(&kept), Ok(Draft::Bytes(bytes)));
        let edit = Edit::Sample(vec![("name".into(), "Vibes".into())]);
        let kept = Draft::encode_edit(&edit).unwrap();
        assert_eq!(Draft::decode(&kept), Ok(Draft::Edit(edit)));
        assert!(Draft::decode(b"CBIN").is_err());
    }
}
