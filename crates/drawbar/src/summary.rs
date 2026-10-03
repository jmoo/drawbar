//! What a read of a file found, kept without its bytes.

use nord_format::Entity;
use nord_usb::ObjectClass;
use serde::{Deserialize, Serialize};

/// The library a program plays: its class, and the id the instrument knows it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Plays {
    Piano(u32),
    Sample(u32),
}

impl Plays {
    /// The first library a program names, piano before sample.
    pub fn of(entity: &Entity) -> Option<Plays> {
        let fields = crate::fields::fields_of(entity)?;
        let named = |path: &str| {
            fields
                .iter()
                .find(|field| field.path == path)
                .and_then(|field| crate::document::library_id(&field.value))
                // Zero is "this program references no library", not an id to look for.
                .filter(|id| *id != 0)
        };
        named("piano_panel.id")
            .map(Plays::Piano)
            .or_else(|| named("sample_panel.id").map(Plays::Sample))
    }

    pub fn class(self) -> ObjectClass {
        match self {
            Plays::Piano(_) => ObjectClass::Piano,
            Plays::Sample(_) => ObjectClass::Sample,
        }
    }

    pub fn id(self) -> u32 {
        match self {
            Plays::Piano(id) | Plays::Sample(id) => id,
        }
    }
}
