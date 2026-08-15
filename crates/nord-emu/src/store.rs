//! What the emulated instrument holds: partitions, their bank geometry, and the
//! objects in their slots.
//!
//! A partition's index **is** its object class code, so the table is a `Vec` indexed by
//! position — including the two `(Native)` library views that have no [`ObjectClass`]
//! name.

use std::collections::BTreeMap;

use nord_usb::wire::{Location, ObjectClass};

/// Device status words this crate answers with.
///
/// The named ones are what the device has been seen to send; each says where it came
/// from.
pub mod status {
    /// Success.
    pub const OK: u32 = 0;
    /// The slot holds nothing. Confirmed on hardware (nord-usb's `INFO` on a vacant
    /// slot), and also what a device with an abandoned UI session answers for **every**
    /// slot.
    pub const EMPTY: u32 = 0x1;
    /// Wrong argument arity or an out-of-range argument value. Confirmed on hardware for
    /// `BANKS` with no argument.
    pub const BAD_ARGUMENTS: u32 = 0x2;
    /// The address does not exist in this class. Confirmed on hardware — bank 9 and slot
    /// 51 on an 8 × 50 program class both draw it.
    pub const OUT_OF_RANGE: u32 = 0x3;
    /// The destination already holds something. Confirmed on hardware for `BEGIN_WRITE`;
    /// `MOVE` and `COPY` do **not** share the precondition.
    pub const OCCUPIED: u32 = 0x4;
    /// Focus does not apply to this class. Confirmed on hardware for the sample library.
    pub const NO_FOCUS: u32 = 0x15;

    /// The cursor is disabled — see [`nord_usb::op::ENUMERATION_DISABLED`].
    pub use nord_usb::op::ENUMERATION_DISABLED;
    /// The session is no longer valid — see [`nord_usb::session::STALE_SESSION`].
    pub use nord_usb::session::STALE_SESSION;
}

/// One bank of a partition, as [`nord_usb::wire::cmd::BANKS`] reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bank {
    pub name: String,
    /// Slot capacity. [`nord_usb::wire::Bank::UNBOUNDED`] is the sentinel the `(Native)`
    /// views report instead of one.
    pub slots: u32,
}

impl Bank {
    pub fn new(name: &str, slots: u32) -> Self {
        Self {
            name: name.into(),
            slots,
        }
    }
}

/// A library object a stored object references, as
/// [`nord_usb::wire::cmd::DEPENDENCIES`] reports it.
///
/// Wider than [`nord_usb::wire::Dependency`] by one field: the host decoder drops the
/// second word, and a device has to put something there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// Whether the section owning this reference is routed to a keyboard part.
    pub flag: u8,
    /// Set when the reference points at an object that is no longer there — a `delete`
    /// dangles a set-list row and this is what changes. Confirmed on hardware; zero
    /// otherwise.
    pub missing: u32,
    /// Raw object class of the referenced object.
    pub class: u32,
    /// Content id, matching the id in the referenced object's own file header. Zero for
    /// slot-addressed references.
    pub id: u32,
    pub name: String,
    /// Set for slot-addressed references (a set list's programs); library content is
    /// addressed by [`Self::id`] and reports none.
    pub location: Option<Location>,
}

impl Dependency {
    /// A library reference: addressed by content id, with no location.
    pub fn library(class: ObjectClass, id: u32, name: &str, live: bool) -> Self {
        Self {
            flag: u8::from(live),
            missing: 0,
            class: class.to_raw(),
            id,
            name: name.into(),
            location: None,
        }
    }

    /// A slot reference — what a set list's rows are.
    pub fn slot(class: ObjectClass, at: Location) -> Self {
        Self {
            flag: 1,
            missing: 0,
            class: class.to_raw(),
            id: 0,
            name: String::new(),
            location: Some(at),
        }
    }

    /// The wire encoding: `[u8 flag][u32 missing][u32 class][u32 id][u32 name_len][name]
    /// [u32 has_location][u32 bank][u32 slot]`, no alignment padding.
    ///
    /// A row with no location writes `0xffffffff` for both address words rather than
    /// zeros. Confirmed in USB captures.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.flag);
        for w in [self.missing, self.class, self.id, self.name.len() as u32] {
            out.extend_from_slice(&w.to_be_bytes());
        }
        out.extend_from_slice(self.name.as_bytes());
        let (has, bank, slot) = match self.location {
            Some(at) => (1, at.bank, at.slot),
            None => (0, u32::MAX, u32::MAX),
        };
        for w in [has, bank, slot] {
            out.extend_from_slice(&w.to_be_bytes());
        }
    }
}

/// One stored object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    /// The name the panel shows. Stored nowhere in the object's own file.
    pub name: String,
    /// Four-character CBIN format tag, e.g. `ne5p`.
    pub format: [u8; 4],
    /// Schema version, per format tag. `INFO` reports it and a read needs it to rebuild
    /// the file header.
    pub version: u32,
    /// The entity body, exactly as the wire carries it.
    pub body: Vec<u8>,
    /// Storage cost in the opaque blocks [`nord_usb::wire::Status`] counts. `0` means
    /// "the partition's per-item cost", which is right for the fixed-size classes and
    /// wrong for pianos and samples, whose items genuinely differ.
    pub blocks: u32,
    pub dependencies: Vec<Dependency>,
    /// The two words `INFO` reports between the version and the name length. Content
    /// specific for library objects and `0xffffffff` for the slot-addressed classes;
    /// carried verbatim, never interpreted.
    pub words_before_name: [u32; 2],
    /// The two words `INFO` reports between the name and the body checksum. Zero for the
    /// slot-addressed classes; also carried verbatim.
    pub words_after_name: [u32; 2],
}

impl Object {
    /// An object of one of the slot-addressed classes: a body, a name, and the two
    /// `0xffffffff` info words those classes report.
    pub fn new(name: &str, format: &[u8; 4], version: u32, body: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            format: *format,
            version,
            body,
            blocks: 0,
            dependencies: Vec::new(),
            words_before_name: [u32::MAX; 2],
            words_after_name: [0; 2],
        }
    }

    pub fn with_dependencies(mut self, rows: Vec<Dependency>) -> Self {
        self.dependencies = rows;
        self
    }

    pub fn with_blocks(mut self, blocks: u32) -> Self {
        self.blocks = blocks;
        self
    }
}

/// What [`nord_usb::wire::cmd::FOCUS`] answers for a class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The panel has this object of the class loaded.
    At(Location),
    /// Nothing of this class is loaded — status `0x1`. Observed on a program-mode panel
    /// asked about set lists.
    Nothing,
    /// The class has no focus at all — status `0x15`. A property of the library classes,
    /// surviving power cycles.
    NotApplicable,
}

/// One storage partition, its geometry, and its contents.
///
/// The position in [`EmuDevice::partitions`](crate::EmuDevice) is the class code
/// [`nord_usb::wire::cmd::SESSION_OPEN`] carries.
#[derive(Debug, Clone)]
pub struct Partition {
    /// The device's own name for it: `Piano`, `Samp Lib (Native)`, `Program`, …
    pub name: String,
    /// The 29 capacity/flag bytes trailing the name in a `PARTITIONS` reply. **Not
    /// decoded** — they do not map cleanly onto the counters `STATUS` reports, so they
    /// are answered verbatim.
    pub fields: Vec<u8>,
    pub banks: Vec<Bank>,
    /// `free + used`, which is constant per class.
    pub capacity_blocks: u32,
    /// What one item costs when every item of the class costs the same. Pianos and
    /// samples do not divide evenly; their objects carry their own [`Object::blocks`].
    pub blocks_per_item: u32,
    /// Whether `STATUS` counts this class's items. Live and Settings report zero however
    /// many objects they hold. Confirmed in USB captures.
    pub counts_items: bool,
    /// Whether `INFO` reports a real body checksum. The library classes and Settings
    /// answer `0xffffffff` instead.
    pub checksummed: bool,
    /// The schema version a write lands with. `BEGIN_WRITE` carries no version, and the
    /// device reports one per format tag, so the class supplies it.
    pub write_version: u32,
    /// `STATUS`'s fourth and fifth words. Undecoded — one is reported elsewhere as a
    /// reserved count and the other is unexplained — so they are answered verbatim.
    pub extra_counters: [u32; 2],
    pub focus: Focus,
    objects: BTreeMap<(u32, u32), Object>,
}

impl Partition {
    /// A partition with geometry but nothing stored in it.
    pub fn new(name: &str, banks: Vec<Bank>, capacity_blocks: u32, blocks_per_item: u32) -> Self {
        Self {
            name: name.into(),
            fields: vec![0; PARTITION_FIELDS],
            banks,
            capacity_blocks,
            blocks_per_item,
            counts_items: true,
            checksummed: true,
            write_version: 0,
            extra_counters: [0; 2],
            focus: Focus::Nothing,
            objects: BTreeMap::new(),
        }
    }

    pub fn get(&self, at: Location) -> Option<&Object> {
        self.objects.get(&(at.bank, at.slot))
    }

    pub fn get_mut(&mut self, at: Location) -> Option<&mut Object> {
        self.objects.get_mut(&(at.bank, at.slot))
    }

    pub fn insert(&mut self, at: Location, object: Object) -> Option<Object> {
        self.objects.insert((at.bank, at.slot), object)
    }

    pub fn remove(&mut self, at: Location) -> Option<Object> {
        self.objects.remove(&(at.bank, at.slot))
    }

    /// Every occupied address, in order.
    pub fn occupied(&self) -> impl Iterator<Item = (Location, &Object)> {
        self.objects.iter().map(|((bank, slot), o)| {
            (
                Location {
                    bank: *bank,
                    slot: *slot,
                },
                o,
            )
        })
    }

    /// Every stored object, mutably — what the cross-object fix-ups a move performs need.
    pub fn objects_mut(&mut self) -> impl Iterator<Item = &mut Object> {
        self.objects.values_mut()
    }

    /// Whether the address exists in this partition's geometry.
    pub fn addressable(&self, at: Location) -> bool {
        match self.banks.get(at.bank as usize) {
            // The `(Native)` views report a sentinel rather than a capacity, so there is
            // nothing to bound a slot against.
            Some(b) if b.slots == nord_usb::wire::Bank::UNBOUNDED => true,
            Some(b) => at.slot < b.slots,
            None => false,
        }
    }

    /// The counters [`nord_usb::wire::cmd::STATUS`] reports: item count, free, used.
    pub fn counters(&self) -> (u32, u32, u32) {
        let used: u32 = self
            .objects
            .values()
            .map(|o| match o.blocks {
                0 => self.blocks_per_item,
                n => n,
            })
            .sum();
        let count = match self.counts_items {
            true => self.objects.len() as u32,
            false => 0,
        };
        (count, self.capacity_blocks.saturating_sub(used), used)
    }

    /// The next occupied slot strictly after `at`, within `at`'s own bank.
    ///
    /// ⚠️ The cursor never leaves its bank — walking from `0:0` and following the answers
    /// enumerates one bank, not the class. Confirmed on hardware.
    pub fn next_occupied(&self, at: Location) -> Option<Location> {
        // NSM starts a bank at slot `0xffffffff`, which under a wrapping successor lands
        // on slot 0 and so makes the first slot reachable. Confirmed in USB captures.
        let from = at.slot.wrapping_add(1);
        self.objects
            .range((at.bank, from)..(at.bank + 1, 0))
            .next()
            .map(|((bank, slot), _)| Location {
                bank: *bank,
                slot: *slot,
            })
    }
}

/// Bytes trailing each partition record.
const PARTITION_FIELDS: usize = 29;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("literal partition fields"))
        .collect()
}

/// The eight partitions a Nord Electro 5 reports, with the geometry and capacities it
/// reports for them. Read off a capture of the author's own instrument.
///
/// The 29 field bytes per partition are answered verbatim because nothing here decodes
/// them; the pairs that describe one pool under two views differ only in a leading flag
/// byte.
pub(crate) fn electro5() -> Vec<Partition> {
    let native = |name: &str, fields: &str, capacity: u32| Partition {
        fields: hex(fields),
        counts_items: false,
        checksummed: false,
        focus: Focus::NotApplicable,
        ..Partition::new(
            name,
            vec![Bank::new("Bank 1", nord_usb::wire::Bank::UNBOUNDED)],
            capacity,
            0,
        )
    };

    vec![
        native(
            "Piano (Native)",
            "0003fe00000000c000000008000000a401010100000000000100010000",
            4013,
        ),
        Partition {
            fields: hex("0003fe00000000c000000008000000a400010100010100000100010000"),
            checksummed: false,
            extra_counters: [73, 2],
            focus: Focus::NotApplicable,
            // The panel's categories, which is what makes a piano address
            // category:position rather than an arbitrary two-level index.
            ..Partition::new(
                "Piano",
                [
                    "Grand", "Upright", "EPiano1", "EPiano2", "Clavinet", "Harps",
                ]
                .map(|n| Bank::new(n, 20))
                .to_vec(),
                4013,
                0,
            )
        },
        native(
            "Samp Lib (Native)",
            "0001fff8000002fa00000002000002e201010100000000000100010000",
            2039,
        ),
        Partition {
            fields: hex("0001fff8000002fa00000002000002e200010100010100000100010000"),
            checksummed: false,
            extra_counters: [8, 1],
            focus: Focus::NotApplicable,
            ..Partition::new("Samp Lib", vec![Bank::new("Samp Lib", 159)], 2039, 0)
        },
        Partition {
            fields: hex("0000000100000000000000010000001000010101010101010001000101"),
            write_version: 4,
            ..Partition::new(
                "Program",
                (1..=8)
                    .map(|n| Bank::new(&format!("Bank {n}"), 50))
                    .collect(),
                56400,
                141,
            )
        },
        Partition {
            fields: hex("0000000100000000000000010000001000010101010101010001000101"),
            write_version: 1,
            ..Partition::new(
                "Set List",
                (1..=4)
                    .map(|n| Bank::new(&format!("Set List {n}"), 50))
                    .collect(),
                7600,
                38,
            )
        },
        Partition {
            fields: hex("0000000100000000000000010000000000010100000000000001000000"),
            counts_items: false,
            ..Partition::new("Live", vec![Bank::new("Live", 3)], 363, 0)
        },
        Partition {
            fields: hex("0000000100000000000000010000000000010100000000000001000000"),
            counts_items: false,
            checksummed: false,
            ..Partition::new("Settings", vec![Bank::new("Settings", 1)], 34, 0)
        },
    ]
}
