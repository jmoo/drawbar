//! The `cat` section: the browser categories an instrument is filed under.
//!
//! The narrow payload opens with five one-byte categories and follows them with two
//! length-prefixed labels, padded to a whole word. The wide payload is eight bytes and
//! keeps only the first two categories. The project's `m_category…` fields render
//! into these bytes in this order. Inferred from specimens; not confirmed on hardware.

use crate::error::ParseError;

/// Bytes a wide payload holds.
pub const WIDE_LEN: usize = 8;

/// Where the narrow payload's labels start, behind its five categories.
const LABELS_AT: usize = 5;

/// The narrow payload's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarrowCat {
    pub category: u8,
    pub sub_category: u8,
    pub timbre: u8,
    pub envelope: u8,
    pub motion: u8,
    pub production: String,
    pub origin: String,
}

impl NarrowCat {
    /// What the editor writes for a project that sets no category.
    pub fn editor_default() -> NarrowCat {
        NarrowCat {
            category: 0x0f,
            sub_category: 0,
            timbre: 0,
            envelope: 0,
            motion: 1,
            production: "Production".into(),
            origin: "Origin".into(),
        }
    }

    /// Refuses a payload whose labels run past its end. The padding behind them is not
    /// read.
    pub fn parse(payload: &[u8]) -> Result<NarrowCat, ParseError> {
        let short = || {
            ParseError::AssertFail(format!(
                "cat section is {} bytes, too short for its categories and labels",
                payload.len()
            ))
        };
        let head = payload.get(..LABELS_AT).ok_or_else(short)?;
        let mut at = LABELS_AT;
        let mut label = || -> Result<String, ParseError> {
            let len = usize::from(*payload.get(at).ok_or_else(short)?);
            let text = payload.get(at + 1..at + 1 + len).ok_or_else(short)?;
            at += 1 + len;
            Ok(String::from_utf8_lossy(text).into_owned())
        };
        let (production, origin) = (label()?, label()?);
        Ok(NarrowCat {
            category: head[0],
            sub_category: head[1],
            timbre: head[2],
            envelope: head[3],
            motion: head[4],
            production,
            origin,
        })
    }

    /// The payload, zero-padded to a whole number of 24-bit words like every narrow
    /// section. Refuses a label longer than its one-byte length can state.
    pub fn payload(&self) -> Result<Vec<u8>, ParseError> {
        let mut payload = vec![
            self.category,
            self.sub_category,
            self.timbre,
            self.envelope,
            self.motion,
        ];
        for label in [&self.production, &self.origin] {
            let len = u8::try_from(label.len()).map_err(|_| ParseError::OutOfBounds {
                value: format!("a {}-byte category label", label.len()),
                bound: "255 bytes, what its length byte states".into(),
            })?;
            payload.push(len);
            payload.extend_from_slice(label.as_bytes());
        }
        while !payload.len().is_multiple_of(3) {
            payload.push(0);
        }
        Ok(payload)
    }
}

/// The wide payload's two categories.
pub fn parse_wide(payload: &[u8]) -> Result<(u8, u8), ParseError> {
    match payload {
        [category, sub_category, ..] => Ok((*category, *sub_category)),
        _ => Err(ParseError::AssertFail(format!(
            "cat section is {} bytes, too short for its categories",
            payload.len()
        ))),
    }
}

/// The wide payload for two categories.
pub fn wide_payload(category: u8, sub_category: u8) -> [u8; WIDE_LEN] {
    let mut payload = [0u8; WIDE_LEN];
    payload[0] = category;
    payload[1] = sub_category;
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_default_is_the_payload_the_encoder_has_always_written() {
        let payload = NarrowCat::editor_default().payload().unwrap();
        let mut want = vec![0x0f, 0, 0, 0, 1, 10];
        want.extend_from_slice(b"Production");
        want.push(6);
        want.extend_from_slice(b"Origin");
        want.push(0);
        assert_eq!(payload, want);
        assert_eq!(
            NarrowCat::parse(&payload).unwrap(),
            NarrowCat::editor_default()
        );
    }

    #[test]
    fn a_label_running_past_the_payload_is_refused() {
        assert!(NarrowCat::parse(&[0x0f, 0, 0, 0, 1, 9, b'P']).is_err());
        assert!(NarrowCat::parse(&[0x0f, 0, 0, 0]).is_err());
    }

    #[test]
    fn empty_labels_pad_to_a_word() {
        let cat = NarrowCat {
            production: String::new(),
            origin: String::new(),
            ..NarrowCat::editor_default()
        };
        assert_eq!(cat.payload().unwrap(), [0x0f, 0, 0, 0, 1, 0, 0, 0, 0]);
    }

    #[test]
    fn the_wide_payload_keeps_two_categories() {
        assert_eq!(wide_payload(3, 1), [3, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(parse_wide(&wide_payload(3, 1)).unwrap(), (3, 1));
        assert!(parse_wide(&[3]).is_err());
    }
}
