//! The `NWS` section chain that makes up a sample instrument's body.

use crate::error::ParseError;

/// Bytes of section header: 3-char tag, NUL, `u8` version, `u32` length.
pub const HEADER_LEN: usize = 9;

/// Opens the body. Its payload is empty; the first real section follows it.
pub const CONTAINER: &[u8; 3] = b"NWS";

pub const HDR: &[u8; 3] = b"hdr";
pub const CAT: &[u8; 3] = b"cat";
pub const MAP: &[u8; 3] = b"map";
pub const STK: &[u8; 3] = b"stk";
pub const STY: &[u8; 3] = b"sty";

/// One section of a sample instrument body.
///
/// ⚠️ `len` on the wire is **big-endian**, inside a CBIN header that is little-endian
/// throughout. Reading it the same way as the header's `u32`s yields a nonsense length
/// in the hundreds of millions.
///
/// The length counts the payload only — the 9 header bytes are not included.
#[derive(Clone, PartialEq, Eq)]
pub struct Section {
    pub tag: [u8; 3],
    /// Schema version of this section alone; sections revise independently.
    pub version: u8,
    pub payload: Vec<u8>,
}

impl Section {
    /// Length on disk, header included.
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }

    pub fn tag_str(&self) -> String {
        String::from_utf8_lossy(&self.tag).into_owned()
    }

    pub fn is(&self, tag: &[u8; 3]) -> bool {
        &self.tag == tag
    }

    fn read_one(bytes: &[u8], at: usize) -> Result<Section, ParseError> {
        let head = bytes
            .get(at..at + HEADER_LEN)
            .ok_or_else(|| ParseError::AssertFail(format!("truncated section header at {at}")))?;
        let len = u32::from_be_bytes([head[5], head[6], head[7], head[8]]) as usize;
        let from = at + HEADER_LEN;
        let payload = bytes.get(from..from + len).ok_or_else(|| {
            ParseError::AssertFail(format!(
                "section {} at {at} declares {len} bytes but only {} remain",
                String::from_utf8_lossy(&head[..3]),
                bytes.len().saturating_sub(from),
            ))
        })?;
        Ok(Section {
            tag: [head[0], head[1], head[2]],
            version: head[4],
            payload: payload.to_vec(),
        })
    }

    pub fn write_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.tag);
        out.push(0);
        out.push(self.version);
        out.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.payload);
    }
}

/// Walks the chain from `at` to the end of `bytes`.
///
/// The chain must land exactly on the end of the file. It is a strong integrity check —
/// a wrong length anywhere puts every later section at the wrong offset — so a short or
/// overrunning walk is an error rather than a truncated result.
pub fn read_chain(bytes: &[u8], at: usize) -> Result<Vec<Section>, ParseError> {
    let mut sections = Vec::new();
    let mut pos = at;
    while pos < bytes.len() {
        let section = Section::read_one(bytes, pos)?;
        pos += section.encoded_len();
        sections.push(section);
    }
    if pos != bytes.len() {
        return Err(ParseError::AssertFail(format!(
            "section chain ended at {pos}, file is {} bytes",
            bytes.len()
        )));
    }
    Ok(sections)
}

/// Finds the single section with `tag`.
///
/// ⚠️ Only for tags that appear at most once. `stk` repeats — one per zone — so it must
/// be collected in order instead; a lookup by tag silently keeps one stroke and drops the
/// rest, and every single-zone file hides that completely.
pub fn find<'a>(sections: &'a [Section], tag: &[u8; 3]) -> Option<&'a Section> {
    sections.iter().find(|s| s.is(tag))
}

pub fn find_mut<'a>(sections: &'a mut [Section], tag: &[u8; 3]) -> Option<&'a mut Section> {
    sections.iter_mut().find(|s| s.is(tag))
}

impl std::fmt::Debug for Section {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Section")
            .field("tag", &self.tag_str())
            .field("version", &self.version)
            .field("len", &self.payload.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(tag: &[u8; 3], version: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = tag.to_vec();
        v.push(0);
        v.push(version);
        v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn chain_round_trips() {
        let mut bytes = section(CONTAINER, 11, &[]);
        bytes.extend(section(HDR, 9, &[1, 2, 3]));
        bytes.extend(section(STK, 9, &[4; 20]));

        let chain = read_chain(&bytes, 0).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].payload.len(), 0);
        assert_eq!(chain[1].payload, vec![1, 2, 3]);
        assert_eq!(chain[2].version, 9);

        let mut out = Vec::new();
        for s in &chain {
            s.write_into(&mut out);
        }
        assert_eq!(out, bytes);
    }

    #[test]
    fn length_is_big_endian() {
        // 0x00000102 = 258 read big-endian; little-endian would be 0x02010000.
        let bytes = section(HDR, 1, &[0; 258]);
        let chain = read_chain(&bytes, 0).unwrap();
        assert_eq!(chain[0].payload.len(), 258);
    }

    #[test]
    fn overrunning_length_is_an_error() {
        let mut bytes = section(HDR, 1, &[7; 4]);
        bytes[8] = 200; // claim 200 payload bytes where 4 exist
        assert!(read_chain(&bytes, 0).is_err());
    }

    #[test]
    fn trailing_bytes_are_an_error() {
        let mut bytes = section(HDR, 1, &[7; 4]);
        bytes.extend_from_slice(&[0, 0, 0]); // not enough for another header
        assert!(read_chain(&bytes, 0).is_err());
    }

    #[test]
    fn repeated_tags_are_all_kept() {
        let mut bytes = section(STK, 9, &[1]);
        bytes.extend(section(STK, 9, &[2]));
        bytes.extend(section(STK, 9, &[3]));
        let chain = read_chain(&bytes, 0).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(
            chain.iter().map(|s| s.payload[0]).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }
}
