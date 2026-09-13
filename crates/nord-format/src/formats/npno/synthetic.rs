//! A piano library laid out from a description, for tests with no library to start
//! from.
//!
//! The body follows the law [`Library::parse_body`](super::Library) checks: the
//! prefix, the stroke directory, the zero alignment gap, then one audio span per
//! stroke in directory order. Each span is filled with a byte naming its stroke, so a
//! re-lay is visible in the bytes themselves.
//!
//! ⚠️ The audio is filler rather than encoded blocks, so [`codec::decode`](super::codec)
//! refuses it. What this builds is a container to transform, not a sound to play.

use super::*;

/// One recording to lay out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Take {
    pub root: u8,
    pub bank: Bank,
    /// The softness value the record states — see [`Stroke::layer`].
    pub layer: u8,
    /// Blocks of audio the stroke owns, each [`Library::block_bytes`] long.
    pub blocks: u16,
}

pub fn take(root: u8, bank: Bank, layer: u8, blocks: u16) -> Take {
    Take {
        root,
        bank,
        layer,
        blocks,
    }
}

/// A library to lay out: the stream version, the channel count, the takes and the key
/// map's routes.
///
/// The takes must be in ascending root order, which is the order the per-root counts
/// index the directory by; [`Library::to_body`] refuses any other.
#[derive(Clone, Debug)]
pub struct Build {
    pub version: u16,
    pub channels: u16,
    pub takes: Vec<Take>,
    /// `(key, root)` routes. Every key named by none reads [`UNCOVERED`].
    pub map: Vec<(u8, u8)>,
}

/// The name every synthetic library carries in the field at [`TextField::COMBINED`].
const NAME: &[u8] = b"Test Piano#Variant";

impl Build {
    /// Two roots, one of them with a release stroke, over three routed keys.
    pub fn new() -> Build {
        Build {
            version: 0x450,
            channels: 1,
            takes: vec![
                take(60, Bank::Attack, 0, 1),
                take(60, Bank::Release, 3, 1),
                take(72, Bank::Attack, 0, 2),
            ],
            map: vec![(60, 60), (61, 60), (72, 72)],
        }
    }

    pub fn body(&self) -> Vec<u8> {
        let block = block_bytes(self.channels);
        let count = self.takes.len();
        let directory_end = DIRECTORY_AT + count * RECORD;
        let first = first_audio_offset(directory_end, block)
            .expect("a directory of this many strokes fits an offset");
        let audio: usize = self
            .takes
            .iter()
            .map(|take| usize::from(take.blocks) * block)
            .sum();

        let mut body = vec![0u8; first + audio];
        body[..4].copy_from_slice(CNSP_MAGIC);
        body[VERSION_AT..VERSION_AT + 2].copy_from_slice(&self.version.to_be_bytes());
        body[VERSION_ECHO_AT..VERSION_ECHO_AT + 2].copy_from_slice(&self.version.to_be_bytes());
        body[CHANNELS_AT..CHANNELS_AT + 2].copy_from_slice(&self.channels.to_be_bytes());
        body[TextField::COMBINED.at..TextField::COMBINED.at + NAME.len()].copy_from_slice(NAME);

        body[KEY_MAP_AT..KEY_MAP_AT + NOTES].fill(UNCOVERED);
        for &(key, root) in &self.map {
            body[KEY_MAP_AT + usize::from(key)] = root;
        }
        body[STROKE_COUNT_AT..STROKE_COUNT_AT + 2].copy_from_slice(&(count as u16).to_be_bytes());
        for note in 0..NOTES {
            let n = self
                .takes
                .iter()
                .filter(|take| usize::from(take.root) == note)
                .count() as u16;
            let at = ROOT_COUNTS_AT + note * 2;
            body[at..at + 2].copy_from_slice(&n.to_be_bytes());
        }

        let mut at = first;
        for (i, take) in self.takes.iter().enumerate() {
            let record = DIRECTORY_AT + i * RECORD;
            body[record + REC_START..record + REC_START + 4]
                .copy_from_slice(&(at as u32).to_be_bytes());
            body[record + REC_BANK] = take.bank.code();
            body[record + REC_LAYER] = take.layer;
            body[record + REC_BLOCKS..record + REC_BLOCKS + 2]
                .copy_from_slice(&take.blocks.to_be_bytes());
            body[record + REC_ID..record + REC_ID + 4].copy_from_slice(&(i as u32).to_be_bytes());
            let span = usize::from(take.blocks) * block;
            body[at..at + span].fill(0x40 + i as u8);
            at += span;
        }
        body
    }

    /// The library as a file, container header and all.
    pub fn piano(&self) -> Piano {
        Piano {
            file: Cbin {
                header: Header::new(FORMAT, (0, 0), 530),
                body: RawBody(self.body()),
            },
        }
    }

    /// The file's bytes, as a reader would be handed them.
    pub fn bytes(&self) -> Result<Vec<u8>, Error> {
        crate::to_bytes(&crate::Entity::Piano(self.piano()))
    }
}

impl Default for Build {
    fn default() -> Self {
        Build::new()
    }
}
