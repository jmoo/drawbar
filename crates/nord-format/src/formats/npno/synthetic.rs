//! A piano library laid out from a description, for tests with no library to start
//! from.
//!
//! The body follows the layout that parsing a [`Library`] checks: the prefix, the
//! stroke directory, the zero alignment gap, then one audio span per stroke in
//! directory order. Each span is filled with a byte naming its stroke, so a re-lay is
//! visible in the bytes themselves.
//!
//! ⚠️ That filler is not encoded audio, so [`codec::decode`] refuses it. Use
//! [`Take::silent`] for a span that decodes.
//!
//! [`Library`]: crate::formats::npno::Library
//! [`codec::decode`]: crate::formats::npno::codec::decode
//! [`Take::silent`]: crate::formats::npno::synthetic::Take::silent

use super::*;

/// One recording to lay out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Take {
    pub root: u8,
    pub bank: Bank,
    /// The softness value the record states; see [`Stroke::layer`].
    pub layer: u8,
    /// Blocks of audio the stroke owns, each [`Library::block_bytes`] long.
    pub blocks: u16,
    /// The length marks at `+0x1c`.
    pub marks: [u32; MARKS],
    /// The decay coefficient at `+0x2e`.
    pub decay: u32,
    /// The decay ladder at `+0x36`.
    pub ladder: [u32; DECAYS],
    /// The identifier at `+0x6e`. When the caller names none, the stroke's place in the
    /// directory keeps a build's identifiers distinct.
    pub id: Option<u32>,
    /// Whether the span holds decodable silent blocks instead of filler.
    pub silent: bool,
}

pub fn take(root: u8, bank: Bank, layer: u8, blocks: u16) -> Take {
    Take {
        root,
        bank,
        layer,
        blocks,
        marks: [0; MARKS],
        decay: 0,
        ladder: [0; DECAYS],
        id: None,
        silent: false,
    }
}

impl Take {
    pub fn marks(mut self, marks: [u32; MARKS]) -> Take {
        self.marks = marks;
        self
    }

    pub fn decay(mut self, decay: u32) -> Take {
        self.decay = decay;
        self
    }

    pub fn ladder(mut self, ladder: [u32; DECAYS]) -> Take {
        self.ladder = ladder;
        self
    }

    pub fn id(mut self, id: u32) -> Take {
        self.id = Some(id);
        self
    }

    /// Lay the span out as decodable silence, every block at order zero over the widest
    /// field, and state the frames those blocks own.
    pub fn silent(mut self) -> Take {
        self.silent = true;
        self
    }
}

/// A library to lay out: the stream version, the channel count, the takes and the key
/// map's routes.
///
/// The takes must be in ascending root order, which the per-root counts require;
/// [`Library::to_body`] refuses any other order.
#[derive(Clone, Debug)]
pub struct Build {
    pub version: u16,
    pub channels: u16,
    pub takes: Vec<Take>,
    /// `(key, root)` routes. A key with no route reads [`UNCOVERED`].
    pub map: Vec<(u8, u8)>,
}

/// The name every synthetic library carries in the field at [`TextField::COMBINED`].
const NAME: &[u8] = b"Test Piano#Variant";

/// The header a block of silence carries: the widest field at order zero, with the
/// attenuation a block with no signal states.
const SILENT_HEADER: u16 = (100 << 8) | codec::MAX_WIDTH as u16;

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
        put16(&mut body, VERSION_AT, self.version);
        put16(&mut body, VERSION_ECHO_AT, self.version);
        put16(&mut body, CHANNELS_AT, self.channels);
        body[TextField::COMBINED.at..TextField::COMBINED.at + NAME.len()].copy_from_slice(NAME);

        body[KEY_MAP_AT..KEY_MAP_AT + NOTES].fill(UNCOVERED);
        for &(key, root) in &self.map {
            body[KEY_MAP_AT + usize::from(key)] = root;
        }
        put16(&mut body, STROKE_COUNT_AT, count as u16);
        for note in 0..NOTES {
            let n = self
                .takes
                .iter()
                .filter(|take| usize::from(take.root) == note)
                .count() as u16;
            let at = ROOT_COUNTS_AT + note * 2;
            put16(&mut body, at, n);
        }

        let mut at = first;
        for (i, take) in self.takes.iter().enumerate() {
            let record = DIRECTORY_AT + i * RECORD;
            put32(&mut body, record + REC_START, at as u32);
            body[record + REC_BANK] = take.bank.code();
            body[record + REC_LAYER] = take.layer;
            put16(&mut body, record + REC_BLOCKS, take.blocks);
            for (mark, value) in take.marks.iter().enumerate() {
                let field = record + REC_MARKS + mark * 4;
                put32(&mut body, field, *value);
            }
            put32(&mut body, record + REC_DECAY, take.decay);
            for (entry, value) in take.ladder.iter().enumerate() {
                let field = record + REC_DECAYS + entry * 4;
                put32(&mut body, field, *value);
            }
            let id = take.id.unwrap_or(i as u32);
            put32(&mut body, record + REC_ID, id);

            let span = usize::from(take.blocks) * block;
            if take.silent {
                let per_block =
                    codec::block_frames(codec::MAX_WIDTH, block, usize::from(self.channels))
                        - codec::OVERLAP;
                let owned = (usize::from(take.blocks) * per_block) as u32;
                put32(&mut body, record + REC_FRAMES, owned);
                for block_at in (at..at + span).step_by(block) {
                    put16(&mut body, block_at, SILENT_HEADER);
                }
            } else {
                body[at..at + span].fill(0x40 + i as u8);
            }
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

    /// The whole file's bytes.
    pub fn bytes(&self) -> Result<Vec<u8>, Error> {
        crate::to_bytes(&crate::Entity::Piano(self.piano()))
    }
}

impl Default for Build {
    fn default() -> Self {
        Build::new()
    }
}
