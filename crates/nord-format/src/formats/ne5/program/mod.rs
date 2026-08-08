//! The Electro 5 program format (`.ne5p`).
//!
//! Reads top-down: the format's constants, then [`Program`] — the 121 bytes after
//! the container header — then the read that pairs it with a header. A file is a
//! `Cbin<Program>`, which derefs to the body. Each panel is a nested `#[bitbody]`
//! in its own module, placed here by byte range; the registry paths
//! (`center_panel.transpose`) follow the field names.
//!
//! The live buffer ([`crate::formats::ne5::live`]) is this same body under the tag
//! `ne5l`, addressed in three slots instead of eight banks of fifty; the two
//! modules share [`Program`] and differ only in tag and slot space.

mod center;
mod effects;
mod organ;
mod piano;
mod sample;

pub use center::{CenterPanel, OrganType};
pub use effects::{EffectsPanel, EqualizerPart, Fx1Type, Fx2Type, Fx3Type, Fx5Type, Routing};
pub use organ::{B3PercSpeed, B3Vib, Drawbars, FarfisaVib, OrganModel, OrganPanel, VoxVib};
pub use piano::{PianoCategory, PianoPanel};
pub use sample::SamplePanel;

pub use crate::fields::Field;

use crate::bank;
use crate::cbin::{self, Cbin, Header};
use crate::error::{Error, ParseError};
use crate::types::RangedU16Pair;

use std::io::{Read, Seek};

pub const FORMAT: &str = "ne5p";
/// Schema versions this build's field offsets have been validated against. Every corpus
/// program reports 4. See [`crate::error::ParseError::UnsupportedVersion`].
pub const KNOWN_VERSIONS: &[u32] = &[4];
/// The panel body after the container header.
pub const BODY_LEN: usize = 121;
/// Type-1 file length: 44-byte CBIN header + the body. Inferred from specimens, not
/// confirmed on hardware: a type-0 file is 18 bytes shorter — 24-byte header, same
/// body, 2-byte trailing checksum.
pub const FILE_LEN: usize = 0x2c + BODY_LEN;
pub const BANK_COUNT: u16 = 8;
pub const SLOT_COUNT: u16 = 50;

pub type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;
pub type Bank = bank::Bank<Cbin<Program>, Location>;

/// The 121-byte panel body: five panels behind a version echo. The pads between
/// the panels are unclaimed bits, kept verbatim.
#[nord_bits_derive::bitbody(121)]
pub struct Program {
    /// Every specimen echoes the header's schema version.
    #[bits(0..=15)]
    program_version: u16,

    #[at(0x02..0x09)]
    pub center_panel: CenterPanel,

    #[at(0x0e..0x16)]
    pub piano_panel: PianoPanel,

    #[at(0x1a..0x22)]
    pub sample_panel: SamplePanel,

    #[at(0x22..0x67)]
    pub organ_panel: OrganPanel,

    #[at(0x67..0x79)]
    pub effects_panel: EffectsPanel,
}

impl Default for Program {
    fn default() -> Program {
        Program {
            raw: [0; BODY_LEN],
            program_version: 4,
            center_panel: CenterPanel::default(),
            piano_panel: PianoPanel::default(),
            sample_panel: SamplePanel::default(),
            organ_panel: OrganPanel::default(),
            effects_panel: EffectsPanel::default(),
        }
    }
}

/// Gate a read on the versions this build's offsets are validated for.
pub(crate) fn known_version(
    format: &'static str,
    version: u32,
    supported: &'static [u32],
) -> Result<(), Error> {
    if !supported.contains(&version) {
        return Err(ParseError::UnsupportedVersion {
            format,
            version,
            supported,
        }
        .into());
    }
    Ok(())
}

/// Gate a read on the `aux` word every slot-addressed specimen holds.
///
/// Inferred from specimens; not confirmed on hardware: every slot-addressed file in
/// the corpus carries `0xFFFFFFFF` at `0x10`. Another value there means the word
/// carries something this build does not model, so the file is refused rather than
/// decoded on the assumption it does not matter. ⚠️ Library formats (`nsmp`) use the
/// word for real data and must not be gated on it.
pub(crate) fn unset_aux(format: &'static str, header: &Header) -> Result<(), Error> {
    if header.aux != 0xFFFF_FFFF {
        return Err(ParseError::AssertFail(format!(
            "{format}: aux is {:#010x}, not the 0xffffffff every slot-addressed file holds",
            header.aux,
        ))
        .into());
    }
    Ok(())
}

/// The typed slot a header's raw location holds, refused if out of `L`'s slot space.
pub(crate) fn slot<L: bank::Location>(header: &Header) -> Result<L, Error> {
    let (bank, slot) = header.slot();
    (bank, slot)
        .try_into()
        .map_err(|_| ParseError::AssertFail(format!("invalid location: {bank} {slot}")).into())
}

/// The program slot the file claims.
///
/// ⚠️ A live slot is the same body under another tag, so this reads a `ne5l` file's
/// location in the *program* slot space. [`crate::formats::ne5::live::location`] is the
/// one that answers for a live buffer.
pub fn location(file: &Cbin<Program>) -> Result<Location, Error> {
    slot(&file.header)
}

/// A default program addressed to `location`.
pub fn new(location: Location) -> Cbin<Program> {
    Cbin {
        header: Header::new(FORMAT, location.inner(), 4),
        body: Program::default(),
    }
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Program>, Error> {
    let file: Cbin<Program> = cbin::read(reader, FORMAT)?;
    known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    unset_aux(FORMAT, &file.header)?;
    location(&file)?;
    Ok(file)
}

/// ⚠️ Programs and live slots are one type, so a `ne5l` file placed in a program
/// [`Bank`] lands wherever its live slot number falls in the program space. The tag
/// in the header is what tells the two apart.
impl bank::Item<Location> for Cbin<Program> {
    fn location(&self) -> Location {
        // Validated at `read_from` and `new`, and only `Header::set_slot` writes it.
        location(self).expect("a program's location is validated at construction")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbin::Generation;
    use std::io::Cursor;

    /// An unknown schema version is refused at read, not decoded on a guess.
    ///
    /// Field offsets are only validated for the versions in the corpus. A future
    /// firmware bumping `ne5p` to 5 could move fields; decoding it with version-4
    /// offsets would yield plausible but wrong values, and writing it back would then
    /// persist them. Refusing is the only safe default.
    #[test]
    fn an_unknown_schema_version_is_refused() {
        let program = new((0, 0).try_into().unwrap());
        let mut bytes = Vec::new();
        program.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);

        // Sanity: as written, it reads back.
        assert!(read_from(&mut Cursor::new(&mut bytes.clone())).is_ok());

        // The schema version lives at 0x14, little-endian.
        assert_eq!(u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap()), 4);
        bytes[0x14..0x18].copy_from_slice(&5u32.to_le_bytes());

        let err = read_from(&mut Cursor::new(&mut bytes)).expect_err("version 5 must not decode");
        // The refusal is a matchable variant carrying the facts, not a string.
        assert!(
            matches!(
                err,
                Error::Parse(crate::error::ParseError::UnsupportedVersion {
                    format: "ne5p",
                    version: 5,
                    ..
                })
            ),
            "unhelpful error: {err}",
        );
    }

    /// A header whose `aux` word is not `0xFFFFFFFF` is refused, not decoded past.
    #[test]
    fn an_unexpected_aux_word_is_refused() {
        let program = new((0, 0).try_into().unwrap());
        let mut bytes = Vec::new();
        program.write_to(&mut Cursor::new(&mut bytes)).unwrap();

        // The type-1 crc32 covers the body alone, so a header edit needs no restamp.
        bytes[0x10..0x14].copy_from_slice(&0u32.to_le_bytes());
        let err = read_from(&mut Cursor::new(&bytes)).expect_err("a set aux must not decode");
        assert!(
            matches!(err, Error::Parse(ParseError::AssertFail(_))),
            "refused for the wrong reason: {err}",
        );
    }

    /// Every panel's encode is `From`, not `TryFrom`: no field can overrun its slot.
    ///
    /// The other half of that guarantee is not assertable from a test — giving a field a
    /// type wider than its slot is a const-eval panic out of `Field::FITS`, so retyping
    /// `PianoPanel::mono` from `bool` to `u8` fails to build rather than failing here.
    #[test]
    fn every_panels_encode_is_total() {
        fn total<P, W>(_: &P)
        where
            for<'a> W: From<&'a P>,
        {
        }

        let program = new((0, 0).try_into().unwrap());
        total::<_, [u8; 7]>(&program.center_panel);
        total::<_, [u8; 8]>(&program.piano_panel);
        total::<_, [u8; 8]>(&program.sample_panel);
        total::<_, [u8; 18]>(&program.effects_panel);
    }

    /// Re-stamp the body CRC after corrupting a byte, so a decode test exercises the
    /// field check rather than the checksum.
    fn restamp_crc(bytes: &mut [u8]) {
        let crc = crate::crc::crc32(&bytes[0x2c..]);
        bytes[0x18..0x1c].copy_from_slice(&crc.to_le_bytes());
    }

    /// Validation is part of the read, not a step a caller has to remember.
    ///
    /// The fallible decode runs inside `cbin::read`'s body pass, so every path to a
    /// `Program` body validates. Note there is no way to build the corrupt input through
    /// the API at all: `lower_part` is an `Instrument`, so a panel in memory *cannot*
    /// hold the invalid value. It has to be forged in the bytes.
    #[test]
    fn no_decode_path_can_skip_validation() {
        let program = new((0, 0).try_into().unwrap());
        let mut bytes = Vec::new();
        program.write_to(&mut Cursor::new(&mut bytes)).unwrap();

        // Self-check: re-stamping an untouched file must be a no-op.
        let pristine = bytes.clone();
        restamp_crc(&mut bytes);
        assert_eq!(bytes, pristine, "the CRC helper does not match the writer");

        // 0b111 is not an `Instrument`.
        bytes[0x2e] |= 0b1110_0000;
        restamp_crc(&mut bytes);

        let front = read_from(&mut Cursor::new(&mut bytes))
            .expect_err("the front door accepted an undecodable panel");
        // Structural, not textual: the typed refusal must survive the read's wrapping.
        assert!(
            matches!(
                front,
                Error::Parse(crate::error::ParseError::OutOfBounds { .. })
            ),
            "refused for the wrong reason: {front}",
        );
        assert!(
            CenterPanel::try_from(<[u8; 7]>::try_from(&bytes[0x2e..0x35]).unwrap()).is_err(),
            "the conversion itself accepted an undecodable panel",
        );
    }

    /// A field set by name lands in the bits that field owns, and in no others.
    #[test]
    fn setting_a_field_by_name_moves_only_that_fields_bytes() {
        let mut program = new((0, 0).try_into().unwrap());
        let mut before = Vec::new();
        program.write_to(&mut Cursor::new(&mut before)).unwrap();

        program.set_field("center_panel.transpose", "-5").unwrap();
        assert_eq!(program.center_panel.transpose.inner(), -5);

        let mut after = Vec::new();
        program.write_to(&mut Cursor::new(&mut after)).unwrap();

        // `transpose` is bits 24..=27 of a panel starting at 0x2e, so byte 0x31 — plus
        // the body CRC at 0x18..0x1c, which every body change moves.
        let moved: Vec<usize> = (0..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        assert_eq!(moved, vec![0x18, 0x19, 0x1a, 0x1b, 0x31], "{moved:x?}");
    }

    /// The library's field names are the CLI's arguments, so a path that does not exist
    /// has to say which half was wrong.
    #[test]
    fn an_unknown_path_names_what_it_could_not_find() {
        let mut program = new((0, 0).try_into().unwrap());
        for (path, wanted) in [
            ("center_panel.nonesuch", "nonesuch"),
            ("nonesuch.transpose", "nonesuch"),
            ("transpose", "transpose"),
        ] {
            let err = program.set_field(path, "0").unwrap_err().to_string();
            assert!(err.contains(wanted), "{path}: {err}");
        }
    }

    /// Every field the panels declare is listed, and each lists a way to spell itself.
    #[test]
    fn every_declared_field_is_settable_by_its_listed_name() {
        let mut program = new((0, 0).try_into().unwrap());
        let fields = program.fields();
        assert!(
            fields.iter().any(|f| f.path == "center_panel.transpose"),
            "the worked example is missing from the registry",
        );

        // Round-tripping every field through its own listed spelling is the property
        // that makes the registry usable: what `--fields` prints is what `--set` takes.
        for f in fields {
            let (path, value) = (f.path.clone(), f.value.clone());
            program
                .set_field(&path, &value)
                .unwrap_or_else(|e| panic!("{path} = {value:?}: {e}"));
        }
    }

    /// A nine-nibble drawbar block has no named values, so it is spelled by its bits —
    /// which for this field is also how a reader wants to see it.
    #[test]
    fn a_wide_field_is_spelled_by_its_stored_bits() {
        let mut program = new((0, 0).try_into().unwrap());
        program
            .set_field("organ_panel.b3_preset1_drawbars", "0x087654321")
            .unwrap();
        assert_eq!(
            program.organ_panel.drawbars(OrganModel::B3, 1),
            [0, 8, 7, 6, 5, 4, 3, 2, 1],
        );

        let listed = program
            .fields()
            .into_iter()
            .find(|f| f.path == "organ_panel.b3_preset1_drawbars")
            .expect("declared");
        assert_eq!(listed.value, "0x87654321");
        assert_eq!(listed.display, "[0, 8, 7, 6, 5, 4, 3, 2, 1]");
    }

    /// Decode and encode are inverses on any bytes the decoder accepts.
    #[test]
    fn decode_and_encode_are_inverse() {
        for pattern in [0u64, u64::MAX, 0xa5a5_a5a5_a5a5_a5a5, 0x5a5a_5a5a_5a5a_5a5a] {
            let raw = pattern.to_be_bytes();
            let panel = PianoPanel::try_from(raw).unwrap();
            assert_eq!(<[u8; 8]>::from(&panel), raw);

            let panel = SamplePanel::try_from(raw).unwrap();
            assert_eq!(<[u8; 8]>::from(&panel), raw);

            let raw: [u8; 7] = raw[..7].try_into().unwrap();
            if let Ok(panel) = CenterPanel::try_from(raw) {
                assert_eq!(<[u8; 7]>::from(&panel), raw);
            }
        }
    }

    /// The layout the macro publishes is the layout the codec uses: the panels
    /// sit where the declaration says, and a nested entry chains into the
    /// panel's own field placements.
    #[test]
    fn the_program_body_layout_is_published_as_data() {
        use crate::layout::BodyLayout;

        let fields = Program::layout();
        let center = fields
            .iter()
            .find(|f| f.path == "center_panel")
            .expect("declared");
        assert_eq!((center.lo / 8, (center.hi + 1) / 8), (0x02, 0x09));
        let nested = center.nested.expect("a panel chains to its own layout");
        assert!(
            nested().iter().any(|f| f.path == "transpose"),
            "the nested layout does not list the panel's fields",
        );

        // The registry walks the same structure: full paths, panel by panel.
        let program = new((0, 0).try_into().unwrap());
        let paths: Vec<String> = program.fields().into_iter().map(|f| f.path).collect();
        assert!(paths.contains(&"center_panel.transpose".to_string()));
        assert!(paths.contains(&"piano_panel.id".to_string()));
        assert!(paths.contains(&"sample_panel.id".to_string()));
    }

    /// A program re-tagged type 0 is the same 121-byte body behind the shorter
    /// header, 18 bytes shorter in total, and it round-trips as itself.
    #[test]
    fn a_type_0_program_is_the_same_body_18_bytes_earlier() {
        let mut program = new((3, 7).try_into().unwrap());
        let mut v1 = Vec::new();
        program.write_to(&mut Cursor::new(&mut v1)).unwrap();

        program.header.generation = Generation::V0;
        let mut v0 = Vec::new();
        program.write_to(&mut Cursor::new(&mut v0)).unwrap();

        assert_eq!(v1.len() - v0.len(), 18);
        assert_eq!(&v1[0x2c..], &v0[0x18..v0.len() - 2], "bodies differ");
        assert_eq!(
            &v1[0x08..0x18],
            &v0[0x08..0x18],
            "shared header fields differ"
        );

        let back = read_from(&mut Cursor::new(&v0)).unwrap();
        assert_eq!(back.header.generation, Generation::V0);
        let mut again = Vec::new();
        back.write_to(&mut Cursor::new(&mut again)).unwrap();
        assert_eq!(again, v0, "type-0 round trip changed the bytes");
    }
}
