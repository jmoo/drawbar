//! A sample instrument's fields outside its audio, read and edited through an
//! [`Index`](super::Index).

use super::{zone, Body, Chain, KeyTable, Meta, Sample, SampleV3, Sty, StyV2, Zone, ZoneV3};
use crate::cbin::Cbin;
use crate::error::{Error, ParseError};

/// What a sample instrument states outside its audio, as [`crate::Sample`] states it: the
/// name, the categories, the zones, the keyboard map and the `sty` preset.
///
/// Every accessor and setter here runs the one [`crate::Sample`] runs, over the same
/// sections, so it answers as a whole read answers. Each stroke is held by its id and
/// root key alone, so nothing here reaches a stream. Edit a clone and hand it to
/// [`Index::patch`](super::Index::patch) to save it.
#[derive(Debug)]
pub struct Outline {
    /// Every section but each stroke whole; each stroke cut to its opening bytes.
    ///
    /// ⚠️ Never handed out: its stroke streams, offsets and bytes are not the file's.
    sample: crate::Sample,
}

impl Clone for Outline {
    fn clone(&self) -> Outline {
        fn cloned<B>(file: &Cbin<Body<B>>) -> Cbin<Body<B>>
        where
            B: super::Framing + Clone,
        {
            Cbin {
                header: file.header.clone(),
                body: Body {
                    sections: file.body.sections.clone(),
                },
            }
        }
        Outline {
            sample: match &self.sample {
                crate::Sample::V2(file) => crate::Sample::V2(cloned(file)),
                crate::Sample::V3(file) => crate::Sample::V3(cloned(file)),
            },
        }
    }
}

/// One generation's own fields, from [`Outline::fields`].
pub enum Fields<'a> {
    V2(NarrowFields<'a>),
    V3(WideFields<'a>),
}

/// The narrow chain's fields. Each is the one the same-named method of
/// [`Cbin<Sample>`] reads.
pub struct NarrowFields<'a>(&'a Cbin<Sample>);

/// The wide chain's fields. Each is the one the same-named method of
/// [`Cbin<SampleV3>`] reads.
pub struct WideFields<'a>(&'a Cbin<SampleV3>);

impl Outline {
    pub(super) fn new(sample: crate::Sample) -> Outline {
        Outline { sample }
    }

    pub(super) fn sample(&self) -> &crate::Sample {
        &self.sample
    }

    pub fn fields(&self) -> Fields<'_> {
        match &self.sample {
            crate::Sample::V2(file) => Fields::V2(NarrowFields(file)),
            crate::Sample::V3(file) => Fields::V3(WideFields(file)),
        }
    }

    /// See [`crate::Sample::name`].
    pub fn name(&self) -> Result<String, Error> {
        self.sample.name()
    }

    /// See [`crate::Sample::max_name_len`].
    pub fn max_name_len(&self) -> usize {
        self.sample.max_name_len()
    }

    /// See [`crate::Sample::generation`].
    pub fn generation(&self) -> &'static str {
        self.sample.generation()
    }

    /// See [`crate::Sample::chain`].
    pub fn chain(&self) -> Result<Chain, Error> {
        self.sample.chain()
    }

    /// See [`crate::Sample::name_is_editable`].
    pub fn name_is_editable(&self) -> bool {
        self.sample.name_is_editable()
    }

    /// See [`crate::Sample::zones_are_editable`].
    pub fn zones_are_editable(&self) -> bool {
        self.sample.zones_are_editable()
    }

    /// See [`crate::Sample::has_low_note`].
    pub fn has_low_note(&self) -> bool {
        self.sample.has_low_note()
    }

    /// See [`crate::Sample::set_name`].
    pub fn set_name(&mut self, name: &str) -> Result<(), Error> {
        self.sample.set_name(name)
    }

    /// See [`crate::Sample::set_root_key`].
    pub fn set_root_key(&mut self, index: usize, note: u8) -> Result<(), Error> {
        self.sample.set_root_key(index, note)
    }

    /// See [`crate::Sample::set_zone_top_note`].
    pub fn set_zone_top_note(&mut self, index: usize, note: u8) -> Result<(), Error> {
        self.sample.set_zone_top_note(index, note)
    }

    /// See [`crate::Sample::set_zone_low_note`].
    pub fn set_zone_low_note(&mut self, index: usize, note: u8) -> Result<(), Error> {
        self.sample.set_zone_low_note(index, note)
    }

    /// Replace the narrow chain's keyboard map, as [`Cbin<Sample>::set_key_table`] does.
    /// The wide chain's is not written.
    pub fn set_key_table(&mut self, table: &KeyTable) -> Result<(), Error> {
        match &mut self.sample {
            crate::Sample::V2(file) => file.set_key_table(table),
            crate::Sample::V3(_) => Err(ParseError::AssertFail(
                "only the narrow chain carries a keyboard map this crate writes".into(),
            )
            .into()),
        }
    }
}

impl NarrowFields<'_> {
    pub fn chain(&self) -> Result<Chain, Error> {
        self.0.chain()
    }

    pub fn zones(&self) -> Result<Vec<Zone>, Error> {
        self.0.zones()
    }

    pub fn key_table(&self) -> Result<KeyTable, Error> {
        self.0.key_table()
    }

    pub fn sty(&self) -> Result<StyV2, Error> {
        self.0.sty()
    }

    pub fn categories(&self) -> Vec<String> {
        self.0.categories()
    }
}

impl WideFields<'_> {
    pub fn sub_name(&self) -> Result<String, Error> {
        self.0.sub_name()
    }

    pub fn zones(&self) -> Result<Vec<ZoneV3>, Error> {
        self.0.zones()
    }

    pub fn zone_table(&self) -> Result<zone::Table, Error> {
        self.0.zone_table()
    }

    pub fn sty(&self) -> Result<Sty, Error> {
        self.0.sty()
    }

    pub fn meta(&self) -> Result<Meta, Error> {
        self.0.meta()
    }

    pub fn stroke_count(&self) -> usize {
        self.0.stroke_count()
    }
}
