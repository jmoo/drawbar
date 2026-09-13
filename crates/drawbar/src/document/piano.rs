//! The piano-library document.
//!
//! A `.npno` is hundreds of megabytes of recorded notes — one **stroke** per root note,
//! bank and velocity layer — and what an editor of one is for is deciding which of them
//! go on the instrument. So an edit here is not a field write: it is a **plan** over the
//! bytes the asset was last saved as, and the working bytes are what [`rebuild`] makes
//! of the two. Dropping strokes throws audio away, and a switch that cannot go back on
//! is not a switch.
//!
//! ⚠️ Nothing decodes to draw a frame. The facts the sections read — the roots, the
//! layers, the banks and what each of them costs — are read out of the saved baseline
//! once and kept; a stroke's audio is decoded only when someone asks to hear it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Cursor;
use std::ops::RangeInclusive;

use eframe::egui;
use nord_format::formats::npno::encode::{Kind, ALL_KEYS_DAMPED};
use nord_format::formats::npno::{self, Bank, FINE_TUNE_CENTS_PER_UNIT};
use nord_format::Entity;
use nord_usb::ObjectClass;

use super::capability::{self, Offset, Row, State as Cap};
use super::controls::{self, Sets};
use super::keys::{self, Audition, Scale, SizeCell, Span};
use super::sample::note_picker;
use super::{Extras, Ink, Loud, SizeLine, StateLine, Tone};
use crate::app;
use crate::device::DeviceState;
use crate::icon::{icon, painted, Glyph};
use crate::led;
use crate::note;
use crate::room;
use crate::workspace::LocalEntity;

pub fn is_piano(entity: &Entity) -> bool {
    matches!(entity, Entity::Piano(_))
}

fn piano(entity: &Entity) -> Option<&npno::Piano> {
    match entity {
        Entity::Piano(piano) => Some(piano),
        _ => None,
    }
}

/// The two halves of the name field, split on its separator.
#[derive(Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    /// The text after the `#`, where the vendor records the voicing and the library's
    /// size. Empty where the field carries none.
    pub variant: String,
}

pub fn snapshot(entity: &Entity) -> Option<Result<Snapshot, String>> {
    Some(read(piano(entity)?))
}

fn read(piano: &npno::Piano) -> Result<Snapshot, String> {
    let (name, variant) = piano.name().map_err(|e| e.to_string())?;
    Ok(Snapshot { name, variant })
}

/// The stretch of keyboard the map draws: a full piano, A0 to C8.
const SPAN: Span = Span { low: 21, high: 108 };

// ---- the plan -----------------------------------------------------------------------

/// What the asset was last saved as, as far as anything here tells two baselines apart.
/// A save moves both halves at once.
type Mark = (usize, Option<u32>);

/// Every edit a piano document holds, against the baseline it is an edit of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    against: Mark,
    /// Banks dropped whole. [`Bank::Attack`] is never one: a library with no attack
    /// strokes answers no key at all.
    banks: BTreeSet<Bank>,
    /// Layer values dropped on every root.
    layers: BTreeSet<u8>,
    /// The exceptions one root makes to those switches, by `(root note, layer value)`.
    roots: BTreeMap<(u8, u8), bool>,
    /// The keys the library is cut to, where it is cut.
    range: Option<RangeInclusive<u8>>,
    name: Option<String>,
    variant: Option<String>,
    /// Retuned keys, in the file's own units — see [`FINE_TUNE_CENTS_PER_UNIT`].
    fine_tune: BTreeMap<u8, i8>,
    /// A gain over the whole library, in signed tenths of a decibel.
    gain: Option<i8>,
    /// The highest key damped at note-off; [`ALL_KEYS_DAMPED`] leaves none ringing.
    damper_top: Option<u8>,
    /// The kind of instrument the library files itself under.
    kind: Option<Kind>,
    /// Decibels a stroke is attenuated by, keyed by `(root, bank code, layer value)` —
    /// the stroke's place in the directory, which a drop moves.
    trims: BTreeMap<(u8, u8, u8), u16>,
    /// The root a re-routed key plays, `None` where it plays nothing.
    key_roots: BTreeMap<u8, Option<u8>>,
}

impl Plan {
    /// Whether one root keeps one layer: its own exception, or what the switch says.
    fn keeps_layer(&self, root: u8, layer: u8) -> bool {
        match self.roots.get(&(root, layer)) {
            Some(kept) => *kept,
            None => !self.layers.contains(&layer),
        }
    }

    /// A bank code the format does not name is kept: no row of the trim section
    /// selects it, so nothing can have asked for it to go.
    fn keeps_bank(&self, bank: Option<Bank>) -> bool {
        match bank {
            Some(bank) => !self.banks.contains(&bank),
            None => true,
        }
    }

    /// The keys the plan routes to the root at `index`, in no order: the ones the
    /// baseline sends it that no re-route has taken, and the ones a re-route has given
    /// it.
    fn routed<'a>(&'a self, facts: &'a Facts, index: usize) -> impl Iterator<Item = u8> + 'a {
        let note = facts.roots[index].note;
        facts.roots[index]
            .keys
            .iter()
            .copied()
            .filter(|key| !self.key_roots.contains_key(key))
            .chain(
                self.key_roots
                    .iter()
                    .filter(move |(_, root)| **root == Some(note))
                    .map(|(key, _)| *key),
            )
    }

    /// The keys the plan routes to the root at `index`, ascending.
    fn keys_of(&self, facts: &Facts, index: usize) -> Vec<u8> {
        let mut keys: Vec<u8> = self.routed(facts, index).collect();
        keys.sort_unstable();
        keys
    }

    /// The root the plan's map sends `key` to.
    fn answers(&self, facts: &Facts, key: u8) -> Option<usize> {
        match self.key_roots.get(&key) {
            Some(root) => root.and_then(|note| facts.index_of(note)),
            None => facts.answers(key),
        }
    }

    /// Whether a range cut still leaves this root a key to answer. A root keeping one
    /// key keeps every stroke it has.
    fn in_range(&self, facts: &Facts, index: usize) -> bool {
        match &self.range {
            None => true,
            Some(range) => self.routed(facts, index).any(|key| range.contains(&key)),
        }
    }

    fn keeps(&self, facts: &Facts, cell: &Cell) -> bool {
        let root = &facts.roots[cell.root];
        self.in_range(facts, cell.root)
            && self.keeps_bank(cell.bank)
            && self.keeps_layer(root.note, cell.layer)
    }

    /// Route `key` to a root, or to nothing, dropping an entry the baseline already
    /// says so that an empty plan stays empty.
    fn route(&mut self, facts: &Facts, key: u8, root: Option<usize>) {
        let note = root.map(|index| facts.roots[index].note);
        match facts.answers(key).map(|index| facts.roots[index].note) == note {
            true => self.key_roots.remove(&key),
            false => self.key_roots.insert(key, note),
        };
    }

    /// Throw one layer's switch for every root.
    ///
    /// ⚠️ It clears that layer's per-root exceptions. A master switch that left them
    /// standing would say one thing and do another.
    fn switch_layer(&mut self, layer: u8, keep: bool) {
        self.roots.retain(|(_, held), _| *held != layer);
        match keep {
            true => self.layers.remove(&layer),
            false => self.layers.insert(layer),
        };
    }

    fn switch_bank(&mut self, bank: Bank, keep: bool) {
        match keep {
            true => self.banks.remove(&bank),
            false => self.banks.insert(bank),
        };
    }

    /// The keys `key` reads as retuned to, in file units.
    fn tune(&self, facts: &Facts, key: u8) -> i8 {
        match self.fine_tune.get(&key) {
            Some(units) => *units,
            None => facts.tune(key),
        }
    }
}

/// The bytes a plan makes of the baseline: read it, edit it, re-lay it, write it.
///
/// All of it or none, the same rule every other editor's apply follows — and from the
/// baseline every time, so a switch put back on puts its strokes back with it.
pub fn rebuild(saved: &[u8], plan: &Plan) -> Result<Vec<u8>, String> {
    let entity = nord_format::from_stream(&mut Cursor::new(saved)).map_err(|e| e.to_string())?;
    let piano = piano(&entity).ok_or("not a piano library")?;
    let mut library = piano.library().map_err(|e| e.to_string())?;
    if let Some(name) = &plan.name {
        library.set_name(name).map_err(|e| e.to_string())?;
    }
    if let Some(variant) = &plan.variant {
        library.set_variant(variant).map_err(|e| e.to_string())?;
    }
    for (key, units) in &plan.fine_tune {
        library
            .set_fine_tune(*key, *units)
            .map_err(|e| e.to_string())?;
    }
    if let Some(tenths) = plan.gain {
        library.set_gain(tenths);
    }
    if let Some(key) = plan.damper_top {
        library.set_damper_top(key).map_err(|e| e.to_string())?;
    }
    if let Some(kind) = plan.kind {
        library.set_kind(kind);
    }
    // Before anything is dropped, so that a route to a root the directory no longer
    // records is refused by the format rather than accepted against a shorter list.
    for (key, root) in &plan.key_roots {
        library
            .set_key_root(*key, *root)
            .map_err(|e| e.to_string())?;
    }
    if let Some(range) = &plan.range {
        library
            .cut_range(range.clone())
            .map_err(|e| e.to_string())?;
    }
    for bank in &plan.banks {
        library.drop_bank(*bank);
    }
    // One predicate rather than a layer pass and a per-root pass: an exception keeps a
    // layer its switch dropped, which `Layers::Only` cannot express.
    if !plan.layers.is_empty() || !plan.roots.is_empty() {
        library.retain_strokes(|stroke| plan.keeps_layer(stroke.root, stroke.layer()));
    }
    if library.strokes().is_empty() {
        return Err("that would leave the library with no strokes at all".to_string());
    }
    // After the drops: a trim names the stroke it belongs to, and the index that stroke
    // sits at moves as strokes before it go. A trim on a stroke that has gone has no
    // record left to write.
    for ((root, bank, layer), decibels) in &plan.trims {
        let at = library.strokes().iter().position(|stroke| {
            (stroke.root, stroke.bank_code(), stroke.layer()) == (*root, *bank, *layer)
        });
        if let Some(at) = at {
            library.set_trim(at, *decibels).map_err(|e| e.to_string())?;
        }
    }
    let edited = library.to_piano().map_err(|e| e.to_string())?;
    nord_format::to_bytes(&Entity::Piano(edited)).map_err(|e| e.to_string())
}

// ---- what the baseline holds --------------------------------------------------------

/// One root note: the recording, and the keys the map sends to it.
struct Root {
    note: u8,
    keys: Vec<u8>,
}

/// Ascending `keys` as the stretches they run in.
///
/// A root's keys are one run — inferred from specimens; not confirmed on hardware —
/// and the key map can hold anything, so a root whose keys are not contiguous gets one
/// cell per run rather than one cell over the keys between them.
fn runs(keys: &[u8]) -> Vec<(u8, u8)> {
    let mut out: Vec<(u8, u8)> = Vec::new();
    for key in keys {
        match out.last_mut() {
            Some((_, top)) if *top + 1 == *key => *top = *key,
            _ => out.push((*key, *key)),
        }
    }
    out
}

/// The audio one root's layer of one bank owns. Every stroke lands in exactly one.
struct Cell {
    root: usize,
    layer: u8,
    /// `None` for a bank code the format does not name.
    bank: Option<Bank>,
    bytes: u64,
}

/// One stroke as the open root's row lists it.
struct Strike {
    root: usize,
    /// The record's own bank byte, which is the half of a trim's key [`Bank`] cannot
    /// carry for a code the format does not name.
    bank: u8,
    layer: u8,
    /// The `+0x34` attenuation, in decibels.
    trim: u16,
    /// What the applied-decay ladder at `+0x36` states, in decibels a second; `None`
    /// where every entry of it applies nothing.
    decay: Option<f32>,
}

/// What the applied-decay ladder is worth, in decibels a second, read off its first
/// entry: a one-pole coefficient of `entry / 2^23` a frame at [`npno::codec::RATE`], so
/// `20·log10(e)·rate·(1 − entry/2^23)`. `None` where every entry of it is
/// [`npno::LADDER_UNITY`], which applies nothing.
fn decay_rate(ladder: &[u32; npno::DECAYS]) -> Option<f32> {
    let held = 1.0 - f64::from(ladder[0]) / f64::from(npno::LADDER_UNITY);
    ladder
        .iter()
        .any(|entry| *entry != npno::LADDER_UNITY)
        .then_some((8.686 * f64::from(npno::codec::RATE) * held) as f32)
}

/// Everything the piano editor draws from, read once out of the saved baseline.
///
/// ⚠️ The **baseline**, not the working bytes: the plan drops strokes, and every row has
/// to go on offering what putting its switch back would restore.
struct Facts {
    name: String,
    variant: String,
    stream: u16,
    channels: u16,
    /// The file's own size, which is the scale the trim section measures against.
    total: u64,
    strokes: usize,
    roots: Vec<Root>,
    /// The layer values the directory holds, ascending — 0 the loudest.
    layers: Vec<u8>,
    /// The banks present, in [`Bank::ALL`] order.
    banks: Vec<Bank>,
    cells: Vec<Cell>,
    /// Every stroke, in directory order.
    strikes: Vec<Strike>,
    /// The per-key fine tune table, in file units.
    fine_tune: Vec<i8>,
    /// The root the map sends each of the 128 keys to.
    of_key: Vec<Option<usize>>,
    /// The keys the map routes somewhere, ascending.
    covered: Vec<u8>,
    /// A gain over the whole library, in signed tenths of a decibel.
    gain: i8,
    /// The highest key damped at note-off.
    damper_top: u8,
    /// The instrument kind the library files itself under; [`kind_name`] spells it.
    kind: u8,
}

impl Facts {
    fn of(saved: &[u8]) -> Result<Facts, String> {
        let entity =
            nord_format::from_stream(&mut Cursor::new(saved)).map_err(|e| e.to_string())?;
        let piano = piano(&entity).ok_or("not a piano library")?;
        let library = piano.library().map_err(|e| e.to_string())?;
        let (name, variant) = library.name();
        let roots: Vec<Root> = library
            .roots()
            .iter()
            .map(|note| Root {
                note: *note,
                keys: library.keys_for(*note),
            })
            .collect();

        let mut grouped: BTreeMap<(usize, u8, Option<Bank>), u64> = BTreeMap::new();
        let mut strikes: Vec<Strike> = Vec::with_capacity(library.strokes().len());
        for stroke in library.strokes() {
            let root = roots
                .iter()
                .position(|root| root.note == stroke.root)
                .ok_or("a stroke names a root the directory does not record")?;
            *grouped
                .entry((root, stroke.layer(), stroke.bank()))
                .or_default() += stroke.audio().len() as u64;
            strikes.push(Strike {
                root,
                bank: stroke.bank_code(),
                layer: stroke.layer(),
                trim: stroke.trim(),
                decay: decay_rate(&stroke.ladder()),
            });
        }
        let cells: Vec<Cell> = grouped
            .into_iter()
            .map(|((root, layer, bank), bytes)| Cell {
                root,
                layer,
                bank,
                bytes,
            })
            .collect();
        let layers: BTreeSet<u8> = cells.iter().map(|cell| cell.layer).collect();
        let present: BTreeSet<Bank> = cells.iter().filter_map(|cell| cell.bank).collect();
        let of_key: Vec<Option<usize>> = library
            .key_map()
            .iter()
            .map(|note| roots.iter().position(|root| root.note == *note))
            .collect();

        Ok(Facts {
            name,
            variant,
            stream: library.stream_version(),
            channels: library.channels(),
            total: saved.len() as u64,
            strokes: library.strokes().len(),
            roots,
            layers: layers.into_iter().collect(),
            banks: Bank::ALL
                .into_iter()
                .filter(|bank| present.contains(bank))
                .collect(),
            cells,
            strikes,
            fine_tune: (0..npno::NOTES)
                .map(|key| library.fine_tune(key as u8).unwrap_or(0))
                .collect(),
            covered: of_key
                .iter()
                .enumerate()
                .filter(|(_, root)| root.is_some())
                .map(|(key, _)| key as u8)
                .collect(),
            of_key,
            gain: library.gain(),
            damper_top: library.damper_top(),
            kind: library.kind_code(),
        })
    }

    fn tune(&self, key: u8) -> i8 {
        self.fine_tune.get(usize::from(key)).copied().unwrap_or(0)
    }

    /// The root the baseline's map sends `key` to.
    fn answers(&self, key: u8) -> Option<usize> {
        self.of_key.get(usize::from(key)).copied().flatten()
    }

    fn index_of(&self, note: u8) -> Option<usize> {
        self.roots.iter().position(|root| root.note == note)
    }

    /// The layers as the sections list them, softest first, each with its rank among
    /// the layers the file holds — rank 0 is the loudest.
    fn shown_layers(&self) -> Vec<(usize, u8)> {
        let count = self.layers.len();
        (0..count)
            .rev()
            .map(|rank| (rank, self.layers[rank]))
            .collect()
    }

    fn channel_word(&self) -> &'static str {
        match self.channels {
            1 => "mono",
            _ => "stereo",
        }
    }
}

// ---- what a plan costs --------------------------------------------------------------

/// Bytes a plan keeps: the file, less the audio of every stroke it drops.
///
/// Against the file rather than a sum of its parts, so a plan that drops nothing reads
/// as the whole file and nothing has to be weighted to make the figures agree.
fn kept_bytes(facts: &Facts, plan: &Plan) -> u64 {
    facts.total.saturating_sub(
        facts
            .cells
            .iter()
            .filter(|cell| !plan.keeps(facts, cell))
            .map(|cell| cell.bytes)
            .sum(),
    )
}

/// Bytes the cells `pick` selects still hold, which is what dropping them would shed.
fn shed_bytes(facts: &Facts, plan: &Plan, pick: impl Fn(&Cell) -> bool) -> u64 {
    facts
        .cells
        .iter()
        .filter(|cell| plan.keeps(facts, cell) && pick(cell))
        .map(|cell| cell.bytes)
        .sum()
}

/// One switch of the trim section, and what it selects.
#[derive(Clone, Copy, PartialEq, Eq)]
enum What {
    Layer(u8),
    Bank(Bank),
}

/// A cut the constraint sentence could name: what it would shed, and the switches it
/// would throw.
struct Cut {
    shed: u64,
    picked: Vec<String>,
}

/// How many switches the search walks. Every subset of them is tried, and a library
/// with more layers than this offers its softest ones and its banks — the cheap cuts.
const CUT_ITEMS: usize = 12;

/// The cheapest set of switches still on that would shed at least `over`.
///
/// Cheapest by megabytes first, then by layers spent, then by switches thrown: a trim
/// should cost the least audio, and of two that cost the same the one that keeps more
/// of the velocity range. Nothing already off is offered, and nothing that would leave
/// the library with no strokes at all.
fn cheapest_cut(facts: &Facts, plan: &Plan, over: u64) -> Option<Cut> {
    let items = droppable(facts, plan);
    let live = shed_bytes(facts, plan, |_| true);
    let mut best: Option<(u64, usize, usize, Vec<String>)> = None;
    for mask in 1..(1u32 << items.len()) {
        let pick: Vec<&(String, What)> = items
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, item)| item)
            .collect();
        let selects = |cell: &Cell| {
            pick.iter().any(|(_, what)| match what {
                What::Layer(layer) => cell.layer == *layer,
                What::Bank(bank) => cell.bank == Some(*bank),
            })
        };
        let shed = shed_bytes(facts, plan, selects);
        // A cut that takes every stroke still kept is not a cut: nothing would play.
        if shed < over || shed >= live {
            continue;
        }
        let layers = pick
            .iter()
            .filter(|(_, what)| matches!(what, What::Layer(_)))
            .count();
        let rank = (shed, layers, pick.len());
        if best
            .as_ref()
            .is_none_or(|held| rank < (held.0, held.1, held.2))
        {
            best = Some((
                shed,
                layers,
                pick.len(),
                pick.iter().map(|(name, _)| name.to_lowercase()).collect(),
            ));
        }
    }
    best.map(|(shed, _, _, picked)| Cut { shed, picked })
}

/// The switches still on, as the search offers them.
fn droppable(facts: &Facts, plan: &Plan) -> Vec<(String, What)> {
    let mut items: Vec<(String, What)> = Vec::new();
    for bank in &facts.banks {
        if *bank != Bank::Attack && plan.keeps_bank(Some(*bank)) {
            items.push((bank_name(*bank).to_string(), What::Bank(*bank)));
        }
    }
    for (rank, layer) in facts.shown_layers() {
        if shed_bytes(facts, plan, |cell| cell.layer == layer) > 0 {
            items.push((
                format!("{} layer", layer_name(rank, facts.layers.len())),
                What::Layer(layer),
            ));
        }
    }
    items.truncate(CUT_ITEMS);
    items
}

/// A list of switches as a sentence names them: `the soft layer, medium layer and
/// release samples`.
fn listed(names: &[String]) -> String {
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// What a layer is called, by its rank among the layers the file holds — rank 0 is the
/// loudest. Three of them are the panel's own three words.
fn layer_name(rank: usize, count: usize) -> String {
    match (count, rank) {
        (3, 0) => "Hard".to_string(),
        (3, 1) => "Medium".to_string(),
        (3, 2) => "Soft".to_string(),
        (_, 0) => "Loudest".to_string(),
        (count, rank) if rank + 1 == count => "Softest".to_string(),
        (_, rank) => format!("Layer {}", rank + 1),
    }
}

/// The velocities each of a root's layer values is the one that sounds, in the order
/// the values are given.
///
/// A key sounds the largest value its root holds that is at most `(127 − v)·31/127`
/// ([`npno::Stroke::layer`]), so between a louder value `a` and the next softer `b` the
/// split is at `v = 127 − ⌈b·127/31⌉`: `b` plays up to there and `a` from the next
/// velocity up. A value no velocity ever reaches gets an empty range.
fn spans(values: &[u8]) -> Vec<(u8, RangeInclusive<u8>)> {
    let mut out: Vec<(u8, RangeInclusive<u8>)> = Vec::with_capacity(values.len());
    let mut below = 0u8;
    for value in values.iter().rev() {
        let over = (u32::from(*value) * 127).div_ceil(31).min(127) as u8;
        let top = 127 - over;
        out.push((*value, below.saturating_add(1)..=top));
        below = below.max(top);
    }
    out.reverse();
    out
}

/// A velocity span as the lanes print it.
fn span_text(span: &RangeInclusive<u8>) -> String {
    match span.start() > span.end() {
        true => "no velocity".to_string(),
        false => format!("v {}–{}", span.start(), span.end()),
    }
}

/// The letter a layer wears on a root's row.
fn layer_short(rank: usize, count: usize) -> String {
    match (count, rank) {
        (3, 0) => "H".to_string(),
        (3, 1) => "M".to_string(),
        (3, 2) => "S".to_string(),
        (_, rank) => (rank + 1).to_string(),
    }
}

fn bank_name(bank: Bank) -> &'static str {
    match bank {
        Bank::Attack => "Attack samples",
        Bank::Resonance => "Pedal resonance",
        Bank::Release => "Release samples",
    }
}

/// What a bank is called on a stroke's line, a code the format does not name included.
fn bank_word(code: u8) -> String {
    match Bank::from_code(code) {
        Some(bank) => bank_name(bank).to_string(),
        None => format!("bank {code}"),
    }
}

fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::ElectricGrand => "Electric grand",
        Kind::ElectricPiano => "Electric piano",
        Kind::Wurlitzer => "Reed piano",
        Kind::Clavinet => "Clavinet",
        Kind::Grand => "Grand",
        Kind::Upright => "Upright",
        Kind::Harpsichord => "Harpsichord",
        Kind::DigitalPiano => "Digital piano",
        Kind::Hybrid => "Hybrid",
        Kind::Mallet => "Mallet",
    }
}

/// What the panel calls the kind a library files itself under, a code the format does
/// not name included.
fn kind_name(code: u8) -> String {
    match Kind::from_code(code) {
        Some(kind) => kind_word(kind).to_string(),
        None => format!("kind {code}"),
    }
}

fn bank_note(bank: Bank) -> &'static str {
    match bank {
        Bank::Attack => "every library has these",
        Bank::Resonance => "what the Small library leaves out",
        Bank::Release => "the key-up tail",
    }
}

/// The range the Keys switch cuts to when it goes off: the middle of the keyboard, as
/// far as the library reaches.
fn default_range(facts: &Facts) -> RangeInclusive<u8> {
    const MIDDLE: RangeInclusive<u8> = 36..=96;
    let low = facts.covered.first().copied().unwrap_or(*MIDDLE.start());
    let high = facts.covered.last().copied().unwrap_or(*MIDDLE.end());
    (*MIDDLE.start()).max(low)..=(*MIDDLE.end()).min(high)
}

// ---- the decoded audio --------------------------------------------------------------

/// One root's loudest kept attack stroke, decoded.
struct Played {
    /// Frames interleaved by channel at [`npno::codec::RATE`], which is what both the
    /// speakers and a WAV take.
    samples: Vec<i16>,
    channels: u16,
    /// The layer the stroke states, which is what names its WAV.
    layer: u8,
}

/// Strokes decoded on request.
///
/// ⚠️ Keyed by the asset's [`stamp`](LocalEntity::stamp) as well as its id, the way the
/// sample editor's cache is: a trim re-lays the file, and what was decoded from what it
/// held before came off another library.
#[derive(Default)]
struct Cache {
    of: Option<(u64, u64)>,
    roots: HashMap<u8, Result<Played, String>>,
}

impl Cache {
    fn follow(&mut self, id: u64, stamp: u64) {
        if self.of != Some((id, stamp)) {
            self.of = Some((id, stamp));
            self.roots.clear();
        }
    }

    fn get(&self, root: u8) -> Option<&Result<Played, String>> {
        self.roots.get(&root)
    }

    /// Decode one root, once. A refusal is remembered like a success: clicking again
    /// would only produce it a second time.
    fn decode(&mut self, entity: &Entity, root: u8) {
        if self.roots.contains_key(&root) {
            return;
        }
        self.roots.insert(root, decode(entity, root));
    }
}

/// The root's loudest stroke of [`Bank::Attack`] — the recording a key on it reaches
/// for at the top of the velocity range, which is
/// [`Stroke::layer`](npno::Stroke::layer)'s law. Confirmed on hardware.
fn decode(entity: &Entity, root: u8) -> Result<Played, String> {
    let piano = piano(entity).ok_or("this is not a piano library")?;
    let library = piano.library().map_err(|e| e.to_string())?;
    let stroke = library
        .strokes()
        .iter()
        .filter(|stroke| stroke.root == root && stroke.bank() == Some(Bank::Attack))
        .min_by_key(|stroke| stroke.layer())
        .ok_or_else(|| {
            format!(
                "root {} has no attack stroke left to play",
                note::name(root)
            )
        })?;
    let audio = npno::codec::decode(stroke, library.channels()).map_err(|e| e.to_string())?;
    Ok(Played {
        samples: audio.interleaved(),
        channels: library.channels(),
        layer: stroke.layer(),
    })
}

/// What a piano document's frame asked the app to do about one root's audio. The root
/// has to be decoded either way, so there is no separate ask for that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ask {
    /// Start this root, or stop it if it is the one sounding.
    Play(u8),
    /// Sound this root for a struck key, `semitones` from the root it was recorded at.
    Strike {
        root: u8,
        semitones: i16,
    },
    Save(u8),
}

impl Ask {
    pub fn root(self) -> u8 {
        match self {
            Ask::Play(root) | Ask::Save(root) | Ask::Strike { root, .. } => root,
        }
    }
}

/// One root's decoded stroke, ready to play or to write.
pub struct Sound<'a> {
    pub samples: &'a [i16],
    pub channels: u16,
    pub rate: u32,
    /// What a WAV of it is called — the spelling `nord piano decode` writes.
    pub name: String,
}

// ---- the document's state -----------------------------------------------------------

/// The row the map and the rows agree on, and what the keyboard is sounding. A tab
/// switch resets all of it — but never the plan, which is the edit.
#[derive(Default)]
struct View {
    picked: Option<usize>,
    open_rows: BTreeSet<usize>,
    /// A row to bring up under the map next frame, once it has a rect to scroll to.
    reveal: Option<usize>,
    key_table: bool,
    audition: Option<Audition>,
}

/// The document being drawn: its baseline, and the facts read out of it.
struct Open {
    id: u64,
    baseline: Mark,
    facts: Result<Facts, String>,
}

#[derive(Default)]
pub struct State {
    /// The plan for each piano document, by id.
    ///
    /// ⚠️ Not dropped when the tab changes. The plan is the only record of what the
    /// working bytes were trimmed from, so leaving the tab must not throw it away.
    plans: HashMap<u64, Plan>,
    open: Option<Open>,
    /// The plan as this frame's controls have left it, to be tried before it is kept.
    draft: Plan,
    view: View,
    audio: Cache,
    /// What the attached instrument has free for pianos, read where the device is in
    /// hand — the sections are drawn where it is not.
    free: Option<u64>,
}

impl State {
    /// Read the baseline, take up the plan, and answer with what the header shows over
    /// a piano library that the asset alone does not say.
    ///
    /// [`Extras::default`] for anything that is no piano library, which is what leaves
    /// the header's own rules running.
    pub fn begin(&mut self, id: u64, entity: &LocalEntity, device: &DeviceState) -> Extras {
        if !entity.entity.as_ref().is_some_and(is_piano) {
            self.open = None;
            return Extras::default();
        }
        let baseline = (entity.saved.bytes.len(), entity.saved.crc32);
        let moved = self
            .open
            .as_ref()
            .is_none_or(|open| (open.id, open.baseline) != (id, baseline));
        if moved {
            self.open = Some(Open {
                id,
                baseline,
                facts: Facts::of(&entity.saved.bytes),
            });
            self.view = View::default();
        }
        let plan = self.plans.entry(id).or_default();
        // A save makes what was dropped gone for good: the rows show the file as it now
        // is, and there is nothing left to put back.
        if plan.against != baseline {
            *plan = Plan {
                against: baseline,
                ..Plan::default()
            };
        }
        self.draft = plan.clone();
        self.audio.follow(id, entity.stamp);
        self.free = room::free_bytes(ObjectClass::Piano, device);

        let Some(facts) = self.facts() else {
            return Extras::default();
        };
        extras(facts, &self.draft, self.free)
    }

    fn facts(&self) -> Option<&Facts> {
        self.open.as_ref()?.facts.as_ref().ok()
    }

    /// Take one of the header's `path = value` sets into the plan.
    pub fn take(&mut self, sets: &Sets) -> Result<(), String> {
        for (path, value) in sets {
            match path.as_str() {
                "name" => self.draft.name = Some(value.clone()),
                "variant" => self.draft.variant = Some(value.clone()),
                _ => return Err(format!("unknown field {path:?}")),
            }
        }
        Ok(())
    }

    /// The plan this frame left behind, where it is not the one in hand.
    pub fn drafted(&self) -> Option<Plan> {
        let held = self
            .open
            .as_ref()
            .and_then(|open| self.plans.get(&open.id))?;
        (*held != self.draft).then(|| self.draft.clone())
    }

    /// Keep a plan the library accepted.
    pub fn commit(&mut self, plan: Plan) {
        if let Some(open) = &self.open {
            self.plans.insert(open.id, plan);
        }
    }

    /// Put back the plan in hand, because the library refused the drafted one.
    pub fn discard(&mut self) {
        if let Some(plan) = self.open.as_ref().and_then(|open| self.plans.get(&open.id)) {
            self.draft = plan.clone();
        }
    }

    /// Forget this document's plan. Revert puts the saved bytes back, and a plan over
    /// bytes nothing holds is not an edit of anything.
    pub fn forget(&mut self, id: u64) {
        self.plans.remove(&id);
        self.draft = Plan::default();
        self.open = None;
    }

    /// Nothing is open any more: the selection, the open rows, the key table and the
    /// audition go. The plans stay.
    pub fn leave(&mut self) {
        self.view = View::default();
        self.open = None;
    }

    /// Decode one root's loudest kept attack stroke, once.
    pub fn decode(&mut self, entity: &Entity, root: u8) {
        self.audio.decode(entity, root);
    }

    /// What a decoded root holds, or the codec's own reason it does not.
    pub fn sound(&self, root: u8) -> Option<Result<Sound<'_>, String>> {
        let facts = self.facts()?;
        Some(match self.audio.get(root)? {
            Ok(played) => Ok(Sound {
                samples: &played.samples,
                channels: played.channels,
                rate: npno::codec::RATE,
                name: crate::workspace::stroke_wav_name(
                    &facts.name,
                    root,
                    Bank::Attack.code(),
                    played.layer,
                ),
            }),
            Err(why) => Err(why.clone()),
        })
    }
}

/// What the header shows over a piano library: what it keeps of what it holds, the word
/// for a library that has been trimmed, and its refusal to be queued when it will not
/// fit.
fn extras(facts: &Facts, plan: &Plan, free: Option<u64>) -> Extras {
    let kept = kept_bytes(facts, plan);
    let over = free
        .and_then(|free| kept.checked_sub(free))
        .filter(|over| *over > 0);
    let trimmed = kept < facts.total;
    Extras {
        size: trimmed.then(|| SizeLine {
            text: room::measure_out_of(kept, facts.total),
            warn: over.is_some(),
            hint: match free {
                Some(free) => format!("{} free on the instrument", room::measure(free)),
                None => "the instrument has not reported its free piano memory".to_string(),
            },
        }),
        edited: trimmed.then(|| StateLine {
            words: "trimmed".to_string(),
            ink: Ink::Warn,
            hint: "the plan drops strokes the saved file holds".to_string(),
        }),
        state: None,
        loud: over.map(|over| Loud {
            label: format!("Won't fit · {} over", room::measure(over)),
            short: format!("{} over", room::measure(over)),
            glyph: Glyph::CircleAlert,
            tone: Tone::Blocked,
            hint: format!("trim {} before this can be queued", room::measure(over)),
            send: None,
        }),
    }
}

// ---- the pieces the sections are painted out of -------------------------------------

/// The corner every rectangle here is drawn with, and the page's own margin.
const RADIUS: f32 = 2.0;
const PAD: f32 = 12.0;
const GAP: f32 = 10.0;

/// A switch row and a root's row are one height; a lane of segments and a head row are
/// shorter.
const ROW: f32 = 26.0;
const LANE: f32 = 20.0;
const HEAD: f32 = 20.0;

/// The words: a row's name, the note beside it, the mono figures, a caps label.
const NAME: f32 = 11.5;
const NOTE: f32 = 10.5;
const MONO: f32 = 10.5;
const MICRO: f32 = 9.5;

/// A lamp's own box, which is what a cell has to leave room for.
const LAMP: egui::Vec2 = egui::vec2(30.0, 20.0);

/// `text` laid out to at most `width` with an ellipsis where it did not fit, painted
/// from `left` and centred on `middle`. Answers with how wide it came out.
fn cell(
    painter: &egui::Painter,
    left: f32,
    middle: f32,
    width: f32,
    text: &str,
    font: egui::FontId,
    ink: egui::Color32,
) -> f32 {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat::simple(font, ink));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    let galley = painter.layout_job(job);
    let wide = galley.size().x;
    painter.galley(
        egui::pos2(left, middle - galley.size().y / 2.0),
        galley,
        ink,
    );
    wide
}

/// A figure ending at `right`, struck through where it is what a size *was*. Answers
/// with the x it reached back to.
fn figure(
    painter: &egui::Painter,
    right: f32,
    middle: f32,
    text: &str,
    font: egui::FontId,
    ink: egui::Color32,
    struck: bool,
) -> f32 {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font,
            color: ink,
            strikethrough: match struck {
                true => egui::Stroke::new(1.0_f32, ink),
                false => egui::Stroke::NONE,
            },
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    let left = right - galley.size().x;
    painter.galley(
        egui::pos2(left, middle - galley.size().y / 2.0),
        galley,
        ink,
    );
    left
}

fn hairline(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
}

/// A lamp inside `box_`, salted so two of them in one frame are two controls.
fn lamp(ui: &mut egui::Ui, box_: egui::Rect, on: bool, salt: impl std::hash::Hash) -> Option<bool> {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(salt)
            .max_rect(box_)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    led::ui(&mut child, on, "")
}

/// One outlined action of an open row.
fn action(ui: &mut egui::Ui, glyph: Glyph, label: &str, mark: egui::Color32) -> bool {
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let word = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(11.0),
        visuals.weak_text_color(),
    );
    let width = 8.0 * 2.0 + 12.0 + 5.0 + word.size().x;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, LANE), egui::Sense::click());
    if response.hovered() {
        painter.rect_filled(rect, RADIUS, visuals.widgets.hovered.weak_bg_fill);
    }
    painter.rect_stroke(
        rect,
        RADIUS,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    painted(
        ui,
        glyph,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 8.0 + 6.0, rect.center().y),
            egui::Vec2::splat(12.0),
        ),
        mark,
    );
    painter.galley(
        egui::pos2(
            rect.left() + 8.0 + 12.0 + 5.0,
            rect.center().y - word.size().y / 2.0,
        ),
        word,
        visuals.weak_text_color(),
    );
    response.clicked()
}

/// A read-only fact of an open row: the caps label, and the value under an eye.
fn read_only(ui: &mut egui::Ui, label: &str, value: &str, hint: &str) {
    let response = ui
        .vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(crate::panel::caps(label).color(app::caption(ui.visuals())));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                icon(ui, Glyph::Eye, 11.0, app::caption(ui.visuals()));
                ui.label(
                    egui::RichText::new(value)
                        .font(egui::FontId::monospace(NAME))
                        .color(ui.visuals().weak_text_color()),
                );
            });
        })
        .response;
    if !hint.is_empty() {
        response.on_hover_text(hint);
    }
}

/// What one root holds, or what it keeps where a plan is given.
fn root_bytes(facts: &Facts, plan: Option<&Plan>, index: usize) -> u64 {
    facts
        .cells
        .iter()
        .filter(|cell| cell.root == index && plan.is_none_or(|plan| plan.keeps(facts, cell)))
        .map(|cell| cell.bytes)
        .sum()
}

/// The keys the plan leaves answering something.
fn covered(facts: &Facts, plan: &Plan) -> Vec<u8> {
    (0..npno::NOTES as u8)
        .filter(|key| {
            plan.range.as_ref().is_none_or(|range| range.contains(key))
                && plan
                    .answers(facts, *key)
                    .is_some_and(|root| root_bytes(facts, Some(plan), root) > 0)
        })
        .collect()
}

/// The stretches of the keyboard the plan leaves silent, low to high.
fn silent(facts: &Facts, plan: &Plan) -> Vec<(u8, u8)> {
    let answered = covered(facts, plan);
    let mut out: Vec<(u8, u8)> = Vec::new();
    for key in SPAN.low..=SPAN.high {
        if answered.contains(&key) {
            continue;
        }
        match out.last_mut() {
            Some((_, top)) if *top + 1 == key => *top = key,
            _ => out.push((key, key)),
        }
    }
    out
}

/// A column `width` wide, laid out top down, claiming only the height its contents
/// take.
///
/// ⚠️ Not `allocate_ui` with a zero height: inside a horizontal layout that hands the
/// child one row's worth of height, every row after the first overflows it — and egui
/// answers an overflowing horizontal by growing the page sideways.
fn column(ui: &mut egui::Ui, width: f32, body: impl FnOnce(&mut egui::Ui)) {
    let height = ui.available_height();
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Min),
        body,
    );
}

/// Megabytes as the lanes print them.
fn mb(bytes: u64) -> f32 {
    bytes as f32 / (1024.0 * 1024.0)
}

// ---- the key map --------------------------------------------------------------------

/// What the keyboard last played, as the line under it reads: whether it sounded, and
/// the sentence.
fn status(facts: &Facts, plan: &Plan, key: u8) -> (bool, String) {
    let silent = |why: &str| (false, format!("{} — {why}", note::name(key)));
    let answers = plan
        .answers(facts, key)
        .filter(|index| plan.in_range(facts, *index))
        .filter(|_| plan.range.as_ref().is_none_or(|range| range.contains(&key)));
    let Some(index) = answers else {
        return silent("no root answers this key; silence.");
    };
    let root = &facts.roots[index];
    let mut attack: Vec<u8> = facts
        .cells
        .iter()
        .filter(|cell| cell.root == index && cell.bank == Some(Bank::Attack))
        .map(|cell| cell.layer)
        .collect();
    attack.sort_unstable();
    let kept: Vec<u8> = attack
        .iter()
        .copied()
        .filter(|layer| plan.keeps_layer(root.note, *layer))
        .collect();
    let said = format!(
        "{} at vel {} → root {} · {}",
        note::name(key),
        keys::AUDITION_VELOCITY,
        note::name(root.note),
        keys::shifted(key, root.note),
    );
    match (attack.first(), kept.first()) {
        (_, None) => silent("every layer of this root is dropped; silence."),
        (Some(loudest), Some(playing)) if playing != loudest => (
            true,
            format!("{said} — the loudest layer is dropped, so the next kept layer plays"),
        ),
        _ => (true, said),
    }
}

/// The damper limit over the keyboard: a marker at the boundary above the highest
/// damped key, and the keys past it drawn quieter — they ring on at note-off. Nothing
/// where every key is damped.
fn damper_mark(ui: &egui::Ui, rect: egui::Rect, top: u8) {
    if top >= ALL_KEYS_DAMPED || top >= SPAN.high {
        return;
    }
    let at = SPAN.x_after(rect, top);
    let visuals = ui.visuals().clone();
    let painter = ui.painter();
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(at, rect.top() + 1.0),
            rect.max - egui::vec2(0.0, 1.0),
        ),
        0.0,
        visuals.window_fill.gamma_multiply(0.45),
    );
    painter.vline(
        at,
        rect.y_range(),
        egui::Stroke::new(1.0_f32, app::caption(&visuals)),
    );
    painter.text(
        egui::pos2(at + 3.0, rect.top() + 1.0),
        egui::Align2::LEFT_TOP,
        "damper",
        egui::FontId::proportional(MICRO),
        app::caption(&visuals),
    );
}

/// Take a dragged lane of root boundaries into the plan's key map: each key the drag
/// moved plays the root whose cell now covers it, or nothing where no cell does.
fn reroute(facts: &Facts, plan: &mut Plan, of_root: &[usize], was: &[(u8, u8)], now: &[(u8, u8)]) {
    let holder = |bounds: &[(u8, u8)], key: u8| {
        bounds
            .iter()
            .position(|(low, top)| (*low..=*top).contains(&key))
    };
    for key in 0..npno::NOTES as u8 {
        let after = holder(now, key);
        if holder(was, key) == after {
            continue;
        }
        plan.route(facts, key, after.map(|cell| of_root[cell]));
    }
}

/// How many silent stretches the map leaves, as the heading reads it.
fn coverage(gaps: &[(u8, u8)]) -> String {
    match gaps.len() {
        0 => "every key answered".to_string(),
        1 => "1 silent range".to_string(),
        n => format!("{n} silent ranges"),
    }
}

impl State {
    /// The key map, pinned above the body: one cell per root over a clickable keyboard,
    /// and a line saying what the last key played.
    pub fn map(&mut self, ui: &mut egui::Ui) -> Option<Ask> {
        let State {
            open, draft, view, ..
        } = self;
        let facts = match &open.as_ref()?.facts {
            Ok(facts) => facts,
            Err(why) => {
                ui.label(egui::RichText::new(why).color(app::bad(ui.visuals())));
                return None;
            }
        };

        let gaps = silent(facts, draft);
        let ink = match gaps.is_empty() {
            true => app::good(ui.visuals()),
            false => app::warn(ui.visuals()),
        };
        let reading = format!(
            "{}  {}–{}",
            coverage(&gaps),
            note::name(SPAN.low),
            note::name(SPAN.high)
        );
        controls::heading(
            ui,
            "Key map",
            "click a key to hear which root answers it",
            Some((&reading, ink)),
        );

        let lit = view.audition.as_ref().map(|struck| struck.note);
        let answering = lit.and_then(|note| draft.answers(facts, note));
        let mut cells = Vec::new();
        let mut of_root = Vec::new();
        for (index, root) in facts.roots.iter().enumerate() {
            let kept = root_bytes(facts, Some(draft), index);
            let original = root_bytes(facts, None, index);
            let in_range = draft.in_range(facts, index);
            for (low, top) in runs(&draft.keys_of(facts, index)) {
                of_root.push(index);
                cells.push(SizeCell {
                    low,
                    top,
                    name: note::name(root.note),
                    kept: mb(kept),
                    original: mb(original),
                    in_range,
                    hint: format!(
                        "root {} answers {}–{} · {} of {}",
                        note::name(root.note),
                        note::name(low),
                        note::name(top),
                        room::measure(kept),
                        room::measure(original),
                    ),
                });
            }
        }
        let marks: Vec<keys::Mark> = facts
            .roots
            .iter()
            .map(|root| keys::Mark {
                note: root.note,
                label: None,
            })
            .collect();

        let mut inner = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(
                    ui.available_rect_before_wrap()
                        .shrink2(egui::vec2(PAD, 0.0)),
                )
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let picked = view
            .picked
            .and_then(|root| of_root.iter().position(|held| *held == root));
        let cell_lit = answering.and_then(|root| of_root.iter().position(|held| *held == root));
        let acted = keys::size_cells(
            &mut inner,
            SPAN,
            &cells,
            picked,
            cell_lit,
            keys::Edges::Both,
        );
        let keyboard = egui::Rect::from_min_size(
            inner.next_widget_position(),
            egui::vec2(inner.available_width().max(1.0), keys::KEYBOARD_H),
        );
        let struck = keys::keyboard(&mut inner, SPAN, lit, &marks);
        damper_mark(
            &inner,
            keyboard,
            draft.damper_top.unwrap_or(facts.damper_top),
        );
        let room = inner.available_rect_before_wrap().width();
        let (line, _) = inner.allocate_exact_size(egui::vec2(room, LANE), egui::Sense::hover());
        if let Some(note) = lit {
            let (good, said) = status(facts, draft, note);
            let (glyph, ink) = match good {
                true => (Glyph::AudioLines, app::good(inner.visuals())),
                false => (Glyph::CircleAlert, app::warn(inner.visuals())),
            };
            painted(
                &inner,
                glyph,
                egui::Rect::from_center_size(
                    egui::pos2(line.left() + 6.0, line.center().y),
                    egui::Vec2::splat(12.0),
                ),
                ink,
            );
            cell(
                inner.painter(),
                line.left() + 19.0,
                line.center().y,
                line.width() - 19.0,
                &said,
                egui::FontId::proportional(11.0),
                inner.visuals().weak_text_color(),
            );
        }
        let drawn = inner.min_rect();
        ui.advance_cursor_after_rect(drawn);
        hairline(ui, drawn.expand2(egui::vec2(0.0, 4.0)));

        match acted {
            Some(keys::BandAct::Pick(index)) => {
                let root = of_root[index];
                view.picked = Some(root);
                view.open_rows.insert(root);
                view.reveal = Some(root);
            }
            Some(keys::BandAct::Drag { bounds, .. }) => {
                let was: Vec<(u8, u8)> = cells.iter().map(|cell| (cell.low, cell.top)).collect();
                reroute(facts, draft, &of_root, &was, &bounds);
            }
            None => {}
        }
        let note = struck?;
        view.audition = Some(Audition::new(note, ui.input(|input| input.time)));
        // Only a key that sounds is asked for. A root with nothing left to play would
        // answer with the codec's refusal, and the line under the keyboard is where
        // silence is explained.
        let (sounds, _) = status(facts, draft, note);
        let root = draft.answers(facts, note).filter(|_| sounds)?;
        let root = facts.roots[root].note;
        Some(Ask::Strike {
            root,
            semitones: i16::from(note) - i16::from(root),
        })
    }

    /// Let go of an audition whose hold is up, and answer with how long a live one has
    /// left — the caller asks for the frame that will clear it.
    pub fn settle(&mut self, now: f64) -> Option<f64> {
        let struck = self.view.audition.as_ref()?;
        let left = Audition::HOLD - (now - struck.started);
        if left <= 0.0 {
            self.view.audition = None;
            return None;
        }
        Some(left)
    }
}

// ---- the trim section ---------------------------------------------------------------

/// One switch of the trim section as its row draws it.
struct Switch {
    on: bool,
    name: String,
    note: String,
    /// Whether the note is a warning rather than a fact.
    loud: bool,
    size: String,
    /// What the size was, where the plan has changed it.
    was: Option<String>,
    hint: String,
    what: Option<What>,
}

/// The switch rows of the trim section: what the plan keeps, against what it holds.
fn switches(ui: &mut egui::Ui, facts: &Facts, plan: &mut Plan) {
    let layers = facts.layers.len();
    let whole = spans(&facts.layers);
    let mut rows: Vec<Switch> = Vec::new();
    for (rank, layer) in facts.shown_layers() {
        let on_roots = facts
            .roots
            .iter()
            .enumerate()
            .filter(|(index, root)| {
                facts
                    .cells
                    .iter()
                    .any(|cell| cell.root == *index && cell.layer == layer)
                    && plan.in_range(facts, *index)
                    && plan.keeps_layer(root.note, layer)
            })
            .count();
        let holds = facts
            .roots
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                facts
                    .cells
                    .iter()
                    .any(|cell| cell.root == *index && cell.layer == layer)
            })
            .count();
        let kept = layer_bytes(facts, Some(plan), layer);
        let held = layer_bytes(facts, None, layer);
        let partial = on_roots > 0 && on_roots < holds;
        rows.push(Switch {
            on: on_roots > 0,
            name: format!("{} layer", layer_name(rank, layers)),
            note: match (partial, rank) {
                (true, _) => format!("on {on_roots} of {holds} roots"),
                (false, 0) => "the loudest strokes".to_string(),
                (false, rank) if rank + 1 == layers => {
                    "the quietest layer — first thing a trim loses".to_string()
                }
                (false, _) => String::new(),
            },
            loud: partial,
            size: room::measure(kept),
            was: (kept != held).then(|| room::measure(held)),
            hint: format!(
                "layer index {layer} in the file — 0 is the loudest; a root holding \
                 every layer sounds it at {}",
                span_text(&whole[rank].1)
            ),
            what: Some(What::Layer(layer)),
        });
    }
    for bank in &facts.banks {
        if *bank == Bank::Attack {
            continue;
        }
        let kept = bank_bytes(facts, Some(plan), *bank);
        let held = bank_bytes(facts, None, *bank);
        rows.push(Switch {
            on: plan.keeps_bank(Some(*bank)),
            name: bank_name(*bank).to_string(),
            note: bank_note(*bank).to_string(),
            loud: false,
            size: room::measure(kept),
            was: (kept != held).then(|| room::measure(held)),
            hint: format!("bank {} in the file", bank.code()),
            what: Some(What::Bank(*bank)),
        });
    }
    rows.push(range_switch(facts, plan));

    let mut thrown = None;
    for (index, row) in rows.iter().enumerate() {
        if let Some(keep) = switch_row(ui, index, row) {
            thrown = Some((row.what, keep));
        }
    }
    match thrown {
        Some((Some(What::Layer(layer)), keep)) => plan.switch_layer(layer, keep),
        Some((Some(What::Bank(bank)), keep)) => plan.switch_bank(bank, keep),
        Some((None, keep)) => {
            plan.range = (!keep).then(|| default_range(facts));
        }
        None => {}
    }
}

/// The Keys row: the whole range, or the middle of it.
fn range_switch(facts: &Facts, plan: &Plan) -> Switch {
    let (low, high) = (
        facts.covered.first().copied().unwrap_or(SPAN.low),
        facts.covered.last().copied().unwrap_or(SPAN.high),
    );
    let held = facts.covered.len();
    let kept = covered(facts, plan).len();
    Switch {
        on: plan.range.is_none(),
        name: format!("Keys {} – {}", note::name(low), note::name(high)),
        note: match &plan.range {
            None => "the library's whole range".to_string(),
            Some(range) => {
                let dropped = (0..facts.roots.len())
                    .filter(|index| !plan.in_range(facts, *index))
                    .count();
                format!(
                    "trimmed to {} – {} · {dropped} roots dropped",
                    note::name(*range.start()),
                    note::name(*range.end())
                )
            }
        },
        loud: plan.range.is_some(),
        size: format!("{kept} keys"),
        was: (kept != held).then(|| format!("{held} keys")),
        hint: "switch it off to keep the middle of the keyboard".to_string(),
        what: None,
    }
}

/// What one layer holds, or what it keeps where a plan is given.
fn layer_bytes(facts: &Facts, plan: Option<&Plan>, layer: u8) -> u64 {
    facts
        .cells
        .iter()
        .filter(|cell| cell.layer == layer && plan.is_none_or(|plan| plan.keeps(facts, cell)))
        .map(|cell| cell.bytes)
        .sum()
}

/// What one bank holds, or what it keeps where a plan is given.
fn bank_bytes(facts: &Facts, plan: Option<&Plan>, bank: Bank) -> u64 {
    facts
        .cells
        .iter()
        .filter(|cell| cell.bank == Some(bank) && plan.is_none_or(|plan| plan.keeps(facts, cell)))
        .map(|cell| cell.bytes)
        .sum()
}

/// A lamp, a name, a note and a size, with a hairline under them.
fn switch_row(ui: &mut egui::Ui, index: usize, row: &Switch) -> Option<bool> {
    const SIZE_W: f32 = 92.0;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::hover());
    hairline(ui, rect);
    // ⚠️ Before the lamp: egui gives a click to the last widget registered over it, and
    // a row-wide target added afterwards would swallow every throw of the switch.
    ui.interact(
        rect,
        ui.id().with(("trim_row", index)),
        egui::Sense::hover(),
    )
    .on_hover_text(&row.hint);
    let visuals = ui.visuals().clone();
    let ink = match row.on {
        true => visuals.weak_text_color(),
        false => app::caption(&visuals),
    };
    let rest = (rect.width() - LAMP.x - SIZE_W - GAP * 3.0).max(0.0);
    let name_w = rest / 2.2;
    let middle = rect.center().y;

    let thrown = lamp(
        ui,
        egui::Rect::from_min_size(egui::pos2(rect.left(), middle - LAMP.y / 2.0), LAMP),
        row.on,
        ("trim", index),
    );
    let painter = ui.painter().clone();
    let left = rect.left() + LAMP.x + GAP;
    cell(
        &painter,
        left,
        middle,
        name_w,
        &row.name,
        egui::FontId::proportional(NAME),
        ink,
    );
    let note_ink = match row.loud {
        true => app::warn(&visuals),
        false => app::caption(&visuals),
    };
    cell(
        &painter,
        left + name_w + GAP,
        middle,
        rest - name_w,
        &row.note,
        egui::FontId::proportional(NOTE),
        note_ink,
    );
    let mono = egui::FontId::monospace(MONO);
    let back = figure(
        &painter,
        rect.right(),
        middle,
        &row.size,
        mono.clone(),
        ink,
        false,
    );
    if let Some(was) = &row.was {
        figure(
            &painter,
            back - 5.0,
            middle,
            was,
            mono,
            app::caption(&visuals),
            true,
        );
    }
    thrown
}

/// The meter: what the file holds, what the plan keeps, what the instrument has free,
/// and the sentence that names the cheapest cut left.
fn meter(ui: &mut egui::Ui, facts: &Facts, plan: &Plan, free: Option<u64>) {
    const TROUGH: f32 = 8.0;
    const LABEL: f32 = 14.0;

    let kept = kept_bytes(facts, plan);
    let visuals = ui.visuals().clone();
    let width = ui.available_width();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} in the file", room::measure(facts.total)))
                .size(11.0)
                .color(visuals.weak_text_color()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{} kept", room::measure(kept)))
                    .font(egui::FontId::monospace(NAME))
                    .color(visuals.text_color()),
            );
        });
    });
    ui.add_space(7.0);

    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, TROUGH + LABEL), egui::Sense::hover());
    let trough = egui::Rect::from_min_size(rect.min, egui::vec2(width, TROUGH));
    let painter = ui.painter().clone();
    painter.rect_filled(trough, RADIUS, visuals.extreme_bg_color);
    let share = |bytes: u64| match facts.total {
        0 => 0.0,
        total => (bytes as f32 / total as f32).clamp(0.0, 1.0) * width,
    };
    let fits = free.is_none_or(|free| kept <= free);
    let filled = share(kept);
    if filled > 0.0 {
        painter.rect_filled(
            egui::Rect::from_min_size(trough.min, egui::vec2(filled, TROUGH)),
            RADIUS,
            match fits {
                true => app::good(&visuals),
                false => app::bad(&visuals),
            },
        );
    }
    let ghost =
        egui::Rect::from_min_max(egui::pos2(trough.left() + filled, trough.top()), trough.max);
    if ghost.width() > 0.0 {
        keys::hatch(&painter, ghost, app::unlit(&visuals), 1.0);
        ui.interact(ghost, ui.id().with("piano_dropped"), egui::Sense::hover())
            .on_hover_text(format!(
                "{} dropped",
                room::measure(facts.total.saturating_sub(kept))
            ));
    }
    if let Some(free) = free {
        let at = trough.left() + share(free);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(at, trough.top() - 3.0),
                egui::pos2(at + 2.0, trough.bottom() + 3.0),
            ),
            0.0,
            visuals.text_color(),
        );
        painter.text(
            egui::pos2(at, trough.bottom() + 4.0),
            egui::Align2::CENTER_TOP,
            format!("{} free", room::measure(free)),
            egui::FontId::monospace(MICRO),
            visuals.weak_text_color(),
        );
    }

    ui.add_space(10.0);
    let (said, loud) = constraint(facts, plan, free);
    ui.label(egui::RichText::new(said).size(11.0).color(match loud {
        true => app::warn(&visuals),
        false => app::caption(&visuals),
    }));
}

/// The sentence under the meter: whether it fits, and what to throw if it does not.
fn constraint(facts: &Facts, plan: &Plan, free: Option<u64>) -> (String, bool) {
    let kept = kept_bytes(facts, plan);
    let Some(free) = free else {
        return (
            "The instrument has not reported its free piano memory, so nothing here can \
             say whether this fits."
                .to_string(),
            false,
        );
    };
    let Some(over) = kept.checked_sub(free).filter(|over| *over > 0) else {
        return (
            format!(
                "At {} it fits with {} to spare.",
                room::measure(kept),
                room::measure(free - kept)
            ),
            false,
        );
    };
    let head = format!(
        "At {} it is {} over.",
        room::measure(kept),
        room::measure(over)
    );
    let rest = match cheapest_cut(facts, plan, over) {
        Some(cut) => format!(
            "Dropping {} sheds {} — the cheapest cut left that fits.",
            listed(&cut.picked),
            room::measure(cut.shed)
        ),
        None if plan.range.is_some() => {
            "Everything droppable is off and it still does not fit; this library cannot go \
             on this instrument."
                .to_string()
        }
        None => "Everything droppable is off — narrowing the key range is the only cut left."
            .to_string(),
    };
    (format!("{head} {rest}"), true)
}

// ---- the playback fields ------------------------------------------------------------

/// A control of the playback section: the caps label, and the control under it.
fn field(ui: &mut egui::Ui, label: &str, hint: &str, body: impl FnOnce(&mut egui::Ui)) {
    let response = ui
        .vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(crate::panel::caps(label).color(app::caption(ui.visuals())));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                body(ui);
            });
        })
        .response;
    response.on_hover_text(hint);
}

/// What the instrument applies over every stroke: a gain over the whole library, the
/// key the damper still reaches, and the kind of instrument the library is filed under.
fn playback(ui: &mut egui::Ui, facts: &Facts, plan: &mut Plan) {
    ui.spacing_mut().item_spacing = egui::vec2(24.0, 10.0);
    ui.horizontal_wrapped(|ui| {
        field(ui, "Instrument gain", "0x40c, tenths of a decibel", |ui| {
            let mut decibels = f64::from(plan.gain.unwrap_or(facts.gain)) / 10.0;
            let moved = ui.add(
                egui::Slider::new(&mut decibels, -12.7..=12.7)
                    .step_by(0.1)
                    .custom_formatter(|value, _| format!("{value:+.1} dB")),
            );
            if moved.changed() {
                let tenths = (decibels * 10.0).round() as i8;
                plan.gain = (tenths != facts.gain).then_some(tenths);
            }
        });
        field(
            ui,
            "Damper reaches",
            "0x40d: keys above ring on at note-off",
            |ui| {
                let held = plan.damper_top.unwrap_or(facts.damper_top);
                if let Some(key) = note_picker(ui, ("piano_damper", 0), held) {
                    plan.damper_top = (key != facts.damper_top).then_some(key);
                }
                let mut every = held >= ALL_KEYS_DAMPED;
                if ui.checkbox(&mut every, "every key").changed() {
                    let key = match every {
                        true => ALL_KEYS_DAMPED,
                        false => SPAN.high,
                    };
                    plan.damper_top = (key != facts.damper_top).then_some(key);
                }
            },
        );
        field(
            ui,
            "Kind",
            "0x18; the instrument files the library under it — it changes nothing the \
             library sounds like, and picking one only defaults the damper limit",
            |ui| {
                let held = plan.kind.map_or(facts.kind, |kind| kind.code());
                let mut picked = Kind::from_code(held);
                egui::ComboBox::from_id_salt("piano_kind")
                    .selected_text(kind_name(held))
                    .show_ui(ui, |ui| {
                        for kind in Kind::ALL {
                            ui.selectable_value(&mut picked, Some(kind), kind_name(kind.code()));
                        }
                    });
                let Some(kind) = picked.filter(|kind| kind.code() != held) else {
                    return;
                };
                if plan.damper_top.is_none() {
                    let top = kind.damper_top();
                    plan.damper_top = (top != facts.damper_top).then_some(top);
                }
                plan.kind = (kind.code() != facts.kind).then_some(kind);
            },
        );
    });
}

// ---- the velocity layer lanes -------------------------------------------------------

/// One lane per layer: the switch that speaks for every root, and one segment per root
/// so a per-root exception shows where it is.
fn lanes(ui: &mut egui::Ui, facts: &Facts, plan: &mut Plan, picked: Option<usize>) {
    const LEFT: f32 = 88.0;
    const RIGHT: f32 = 150.0;
    const SEG_H: f32 = 16.0;
    /// How wide a segment must be before its velocities are worth printing in it.
    const SPAN_W: f32 = 48.0;

    let layers = facts.layers.len();
    // A root missing a layer the file holds answers over the gap it leaves, so the
    // spans are the ones that root's own values give.
    let held: Vec<Vec<(u8, RangeInclusive<u8>)>> = (0..facts.roots.len())
        .map(|index| spans(&root_layers(facts, index)))
        .collect();
    let mut thrown: Option<(u8, bool)> = None;
    let mut excepted: Option<(u8, u8, bool)> = None;
    for (rank, layer) in facts.shown_layers() {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), LANE), egui::Sense::hover());
        let visuals = ui.visuals().clone();
        let on: Vec<bool> = facts
            .roots
            .iter()
            .enumerate()
            .map(|(index, root)| plan.in_range(facts, index) && plan.keeps_layer(root.note, layer))
            .collect();
        let kept_roots = on.iter().filter(|kept| **kept).count();
        let all = kept_roots == facts.roots.len();
        let word = match (kept_roots, all) {
            (0, _) => "drop",
            (_, true) => "keep",
            _ => "partial",
        };
        let ink = match all || kept_roots == 0 {
            true => visuals.weak_text_color(),
            false => app::warn(&visuals),
        };
        if let Some(keep) = lamp(
            ui,
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.center().y - LAMP.y / 2.0),
                LAMP,
            ),
            kept_roots > 0,
            ("lane", layer),
        ) {
            thrown = Some((layer, keep));
        }
        let painter = ui.painter().clone();
        cell(
            &painter,
            rect.left() + LAMP.x + GAP,
            rect.center().y,
            LEFT - LAMP.x - GAP,
            word,
            egui::FontId::proportional(NOTE),
            ink,
        );

        let lane = egui::Rect::from_min_max(
            egui::pos2(rect.left() + LEFT, rect.center().y - SEG_H / 2.0),
            egui::pos2(rect.right() - RIGHT, rect.center().y + SEG_H / 2.0),
        );
        let step = lane.width() / facts.roots.len().max(1) as f32;
        let pointer = ui.input(|input| input.pointer.interact_pos());
        for (index, root) in facts.roots.iter().enumerate() {
            let seg = egui::Rect::from_min_size(
                egui::pos2(lane.left() + step * index as f32, lane.top()),
                egui::vec2((step - 1.0).max(1.0), SEG_H),
            );
            let kept = on[index];
            match kept {
                true => {
                    painter.rect_filled(
                        seg,
                        RADIUS,
                        match picked == Some(index) {
                            true => visuals.selection.bg_fill,
                            false => visuals.faint_bg_color,
                        },
                    );
                    painter.rect_stroke(
                        seg,
                        RADIUS,
                        egui::Stroke::new(
                            1.0_f32,
                            match picked == Some(index) {
                                true => app::accent(&visuals),
                                false => visuals.widgets.noninteractive.bg_stroke.color,
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                }
                false => crate::panel::dashed_rect(
                    &painter,
                    seg,
                    egui::Stroke::new(1.0_f32, app::unlit(&visuals)),
                ),
            }
            let span = held[index]
                .iter()
                .find(|(value, _)| *value == layer)
                .map(|(_, span)| span_text(span))
                .unwrap_or_default();
            if seg.width() >= SPAN_W && !span.is_empty() {
                cell(
                    &painter,
                    seg.left() + 4.0,
                    seg.center().y,
                    seg.width() - 8.0,
                    &span,
                    egui::FontId::monospace(MICRO),
                    match kept {
                        true => visuals.weak_text_color(),
                        false => app::unlit(&visuals),
                    },
                );
            }
            if !pointer.is_some_and(|at| seg.contains(at)) {
                continue;
            }
            let mut said = format!(
                "{} on {} — {} · {}",
                layer_name(rank, layers),
                note::name(root.note),
                match kept {
                    true => "kept · click to drop",
                    false => "dropped · click to keep",
                },
                room::measure(root_layer_bytes(facts, index, layer)),
            );
            if !span.is_empty() {
                said.push_str(&format!(" · {span}"));
            }
            if let Some(decibels) = segment_trim(facts, plan, index, layer) {
                said.push_str(&format!(" · trim {decibels} dB"));
            }
            let response = ui
                .interact(
                    seg,
                    ui.id().with(("segment", layer, root.note)),
                    egui::Sense::click(),
                )
                .on_hover_text(said);
            if response.clicked() {
                excepted = Some((root.note, layer, !kept));
            }
        }

        let kept = layer_bytes(facts, Some(plan), layer);
        let held = layer_bytes(facts, None, layer);
        figure(
            &painter,
            rect.right(),
            rect.center().y,
            &match kept == held {
                true => format!("{} · {}", layer_name(rank, layers), room::measure(held)),
                false => format!(
                    "{} · {} of {}",
                    layer_name(rank, layers),
                    room::measure(kept),
                    room::measure(held)
                ),
            },
            egui::FontId::monospace(10.0),
            match kept == held {
                true => app::caption(&visuals),
                false => app::warn(&visuals),
            },
            false,
        );
        ui.add_space(3.0);
    }
    if let Some((layer, keep)) = thrown {
        plan.switch_layer(layer, keep);
    }
    if let Some((root, layer, keep)) = excepted {
        plan.roots.insert((root, layer), keep);
    }
}

/// The layer values one root holds, ascending.
fn root_layers(facts: &Facts, root: usize) -> Vec<u8> {
    let mut values: Vec<u8> = facts
        .cells
        .iter()
        .filter(|cell| cell.root == root)
        .map(|cell| cell.layer)
        .collect();
    values.sort_unstable();
    values.dedup();
    values
}

/// What a stroke is attenuated by under this plan, in decibels.
fn trim_of(facts: &Facts, plan: &Plan, strike: &Strike) -> u16 {
    let key = (facts.roots[strike.root].note, strike.bank, strike.layer);
    plan.trims.get(&key).copied().unwrap_or(strike.trim)
}

/// The trim a lane's segment reads: one segment covers a root's layer in every bank
/// that records it, and the attack stroke is the one a key sounds.
fn segment_trim(facts: &Facts, plan: &Plan, root: usize, layer: u8) -> Option<u16> {
    facts
        .strikes
        .iter()
        .filter(|strike| strike.root == root && strike.layer == layer)
        .min_by_key(|strike| strike.bank)
        .map(|strike| trim_of(facts, plan, strike))
}

/// What one root's layer holds, across every bank that records it.
fn root_layer_bytes(facts: &Facts, root: usize, layer: u8) -> u64 {
    facts
        .cells
        .iter()
        .filter(|cell| cell.root == root && cell.layer == layer)
        .map(|cell| cell.bytes)
        .sum()
}

// ---- the roots rows -----------------------------------------------------------------

/// The ink every cell of a row wears.
///
/// ⚠️ A selected row is one colour throughout, warn text included: a row lit by the
/// selection fill and carrying a warning in warn ink reads as two rows.
struct RowInk {
    text: egui::Color32,
    loud: egui::Color32,
    quiet: egui::Color32,
}

fn row_ink(visuals: &egui::Visuals, picked: bool, in_range: bool) -> RowInk {
    match (picked, in_range) {
        (true, _) => RowInk {
            text: visuals.selection.stroke.color,
            loud: visuals.selection.stroke.color,
            quiet: visuals.selection.stroke.color,
        },
        (false, true) => RowInk {
            text: visuals.weak_text_color(),
            loud: app::warn(visuals),
            quiet: app::caption(visuals),
        },
        (false, false) => RowInk {
            text: app::caption(visuals),
            loud: app::caption(visuals),
            quiet: app::caption(visuals),
        },
    }
}

/// How many lamps a root's row carries before the layers are better read in the lanes
/// above it.
const ROW_LAMPS: usize = 6;

/// One row per root: what it answers, which of its layers it keeps, what it costs, and
/// the facts and actions under it when it is open.
fn roots(
    ui: &mut egui::Ui,
    facts: &Facts,
    plan: &mut Plan,
    view: &mut View,
    sounding: Option<u8>,
) -> Option<Ask> {
    const COL_A: f32 = 56.0;
    const SIZE_W: f32 = 74.0;
    const CHEVRON: f32 = 20.0;

    let head = ui
        .allocate_exact_size(egui::vec2(ui.available_width(), HEAD), egui::Sense::hover())
        .0;
    hairline(ui, head);
    let quiet = app::caption(ui.visuals());
    let rest = (head.width() - COL_A - SIZE_W - CHEVRON - GAP * 4.0).max(0.0);
    let answers_w = rest / 2.6 * 1.1;
    let layers_w = rest - answers_w;
    {
        let painter = ui.painter().clone();
        let mut at = head.left() + PAD;
        for (label, width) in [
            ("ROOT", COL_A),
            ("ANSWERS", answers_w),
            ("LAYERS", layers_w),
        ] {
            cell(
                &painter,
                at,
                head.center().y,
                width,
                label,
                egui::FontId::proportional(9.0),
                quiet,
            );
            at += width + GAP;
        }
        figure(
            &painter,
            head.right() - PAD - CHEVRON - GAP,
            head.center().y,
            "SIZE",
            egui::FontId::proportional(9.0),
            quiet,
            false,
        );
    }

    let mut ask = None;
    let mut clicked: Option<usize> = None;
    let mut excepted: Option<(u8, u8, bool)> = None;
    let mut dropped: Option<usize> = None;
    let layers = facts.shown_layers();
    for (index, root) in facts.roots.iter().enumerate() {
        let picked = view.picked == Some(index);
        let open = picked && view.open_rows.contains(&index);
        let in_range = plan.in_range(facts, index);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::hover());
        // ⚠️ Before the lamps drawn over it: egui gives a click to the last widget
        // registered over the point, and a row-wide target added afterwards would
        // swallow every one of them.
        let response = ui
            .interact(
                rect,
                ui.id().with(("root_row", root.note)),
                egui::Sense::click(),
            )
            .on_hover_text("one root, its own strokes; the layer index is the file's");
        let visuals = ui.visuals().clone();
        let ink = row_ink(&visuals, picked, in_range);
        let painter = ui.painter().clone();
        if picked {
            painter.rect_filled(rect, 0.0, visuals.selection.bg_fill);
        }
        hairline(ui, rect);
        let middle = rect.center().y;
        let left = rect.left() + PAD;
        painter.circle_filled(
            egui::pos2(left + 3.0, middle),
            3.0,
            match in_range {
                true => app::accent(&visuals),
                false => app::unlit(&visuals),
            },
        );
        cell(
            &painter,
            left + 12.0,
            middle,
            COL_A - 12.0,
            &note::name(root.note),
            egui::FontId::new(NAME, app::bold()),
            ink.text,
        );
        let runs = runs(&plan.keys_of(facts, index));
        let answers = match runs.as_slice() {
            [(low, top)] => format!("{} – {}", note::name(*low), note::name(*top)),
            runs => runs
                .iter()
                .map(|(low, top)| format!("{}–{}", note::name(*low), note::name(*top)))
                .collect::<Vec<_>>()
                .join(", "),
        };
        cell(
            &painter,
            left + COL_A + GAP,
            middle,
            answers_w,
            &answers,
            egui::FontId::monospace(11.0),
            ink.text,
        );

        let mut at = left + COL_A + GAP + answers_w + GAP;
        let lamps = layers.len() <= ROW_LAMPS;
        if lamps {
            for (rank, layer) in &layers {
                let kept = in_range && plan.keeps_layer(root.note, *layer);
                if let Some(keep) = lamp(
                    ui,
                    egui::Rect::from_min_size(egui::pos2(at, middle - LAMP.y / 2.0), LAMP),
                    kept,
                    ("root_lamp", root.note, layer),
                ) {
                    excepted = Some((root.note, *layer, keep));
                }
                at += LAMP.x + 2.0;
                at += cell(
                    &painter,
                    at,
                    middle,
                    12.0,
                    &layer_short(*rank, facts.layers.len()),
                    egui::FontId::proportional(MICRO),
                    ink.text,
                ) + GAP;
            }
        }
        let on = layers
            .iter()
            .filter(|(_, layer)| plan.keeps_layer(root.note, *layer))
            .count();
        let said = match (in_range, on == layers.len()) {
            (false, _) => "outside the trimmed range".to_string(),
            (true, false) => format!("{on} of {}", layers.len()),
            (true, true) => String::new(),
        };
        cell(
            &painter,
            at,
            middle,
            (left + COL_A + GAP + answers_w + GAP + layers_w - at).max(0.0),
            &said,
            egui::FontId::proportional(11.0),
            match in_range {
                true => ink.loud,
                false => ink.quiet,
            },
        );

        let kept = root_bytes(facts, Some(plan), index);
        let held = root_bytes(facts, None, index);
        let right = rect.right() - PAD - CHEVRON - GAP;
        let mono = egui::FontId::monospace(MONO);
        let back = figure(
            &painter,
            right,
            middle,
            &room::measure(kept),
            mono.clone(),
            ink.text,
            false,
        );
        if kept != held {
            figure(
                &painter,
                back - 5.0,
                middle,
                &room::measure(held),
                mono,
                ink.quiet,
                true,
            );
        }
        painted(
            ui,
            match open {
                true => Glyph::ChevronDown,
                false => Glyph::ChevronRight,
            },
            egui::Rect::from_center_size(
                egui::pos2(rect.right() - PAD - CHEVRON / 2.0, middle),
                egui::Vec2::splat(12.0),
            ),
            ink.quiet,
        );
        if response.clicked() {
            clicked = Some(index);
        }
        if view.reveal == Some(index) {
            ui.scroll_to_rect(rect, Some(egui::Align::TOP));
        }

        if open {
            if let Some(asked) = open_row(ui, facts, plan, index, &runs, sounding) {
                match asked {
                    Opened::Audio(asked) => ask = Some(asked),
                    Opened::Drop => dropped = Some(index),
                }
            }
        }
    }
    view.reveal = None;
    if let Some(index) = clicked {
        match view.picked == Some(index) && view.open_rows.contains(&index) {
            true => {
                view.open_rows.remove(&index);
            }
            false => {
                view.open_rows.insert(index);
            }
        }
        view.picked = Some(index);
    }
    if let Some((root, layer, keep)) = excepted {
        plan.roots.insert((root, layer), keep);
    }
    if let Some(index) = dropped {
        for (_, layer) in &layers {
            plan.roots.insert((facts.roots[index].note, *layer), false);
        }
    }
    ask
}

/// What an open root's row asked for.
enum Opened {
    Audio(Ask),
    Drop,
}

/// The facts and the actions under an open root: what the file says about it, and the
/// three things that can be done to it.
fn open_row(
    ui: &mut egui::Ui,
    facts: &Facts,
    plan: &mut Plan,
    index: usize,
    runs: &[(u8, u8)],
    sounding: Option<u8>,
) -> Option<Opened> {
    let mut asked = None;
    let root = &facts.roots[index];
    let (low, top) = (
        runs.first().map_or(root.note, |(low, _)| *low),
        runs.last().map_or(root.note, |(_, top)| *top),
    );
    let cents = f32::from(plan.tune(facts, root.note)) * FINE_TUNE_CENTS_PER_UNIT;
    egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .inner_margin(egui::Margin {
            left: 68,
            right: 12,
            top: 10,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(20.0, 10.0);
            ui.horizontal_wrapped(|ui| {
                read_only(ui, "Root key", &note::name(root.note), "the recorded note");
                let dragged = "drag the root's boundary on the key map to move it";
                read_only(ui, "Answers from", &note::name(low), dragged);
                read_only(ui, "up to", &note::name(top), dragged);
                read_only(
                    ui,
                    "Channels",
                    facts.channel_word(),
                    "per file, not per stroke",
                );
                read_only(
                    ui,
                    "Fine tune",
                    &format!("{cents:+.1} c"),
                    "the per-key lane below is what edits it",
                );
            });
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 6.0);
            let mut strikes: Vec<&Strike> = facts
                .strikes
                .iter()
                .filter(|strike| strike.root == index)
                .collect();
            strikes.sort_by_key(|strike| (strike.bank, strike.layer));
            for strike in strikes {
                let rank = facts
                    .layers
                    .iter()
                    .position(|value| *value == strike.layer)
                    .unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        egui::vec2(170.0, LANE),
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "{} · {}",
                                bank_word(strike.bank),
                                layer_name(rank, facts.layers.len())
                            ))
                            .size(NAME)
                            .color(ui.visuals().weak_text_color()),
                        )
                        .halign(egui::Align::LEFT),
                    );
                    let mut decibels = f64::from(trim_of(facts, plan, strike));
                    let set = ui
                        .add(
                            egui::DragValue::new(&mut decibels)
                                .range(0.0..=48.0)
                                .speed(0.1)
                                .suffix(" dB"),
                        )
                        .on_hover_text("+0x34 in the stroke record, 1 dB a unit");
                    if set.changed() {
                        let key = (root.note, strike.bank, strike.layer);
                        let picked = decibels.round() as u16;
                        match picked == strike.trim {
                            true => plan.trims.remove(&key),
                            false => plan.trims.insert(key, picked),
                        };
                    }
                    ui.label(
                        egui::RichText::new(match strike.decay {
                            Some(rate) => format!("decay {rate:.1} dB/s"),
                            None => "decay none".to_string(),
                        })
                        .font(egui::FontId::monospace(MONO))
                        .color(app::caption(ui.visuals())),
                    )
                    .on_hover_text("the fourteen coefficients at +0x36, carried as read");
                });
            }
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            ui.horizontal_wrapped(|ui| {
                let playing = sounding == Some(root.note);
                let (glyph, label) = match playing {
                    true => (Glyph::X, "Stop"),
                    false => (Glyph::AudioLines, "Audition"),
                };
                if action(ui, glyph, label, app::accent(ui.visuals())) {
                    asked = Some(Opened::Audio(Ask::Play(root.note)));
                }
                if action(ui, Glyph::Waves, "Save WAV…", app::caption(ui.visuals())) {
                    asked = Some(Opened::Audio(Ask::Save(root.note)));
                }
                if action(ui, Glyph::X, "Drop this root", app::warn(ui.visuals())) {
                    asked = Some(Opened::Drop);
                }
            });
        });
    asked
}

// ---- the per-key lane ---------------------------------------------------------------

/// What the lane draws a key's tune at, where full deflection is [`TUNE_CENTS`].
const TUNE_CENTS: i32 = 25;

/// A tune in file units as the lane's own reach, and back again.
///
/// The file stores units and the lane shows cents, so the two conversions are the one
/// place that knows which is which.
fn reach(units: i8) -> f32 {
    f32::from(units) * FINE_TUNE_CENTS_PER_UNIT / TUNE_CENTS as f32
}

fn units(reach: f32) -> i8 {
    let units = (reach * TUNE_CENTS as f32 / FINE_TUNE_CENTS_PER_UNIT).round();
    units.clamp(f32::from(i8::MIN), f32::from(i8::MAX)) as i8
}

/// The fine tune lane: one bar per key, drawn over by a drag across it.
fn per_key(ui: &mut egui::Ui, facts: &Facts, plan: &mut Plan, view: &mut View) {
    const LABEL: f32 = 80.0;
    const AXIS: f32 = 34.0;
    const RIGHT: f32 = 150.0;

    let values: Vec<f32> = (SPAN.low..=SPAN.high)
        .map(|key| reach(plan.tune(facts, key)))
        .collect();
    let edited: Vec<bool> = (SPAN.low..=SPAN.high)
        .map(|key| plan.tune(facts, key) != facts.tune(key))
        .collect();
    let count = edited.iter().filter(|held| **held).count();

    let painted_keys = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_sized(
                egui::vec2(LABEL, LANE),
                egui::Label::new(
                    egui::RichText::new("Fine tune")
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                )
                .halign(egui::Align::LEFT),
            );
            let (top, bottom) = Scale::Cents(TUNE_CENTS).axis_labels();
            ui.allocate_ui(egui::vec2(AXIS, 38.0), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
                    for label in [top.as_str(), "0", bottom.as_str()] {
                        ui.label(
                            egui::RichText::new(label)
                                .font(egui::FontId::monospace(9.0))
                                .color(app::caption(ui.visuals())),
                        );
                    }
                });
            });
            let lane = ui.available_width() - RIGHT - 8.0;
            let drawn = ui
                .allocate_ui(egui::vec2(lane.max(1.0), 38.0), |ui| {
                    keys::lane(ui, SPAN, &values, &edited, Scale::Cents(TUNE_CENTS))
                })
                .inner;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let said = match count {
                    0 => format!("{} keys · ±{TUNE_CENTS} c", SPAN.keys()),
                    n => format!("{} keys · ±{TUNE_CENTS} c · {n} edited", SPAN.keys()),
                };
                ui.label(
                    egui::RichText::new(said)
                        .font(egui::FontId::monospace(10.0))
                        .color(match count {
                            0 => app::caption(ui.visuals()),
                            _ => app::warn(ui.visuals()),
                        }),
                );
            });
            drawn
        })
        .inner;

    for (key, reach) in painted_keys {
        let units = units(reach);
        match units == facts.tune(key) {
            true => plan.fine_tune.remove(&key),
            false => plan.fine_tune.insert(key, units),
        };
    }

    ui.add_space(8.0);
    let chevron = match view.key_table {
        true => Glyph::ChevronDown,
        false => Glyph::ChevronRight,
    };
    let label = match view.key_table {
        true => "Hide the per-key table",
        false => "Show the 128-key fine tune table",
    };
    if ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            icon(ui, chevron, 12.0, app::caption(ui.visuals()));
            ui.label(
                egui::RichText::new(label)
                    .size(11.0)
                    .color(app::caption(ui.visuals())),
            )
        })
        .response
        .interact(egui::Sense::click())
        .clicked()
    {
        view.key_table = !view.key_table;
    }
    if !view.key_table {
        return;
    }
    egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(egui::Stroke::new(
            1.0_f32,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("piano_key_table")
                .num_columns(8)
                .spacing(egui::vec2(18.0, 0.0))
                .show(ui, |ui| {
                    for key in 0..npno::NOTES as u8 {
                        let units = plan.tune(facts, key);
                        let cents = f32::from(units) * FINE_TUNE_CENTS_PER_UNIT;
                        ui.label(
                            egui::RichText::new(note::name(key))
                                .font(egui::FontId::monospace(MONO))
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.label(
                            egui::RichText::new(format!("{cents:+.1} c"))
                                .font(egui::FontId::monospace(MONO))
                                .color(match units == 0 {
                                    true => app::caption(ui.visuals()),
                                    false => ui.visuals().text_color(),
                                }),
                        );
                        if key % 4 == 3 {
                            ui.end_row();
                        }
                    }
                });
        });
}

// ---- the faces ----------------------------------------------------------------------

impl State {
    /// The Edit face under the key map: what is kept, which layers, which roots, and
    /// the per-key tune.
    pub fn ui(&mut self, ui: &mut egui::Ui, sounding: Option<u8>) -> Option<Ask> {
        let State {
            open,
            draft,
            view,
            free,
            ..
        } = self;
        let facts = match &open.as_ref()?.facts {
            Ok(facts) => facts,
            Err(why) => {
                ui.label(egui::RichText::new(why).color(app::bad(ui.visuals())));
                return None;
            }
        };
        let free = *free;

        let kept = kept_bytes(facts, draft);
        let (badge, ink) = match free {
            Some(free) if kept > free => (
                format!("{} over", room::measure(kept - free)),
                app::warn(ui.visuals()),
            ),
            Some(free) => (
                format!("fits · {} to spare", room::measure(free - kept)),
                app::good(ui.visuals()),
            ),
            None => (
                "no free piano memory reported".to_string(),
                app::caption(ui.visuals()),
            ),
        };
        controls::heading(
            ui,
            "Trim to fit",
            "what is kept, against what the instrument has free",
            Some((&badge, ink)),
        );
        let width = ui.available_width();
        let wide = width >= 640.0;
        let wide_column = match wide {
            true => (width - PAD * 2.0 - 24.0) / 2.0,
            false => width - PAD * 2.0,
        };
        match wide {
            true => {
                ui.horizontal_top(|ui| {
                    ui.add_space(PAD);
                    column(ui, wide_column, |ui| switches(ui, facts, draft));
                    ui.add_space(24.0);
                    column(ui, wide_column, |ui| meter(ui, facts, draft, free));
                });
            }
            false => {
                ui.horizontal_top(|ui| {
                    ui.add_space(PAD);
                    column(ui, wide_column, |ui| {
                        switches(ui, facts, draft);
                        ui.add_space(10.0);
                        meter(ui, facts, draft, free);
                    });
                });
            }
        }
        ui.add_space(12.0);

        controls::heading(
            ui,
            "Playback",
            "what the instrument applies over every stroke",
            None,
        );
        ui.horizontal_top(|ui| {
            ui.add_space(PAD);
            column(ui, width - PAD * 2.0, |ui| playback(ui, facts, draft));
        });
        ui.add_space(12.0);

        controls::heading(
            ui,
            "Velocity layers",
            "the switch speaks for every root; a segment is one root — click it to drop \
             that layer there only",
            None,
        );
        ui.horizontal_top(|ui| {
            ui.add_space(PAD);
            column(ui, width - PAD * 2.0, |ui| {
                lanes(ui, facts, draft, view.picked)
            });
        });
        ui.add_space(12.0);

        controls::heading(
            ui,
            "Roots",
            "each root keeps or drops its own layers",
            Some((
                &format!("{} roots", facts.roots.len()),
                app::caption(ui.visuals()),
            )),
        );
        let ask = roots(ui, facts, draft, view, sounding);
        ui.add_space(12.0);

        controls::heading(
            ui,
            "Per key",
            "drag across a lane to draw values; the table below edits one key at a time",
            None,
        );
        ui.horizontal_top(|ui| {
            ui.add_space(PAD);
            column(ui, width - PAD * 2.0, |ui| per_key(ui, facts, draft, view));
        });
        ui.add_space(12.0);
        ask
    }

    /// What the file says about itself, above the container record every document has.
    pub fn meta(&mut self, ui: &mut egui::Ui) {
        let Some(facts) = self.facts() else {
            return;
        };
        controls::heading(
            ui,
            "Metadata",
            "what the file says about itself — read here, never written differently",
            None,
        );
        let rows = [
            ("Format", "npno".to_string(), "a piano library"),
            (
                "Stream version",
                format!("{:#05x}", facts.stream),
                "what decides the capability struct",
            ),
            ("Size", room::measure(facts.total), "before any trim"),
            (
                "Name",
                format!("{}#{}", facts.name, facts.variant),
                "Name#Variant at 0x1c",
            ),
            ("Channels", facts.channel_word().to_string(), "per file"),
            (
                "Strokes",
                format!("{} in {} roots", facts.strokes, facts.roots.len()),
                "one recorded note each",
            ),
            (
                "Keys answered",
                format!("{} of 128", facts.covered.len()),
                "the key map at 0x8c",
            ),
            ("Kind", kind_name(facts.kind), "0x18"),
            (
                "Gain",
                format!("{:+.1} dB", f32::from(facts.gain) / 10.0),
                "0x40c, tenths of a decibel",
            ),
        ];
        for (label, value, note) in rows {
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
            ui.painter().hline(
                rect.x_range(),
                rect.top() + 0.5,
                egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
            );
            let painter = ui.painter().clone();
            let inner = rect.shrink2(egui::vec2(PAD, 0.0));
            let track = inner.width() / 3.8;
            cell(
                &painter,
                inner.left(),
                rect.center().y,
                track,
                label,
                egui::FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
            cell(
                &painter,
                inner.left() + track,
                rect.center().y,
                track,
                &value,
                egui::FontId::monospace(11.0),
                ui.visuals().text_color(),
            );
            cell(
                &painter,
                inner.left() + track * 2.0,
                rect.center().y,
                inner.right() - inner.left() - track * 2.0,
                note,
                egui::FontId::proportional(NOTE),
                app::caption(ui.visuals()),
            );
        }
        ui.add_space(10.0);
        ui.horizontal_top(|ui| {
            ui.add_space(PAD);
            ui.spacing_mut().item_spacing.x = 7.0;
            icon(ui, Glyph::CircleCheck, 12.0, app::good(ui.visuals()));
            ui.label(
                egui::RichText::new(
                    "Byte-exact: the stroke directory and audio stay verbatim; a trim \
                     re-lays the directory and every audio offset, and the container \
                     recomputes its checksum.",
                )
                .size(11.0)
                .color(ui.visuals().weak_text_color()),
            );
        });
        ui.add_space(10.0);
    }

    /// The Advanced face: what this format holds, and where the edited fields land.
    pub fn advanced(&mut self, ui: &mut egui::Ui) {
        if self.facts().is_none() {
            return;
        }
        capability::table(ui, CAPABILITIES);
        capability::offsets(ui, &offsets());
    }
}

/// What a piano library holds, in the state this editor puts each of them in.
const CAPABILITIES: &[Row] = &[
    Row {
        name: "name",
        state: Cap::Editable,
        note: "Name#Variant at 0x1c",
    },
    Row {
        name: "category / sub",
        state: Cap::Absent,
        note: "no category in a piano library",
    },
    Row {
        name: "key zones: root / top / low",
        state: Cap::Editable,
        note: "a root per key at 0x8c; the boundaries are dragged on the key map",
    },
    Row {
        name: "velocity layers",
        state: Cap::Editable,
        note: "per root; the index is the file's",
    },
    Row {
        name: "per-zone gain / detune",
        state: Cap::Editable,
        note: "trim per stroke at +0x34 (1 dB a unit); fine tune per key at 0x18c",
    },
    Row {
        name: "per-key table",
        state: Cap::Editable,
        note: "128 × fine tune at 0x18c",
    },
    Row {
        name: "instrument gain",
        state: Cap::Editable,
        note: "0x40c, signed tenths of a dB",
    },
    Row {
        name: "loop points / crossfade",
        state: Cap::ReadOnly,
        note: "marks in the stroke record, carried verbatim; meaning open",
    },
    Row {
        name: "loop decay / detune",
        state: Cap::ReadOnly,
        note: "the applied-decay ladder at +0x36, carried as read",
    },
    Row {
        name: "release samples",
        state: Cap::Editable,
        note: "bank 2 — drop or keep",
    },
    Row {
        name: "pedal resonance samples",
        state: Cap::Editable,
        note: "bank 1 — Small out of Medium",
    },
    Row {
        name: "sound parameters",
        state: Cap::Absent,
        note: "panel-side: acoustics, touch",
    },
    Row {
        name: "stereo / channels",
        state: Cap::ReadOnly,
        note: "per file",
    },
    Row {
        name: "replace / add a stroke",
        state: Cap::Editable,
        note: "File › New › piano library builds one from WAVs; a template is optional",
    },
    Row {
        name: "cut / move / drop strokes",
        state: Cap::Editable,
        note: "strokes are self-contained",
    },
    Row {
        name: "decode / audition",
        state: Cap::Editable,
        note: "one stroke at a time, on request",
    },
    Row {
        name: "size trim",
        state: Cap::Editable,
        note: "banks, layers, range",
    },
    Row {
        name: "write to the instrument",
        state: Cap::Editable,
        note: "class 1",
    },
    Row {
        name: "byte-exact round trip",
        state: Cap::Verified,
        note: "directory and audio re-laid, checksum recomputed",
    },
];

/// Where the fields this editor writes land in the body.
fn offsets() -> Vec<Offset> {
    vec![
        Offset {
            at: "body 0x04".to_string(),
            holds: "u16".to_string(),
            note: "stream version — the offsets below are pinned to it",
        },
        Offset {
            at: "body 0x1c".to_string(),
            holds: "Name#Variant".to_string(),
            note: "32 bytes, NUL-padded; patched on rename",
        },
        Offset {
            at: "body 0x8c".to_string(),
            holds: "128 entries".to_string(),
            note: "the root that plays each key; 0xFF where the library covers nothing",
        },
        Offset {
            at: "body 0x18c".to_string(),
            holds: "128 × i8".to_string(),
            note: "fine tune, 0.7 cents a unit",
        },
        Offset {
            at: "body 0x18".to_string(),
            holds: "u8".to_string(),
            note: "the kind of instrument the library is filed under",
        },
        Offset {
            at: "body 0x40c".to_string(),
            holds: "i8".to_string(),
            note: "a gain over the whole library, in tenths of a decibel",
        },
        Offset {
            at: "body 0x40d".to_string(),
            holds: "u8".to_string(),
            note: "the highest key damped at note-off; keys above it ring on",
        },
        Offset {
            at: "record +0x34".to_string(),
            holds: "u16".to_string(),
            note: "trim per stroke in the directory at 0x732, 118 bytes a record",
        },
    ]
}

#[cfg(test)]
mod tests {
    use nord_format::formats::npno::synthetic::{take, Build};
    use nord_usb::wire::Status;

    use super::*;
    use crate::device::{pretend_allocation_unit, Device};
    use crate::log::Log;
    use crate::workspace::{Origin, Workspace};

    /// The roots the test library records, and the layer values it spreads them over.
    const ROOTS: [u8; 3] = [48, 60, 72];
    const LAYERS: [u8; 3] = [0, 6, 12];

    /// A library shaped like a small vendor one: three roots of three attack layers,
    /// one of them also carrying a pedal-resonance and a release stroke, and a map that
    /// answers every key from A0 to C8.
    ///
    /// The blocks per stroke differ, so every figure the sections print is a figure
    /// something could get wrong.
    fn built() -> Build {
        let mut takes = Vec::new();
        for (index, root) in ROOTS.into_iter().enumerate() {
            for (rank, layer) in LAYERS.into_iter().enumerate() {
                takes.push(take(
                    root,
                    Bank::Attack,
                    layer,
                    (8 - rank * 2 + index) as u16,
                ));
            }
            if root == 60 {
                takes.push(take(root, Bank::Resonance, 0, 5));
                takes.push(take(root, Bank::Release, 0, 3));
            }
        }
        let map = (SPAN.low..=SPAN.high)
            .map(|key| {
                let root = ROOTS
                    .into_iter()
                    .min_by_key(|root| root.abs_diff(key))
                    .expect("three roots");
                (key, root)
            })
            .collect();
        Build {
            version: 0x464,
            channels: 1,
            takes,
            map,
        }
    }

    fn bytes() -> Vec<u8> {
        built().bytes().expect("the builder lays out a library")
    }

    fn facts() -> Facts {
        Facts::of(&bytes()).expect("it reads")
    }

    /// The test library with every switch still on.
    fn plan() -> Plan {
        Plan::default()
    }

    // ---- the plan -------------------------------------------------------------------

    #[test]
    fn a_plan_that_drops_nothing_rebuilds_the_bytes_it_was_saved_as() {
        let saved = bytes();
        assert_eq!(rebuild(&saved, &plan()).unwrap(), saved);
    }

    /// The release bank is its own set of strokes: dropping it takes exactly those and
    /// leaves every key still answering the root it did.
    #[test]
    fn dropping_the_release_bank_takes_its_strokes_and_uncovers_no_key() {
        let saved = bytes();
        let facts = facts();
        let mut plan = plan();
        plan.switch_bank(Bank::Release, false);

        let made = rebuild(&saved, &plan).unwrap();
        let entity = nord_format::from_stream(&mut Cursor::new(&made)).unwrap();
        let library = piano(&entity).unwrap().library().unwrap();
        assert_eq!(library.strokes().len(), facts.strokes - 1);
        assert!(
            library
                .strokes()
                .iter()
                .all(|stroke| stroke.bank() != Some(Bank::Release)),
            "a release stroke survived the drop"
        );
        let was = nord_format::from_stream(&mut Cursor::new(&saved)).unwrap();
        assert_eq!(
            library.key_map(),
            piano(&was).unwrap().library().unwrap().key_map(),
            "the release bank answers no key of its own"
        );
        assert_eq!(
            kept_bytes(&facts, &plan),
            facts.total - bank_bytes(&facts, None, Bank::Release)
        );
    }

    /// A per-root exception is the one selection no named transform expresses: it takes
    /// one layer off one root and leaves every other stroke where it was.
    #[test]
    fn a_per_root_layer_drop_leaves_every_other_stroke_where_it_was() {
        let saved = bytes();
        let before = nord_format::from_stream(&mut Cursor::new(&saved)).unwrap();
        let before = piano(&before).unwrap().library().unwrap();

        let mut plan = plan();
        plan.roots.insert((60, LAYERS[1]), false);
        let made = rebuild(&saved, &plan).unwrap();
        let after = nord_format::from_stream(&mut Cursor::new(&made)).unwrap();
        let after = piano(&after).unwrap().library().unwrap();

        assert_eq!(after.strokes().len(), before.strokes().len() - 1);
        let kept: Vec<(u8, u8, u8)> = before
            .strokes()
            .iter()
            .filter(|stroke| !(stroke.root == 60 && stroke.layer() == LAYERS[1]))
            .map(|stroke| (stroke.root, stroke.bank_code(), stroke.layer()))
            .collect();
        let left: Vec<(u8, u8, u8)> = after
            .strokes()
            .iter()
            .map(|stroke| (stroke.root, stroke.bank_code(), stroke.layer()))
            .collect();
        assert_eq!(left, kept);
        for (was, is) in before
            .strokes()
            .iter()
            .filter(|stroke| !(stroke.root == 60 && stroke.layer() == LAYERS[1]))
            .zip(after.strokes())
        {
            assert_eq!(
                was.audio(),
                is.audio(),
                "a span moved with its bytes changed"
            );
        }
        assert_eq!(
            after.key_root(60).unwrap(),
            Some(60),
            "root 60 still has layers, so its keys still answer"
        );
    }

    /// The name is a field in the prefix: writing it moves the bytes it owns and the
    /// container's checksum, and nothing else.
    #[test]
    fn renaming_a_library_changes_the_name_bytes_and_nothing_else() {
        let saved = bytes();
        let mut plan = plan();
        plan.name = Some("Wurly 200A".to_string());
        let made = rebuild(&saved, &plan).unwrap();

        assert_eq!(made.len(), saved.len(), "a rename moves no audio");
        let moved: Vec<usize> = (0..saved.len())
            .filter(|at| saved[*at] != made[*at])
            .collect();
        assert!(!moved.is_empty(), "the name did not land");
        let entity = nord_format::from_stream(&mut Cursor::new(&made)).unwrap();
        let library = piano(&entity).unwrap().library().unwrap();
        assert_eq!(library.name(), ("Wurly 200A".into(), "Variant".into()));

        let body_at = saved
            .windows(4)
            .position(|word| word == b"CNSP")
            .expect("a CNSP body");
        let body_len = library.body_len().unwrap();
        for at in &moved {
            // `Name#Variant` at body 0x1c, and on this stream the long name at 0x3c.
            // Anything outside the body is the container's own checksum.
            let inside = at.checked_sub(body_at).filter(|at| *at < body_len);
            if let Some(at) = inside {
                assert!(
                    (0x1c..0x5c).contains(&at),
                    "body byte {at:#x} is outside the name fields",
                );
            }
        }
    }

    /// The switches speak for every root, so throwing one has to clear the exceptions
    /// that root made — a switch that left them standing would say one thing and do
    /// another.
    #[test]
    fn a_master_switch_clears_the_per_root_exceptions_of_its_layer() {
        let mut plan = plan();
        plan.roots.insert((60, LAYERS[0]), false);
        plan.roots.insert((60, LAYERS[1]), false);
        assert!(!plan.keeps_layer(60, LAYERS[0]));

        plan.switch_layer(LAYERS[0], false);
        assert!(
            !plan.keeps_layer(48, LAYERS[0]),
            "the switch speaks for every root"
        );
        plan.switch_layer(LAYERS[0], true);
        assert!(
            plan.keeps_layer(60, LAYERS[0]),
            "and putting it back clears the exception it spoke over"
        );
        assert!(
            !plan.keeps_layer(60, LAYERS[1]),
            "another layer's exception is not this switch's business"
        );
    }

    /// A plan that would leave the library with nothing to play is refused, and the
    /// rebuild says so rather than writing an empty directory.
    #[test]
    fn a_plan_that_would_leave_no_strokes_at_all_is_refused() {
        let saved = bytes();
        let mut plan = plan();
        for layer in LAYERS {
            plan.switch_layer(layer, false);
        }
        let refused = rebuild(&saved, &plan).unwrap_err();
        assert!(refused.contains("no strokes at all"), "{refused}");
    }

    #[test]
    fn a_body_that_is_no_piano_library_is_refused_before_anything_is_written() {
        let mut plan = plan();
        plan.name = Some("Wurly 200A".to_string());
        let refused = rebuild(&crate::fields::blank::electro5_song(), &plan);
        assert!(refused.is_err(), "a set list is not a piano library");
    }

    // ---- the playback fields, the trims and the key map -----------------------------

    // Where the piano document writes, as the Advanced face states the offsets.
    const KIND_AT: usize = 0x18;
    const KEY_MAP_AT: usize = 0x8c;
    const GAIN_AT: usize = 0x40c;
    const DAMPER_TOP_AT: usize = 0x40d;
    const DIRECTORY_AT: usize = 0x732;
    const REC_TRIM: usize = 0x34;

    /// The body offsets a plan moves. Anything outside the body is the container's own
    /// checksum, which every write moves.
    fn moved(saved: &[u8], plan: &Plan) -> Vec<usize> {
        let base = rebuild(saved, &Plan::default()).unwrap();
        let made = rebuild(saved, plan).unwrap();
        assert_eq!(made.len(), base.len(), "the plan re-laid the audio");
        let body_at = saved
            .windows(4)
            .position(|word| word == b"CNSP")
            .expect("a CNSP body");
        let entity = nord_format::from_stream(&mut Cursor::new(&base)).unwrap();
        let body_len = piano(&entity)
            .unwrap()
            .library()
            .unwrap()
            .body_len()
            .unwrap();
        (0..base.len())
            .filter(|at| base[*at] != made[*at])
            .filter_map(|at| at.checked_sub(body_at).filter(|at| *at < body_len))
            .collect()
    }

    #[test]
    fn each_playback_field_writes_only_the_byte_it_names() {
        let saved = bytes();

        let mut gained = plan();
        gained.gain = Some(-20);
        assert_eq!(moved(&saved, &gained), [GAIN_AT]);

        let mut damped = plan();
        damped.damper_top = Some(90);
        assert_eq!(moved(&saved, &damped), [DAMPER_TOP_AT]);

        let mut filed = plan();
        filed.kind = Some(Kind::Wurlitzer);
        assert_eq!(moved(&saved, &filed), [KIND_AT]);

        let mut routed = plan();
        routed.key_roots.insert(60, Some(ROOTS[0]));
        assert_eq!(moved(&saved, &routed), [KEY_MAP_AT + 60]);

        // The trim is a u16 in the record of the stroke it names — the first of them,
        // which is the lowest root's loudest attack.
        let mut trimmed = plan();
        trimmed
            .trims
            .insert((ROOTS[0], Bank::Attack.code(), LAYERS[0]), 6);
        assert_eq!(moved(&saved, &trimmed), [DIRECTORY_AT + REC_TRIM + 1]);
    }

    /// A trim names the stroke it belongs to, not the place in the directory that
    /// stroke sat at: dropping a layer moves every record after it, and the trim has to
    /// move with its own.
    #[test]
    fn a_trim_lands_on_its_own_stroke_after_another_layer_is_dropped() {
        let saved = bytes();
        let mut plan = plan();
        let wanted = (ROOTS[1], Bank::Attack.code(), LAYERS[2]);
        plan.trims.insert(wanted, 9);
        plan.switch_layer(LAYERS[0], false);

        let made = rebuild(&saved, &plan).unwrap();
        let entity = nord_format::from_stream(&mut Cursor::new(&made)).unwrap();
        let library = piano(&entity).unwrap().library().unwrap();
        for stroke in library.strokes() {
            let held = (stroke.root, stroke.bank_code(), stroke.layer());
            assert_eq!(
                stroke.trim(),
                match held == wanted {
                    true => 9,
                    false => 0,
                },
                "{stroke:?}"
            );
        }
    }

    /// The map is written before anything is dropped, so a route the library cannot
    /// play is the format's own refusal rather than a silently uncovered key.
    #[test]
    fn a_route_to_a_root_the_directory_does_not_record_is_refused_in_the_formats_words() {
        let saved = bytes();
        let mut plan = plan();
        plan.key_roots.insert(60, Some(100));
        let refused = rebuild(&saved, &plan).unwrap_err();
        assert!(refused.contains("root 100"), "{refused}");

        // A route to a root a later drop takes away is not refused: the drop uncovers
        // the key, which is what dropping a root has always done.
        let mut dropped = plan.clone();
        dropped.key_roots.insert(60, Some(ROOTS[0]));
        for layer in LAYERS {
            dropped.roots.insert((ROOTS[0], layer), false);
        }
        let made = rebuild(&saved, &dropped).unwrap();
        let entity = nord_format::from_stream(&mut Cursor::new(&made)).unwrap();
        let library = piano(&entity).unwrap().library().unwrap();
        assert_eq!(library.key_root(60).unwrap(), None);
    }

    /// Dragging the boundary between two roots moves the keys across it, and the map,
    /// the coverage and what a struck key sounds all read the plan rather than the file.
    #[test]
    fn a_dragged_root_boundary_moves_the_keys_across_it() {
        let facts = facts();
        let spread = |plan: &Plan| -> Vec<(u8, u8)> {
            (0..facts.roots.len())
                .flat_map(|index| runs(&plan.keys_of(&facts, index)))
                .collect()
        };
        let was = spread(&plan());
        assert_eq!(was, [(SPAN.low, 54), (55, 66), (67, SPAN.high)]);

        let mut moved = plan();
        let now = keys::boundary(&was, SPAN, 0, keys::Edge::Top, 57);
        reroute(&facts, &mut moved, &[0, 1, 2], &was, &now);
        assert_eq!(spread(&moved), [(SPAN.low, 57), (58, 66), (67, SPAN.high)]);
        assert!(
            status(&facts, &moved, 56).1.contains("root C3"),
            "the keys moved with the boundary"
        );
        assert!(silent(&facts, &moved).is_empty(), "and none fell silent");
        assert_eq!(
            rebuild(&bytes(), &moved).unwrap().len(),
            bytes().len(),
            "a remap moves no audio"
        );

        // The outer end has no root to share with, so it uncovers keys instead.
        let mut cut = plan();
        let now = keys::boundary(&was, SPAN, 2, keys::Edge::Top, 100);
        reroute(&facts, &mut cut, &[0, 1, 2], &was, &now);
        assert_eq!(silent(&facts, &cut), [(101, SPAN.high)]);
        assert!(!status(&facts, &cut, 105).0);
    }

    /// A key sounds the largest layer value its root holds that is at most
    /// `(127 − v)·31/127`, so each value answers from the split with the value below it
    /// up to its own.
    #[test]
    fn a_layer_value_answers_the_velocities_no_softer_value_reaches() {
        assert_eq!(spans(&[0]), [(0, 1..=127)]);
        assert_eq!(
            spans(&[0, 6, 12]),
            [(0, 103..=127), (6, 78..=102), (12, 1..=77)]
        );

        // 31 is past the bound at every velocity a key can send, so nothing reaches it.
        let reach = spans(&[0, 31]);
        assert_eq!(reach[0], (0, 1..=127));
        assert!(reach[1].1.is_empty(), "{:?}", reach[1]);
        assert_eq!(span_text(&reach[1].1), "no velocity");
        assert_eq!(span_text(&reach[0].1), "v 1–127");
    }

    /// The ladder holds a per-frame coefficient, and the stroke's line reads it as the
    /// rate it decays at. Unity applies nothing at all.
    #[test]
    fn an_applied_decay_reads_as_the_rate_its_ladder_states() {
        let unity = [npno::LADDER_UNITY; npno::DECAYS];
        assert_eq!(decay_rate(&unity), None);

        let mut ladder = unity;
        ladder[0] = npno::LADDER_UNITY - 1000;
        let rate = decay_rate(&ladder).expect("an entry below unity applies something");
        // 8.686 × 35002 × 1000/2^23 = 36.2 dB a second.
        assert!((rate - 36.2).abs() < 0.1, "{rate}");
    }

    // ---- the arithmetic -------------------------------------------------------------

    /// Every figure the trim section prints comes off the strokes' own byte lengths,
    /// and the file is the scale: a plan that drops nothing keeps all of it.
    #[test]
    fn kept_bytes_is_the_file_less_the_audio_the_plan_drops() {
        let facts = facts();
        let mut plan = plan();
        assert_eq!(kept_bytes(&facts, &plan), facts.total);

        let softest = *LAYERS.last().unwrap();
        let layer = layer_bytes(&facts, None, softest);
        assert!(layer > 0);
        plan.switch_layer(softest, false);
        assert_eq!(kept_bytes(&facts, &plan), facts.total - layer);

        // And the rebuilt file is no larger than what the arithmetic promised: the
        // directory loses a record per stroke as well.
        let made = rebuild(&bytes(), &plan).unwrap();
        assert!(
            made.len() as u64 <= kept_bytes(&facts, &plan),
            "{} against {}",
            made.len(),
            kept_bytes(&facts, &plan)
        );
    }

    /// A range cut drops whole roots, and a root keeping one key keeps every stroke it
    /// has.
    #[test]
    fn a_range_cut_costs_the_roots_it_leaves_no_key_to_answer() {
        let facts = facts();
        let mut plan = plan();
        plan.range = Some(55..=108);
        assert!(!plan.in_range(&facts, 0), "root 48 answers nothing now");
        assert!(plan.in_range(&facts, 1));
        assert_eq!(
            kept_bytes(&facts, &plan),
            facts.total - root_bytes(&facts, None, 0)
        );
        assert!(covered(&facts, &plan).iter().all(|key| *key >= 55));
    }

    /// The sentence names the smallest set of switches still on that clears the
    /// overage, and never one that is already off.
    #[test]
    fn the_cheapest_cut_is_the_smallest_set_of_switches_that_clears_the_overage() {
        let facts = facts();
        let plan = plan();
        let release = bank_bytes(&facts, None, Bank::Release);
        let resonance = bank_bytes(&facts, None, Bank::Resonance);
        assert!(release < resonance, "the release bank is the cheaper cut");

        // An overage the release bank alone covers is the release bank alone.
        let cut = cheapest_cut(&facts, &plan, release - 1).expect("something fits");
        assert_eq!(cut.picked, ["release samples"]);
        assert_eq!(cut.shed, release);

        // One it does not: the next cheapest single switch, not the release plus a
        // layer.
        let cut = cheapest_cut(&facts, &plan, release + 1).expect("something fits");
        assert_eq!(cut.picked.len(), 1, "{:?}", cut.picked);
        assert!(cut.shed > release);

        // Nothing left to throw once every switch is off.
        let mut spent = plan.clone();
        for layer in LAYERS {
            spent.switch_layer(layer, false);
        }
        for bank in [Bank::Resonance, Bank::Release] {
            spent.switch_bank(bank, false);
        }
        assert!(cheapest_cut(&facts, &spent, 1).is_none());

        // And a cut that would leave nothing at all to play is not a cut.
        let audio: u64 = facts.cells.iter().map(|cell| cell.bytes).sum();
        assert!(cheapest_cut(&facts, &plan, audio).is_none());
    }

    #[test]
    fn a_list_of_switches_reads_as_a_sentence_names_them() {
        let one = vec!["release samples".to_string()];
        assert_eq!(listed(&one), "release samples");
        let two = vec!["soft layer".to_string(), "release samples".to_string()];
        assert_eq!(listed(&two), "soft layer and release samples");
        let three = vec![
            "soft layer".to_string(),
            "medium layer".to_string(),
            "release samples".to_string(),
        ];
        assert_eq!(
            listed(&three),
            "soft layer, medium layer and release samples"
        );
        assert_eq!(listed(&[]), "");
    }

    /// Three layers are the panel's own three words; any other count is named by its
    /// place, loudest first.
    #[test]
    fn a_layer_is_named_by_its_place_among_the_layers_the_file_holds() {
        let three: Vec<String> = (0..3).map(|rank| layer_name(rank, 3)).collect();
        assert_eq!(three, ["Hard", "Medium", "Soft"]);
        let five: Vec<String> = (0..5).map(|rank| layer_name(rank, 5)).collect();
        assert_eq!(
            five,
            ["Loudest", "Layer 2", "Layer 3", "Layer 4", "Softest"]
        );
        assert_eq!(layer_name(0, 1), "Loudest");
        // The rows and the lamps read softest first, which is the order a trim spends
        // them in.
        let facts = facts();
        assert_eq!(
            facts.shown_layers(),
            [(2, LAYERS[2]), (1, LAYERS[1]), (0, LAYERS[0])]
        );
        assert_eq!(layer_short(2, 3), "S");
    }

    // ---- the fine tune lane ---------------------------------------------------------

    /// The file stores units and the lane shows cents, so the conversion has to come
    /// back to the unit it started at — including at the ends of an `i8`.
    #[test]
    fn a_fine_tune_reads_in_cents_and_writes_back_the_unit_it_came_from() {
        for held in [0i8, 1, -1, 27, -27, 35, -35] {
            assert_eq!(units(reach(held)), held, "{held} units");
        }
        // The lane reaches ±25 cents, which is ±35 units at 0.7 cents each; a value
        // drawn past that lands on the unit the lane's own end is worth.
        assert_eq!(units(1.0), 36);
        assert_eq!(units(-1.0), -36);
        // And nothing wraps: an `i8` is what the byte holds.
        assert_eq!(units(100.0), i8::MAX);
        assert_eq!(units(-100.0), i8::MIN);
    }

    #[test]
    fn a_retuned_key_lands_on_the_key_it_was_drawn_on() {
        let saved = bytes();
        let facts = facts();
        let mut plan = plan();
        plan.fine_tune.insert(60, -4);
        assert_eq!(plan.tune(&facts, 60), -4);
        assert_eq!(plan.tune(&facts, 61), 0, "and no neighbour moved");

        let made = rebuild(&saved, &plan).unwrap();
        let entity = nord_format::from_stream(&mut Cursor::new(&made)).unwrap();
        let library = piano(&entity).unwrap().library().unwrap();
        assert_eq!(library.fine_tune(60).unwrap(), -4);
        assert_eq!(library.fine_tune(61).unwrap(), 0);
    }

    // ---- the header's extras --------------------------------------------------------

    /// A partition with about `free` bytes left for pianos, and nothing else attached.
    ///
    /// The pretend allocation unit is small, so a test can name a free size the
    /// partition's own counters can actually express.
    fn attached(free: u64) -> Device {
        const UNIT: u32 = 1024;
        let mut device = Device::new(egui::Context::default());
        device.pretend_partitions(&[(ObjectClass::Piano, "Piano", UNIT)]);
        let unit = pretend_allocation_unit(ObjectClass::Piano, UNIT);
        device.state.inventory.push(Status {
            class: ObjectClass::Piano,
            count: 1,
            free: u32::try_from(free / u64::from(unit.get())).expect("a small pretend"),
            used: 0,
            dirty: 0,
            spare: 0,
        });
        device
    }

    /// The header reads what is kept of what is held, says `trimmed` rather than
    /// `edited`, and refuses to queue a library the instrument has no room for.
    #[test]
    fn the_header_reads_what_is_kept_and_says_when_it_will_not_fit() {
        let facts = facts();
        let plenty = attached(facts.total * 2);
        let untouched = extras(
            &facts,
            &plan(),
            room::free_bytes(ObjectClass::Piano, &plenty.state),
        );
        assert!(
            untouched.size.is_none(),
            "nothing is trimmed, so nothing is said"
        );
        assert!(untouched.edited.is_none());
        assert!(untouched.loud.is_none(), "the header's own rule runs");

        let mut plan = plan();
        plan.switch_bank(Bank::Release, false);
        let kept = kept_bytes(&facts, &plan);
        let trimmed = extras(
            &facts,
            &plan,
            room::free_bytes(ObjectClass::Piano, &plenty.state),
        );
        assert_eq!(
            trimmed.size.as_ref().map(|size| size.text.clone()),
            Some(room::measure_out_of(kept, facts.total))
        );
        assert_eq!(
            trimmed.edited.as_ref().map(|line| line.words.as_str()),
            Some("trimmed")
        );
        assert!(!trimmed.size.unwrap().warn, "it fits");
        assert!(trimmed.loud.is_none());

        // A partition with room for half of it: the loud action carries the overage.
        let cramped = attached(kept / 2);
        let free = room::free_bytes(ObjectClass::Piano, &cramped.state).expect("a unit arrived");
        let held = extras(&facts, &plan, Some(free));
        let loud = held.loud.expect("it will not fit");
        assert_eq!(loud.tone, Tone::Blocked);
        assert_eq!(loud.send, None, "a blocked action asks for nothing");
        assert_eq!(
            loud.label,
            format!("Won't fit · {} over", room::measure(kept - free))
        );
        assert!(held.size.expect("a size line").warn);

        // And with no instrument attached there is no verdict to give.
        let alone = Device::new(egui::Context::default());
        let quiet = extras(
            &facts,
            &plan,
            room::free_bytes(ObjectClass::Piano, &alone.state),
        );
        assert!(quiet.loud.is_none(), "nothing has reported its free memory");
        assert!(quiet
            .size
            .expect("it is still trimmed")
            .hint
            .contains("not reported"));
    }

    /// The sentence under the meter says whether it fits, what to throw when it does
    /// not, and says nothing it cannot know.
    #[test]
    fn the_constraint_sentence_names_the_cut_or_says_it_cannot_tell() {
        let facts = facts();
        let plan = plan();
        let (said, loud) = constraint(&facts, &plan, Some(facts.total * 2));
        assert!(
            said.starts_with("At ") && said.contains("to spare"),
            "{said}"
        );
        assert!(!loud);

        let (said, loud) = constraint(&facts, &plan, Some(facts.total / 2));
        assert!(said.contains("over."), "{said}");
        assert!(said.contains("the cheapest cut left that fits"), "{said}");
        assert!(loud);

        let (said, _) = constraint(&facts, &plan, None);
        assert!(said.contains("has not reported"), "{said}");
    }

    // ---- the status line ------------------------------------------------------------

    /// What a struck key says: which root answered it, how far it was shifted, and why
    /// it was silent where it was.
    #[test]
    fn a_struck_key_says_which_root_answered_it() {
        let facts = facts();
        let mut plan = plan();
        let (good, said) = status(&facts, &plan, 61);
        assert!(good);
        assert_eq!(
            said,
            format!(
                "C#4 at vel {} → root C4 · shifted +1 st",
                keys::AUDITION_VELOCITY
            )
        );

        // With the loudest layer of that root dropped, the next kept one plays.
        plan.roots.insert((60, LAYERS[0]), false);
        let (good, said) = status(&facts, &plan, 60);
        assert!(good, "{said}");
        assert!(said.ends_with("the loudest layer is dropped, so the next kept layer plays"));

        // With every layer of it dropped, nothing does.
        for layer in LAYERS {
            plan.roots.insert((60, layer), false);
        }
        let (good, said) = status(&facts, &plan, 60);
        assert!(!good);
        assert!(
            said.contains("every layer of this root is dropped"),
            "{said}"
        );

        // A key the trim has cut away answers nothing at all.
        let mut cut = plan.clone();
        cut.range = Some(21..=53);
        let (good, said) = status(&facts, &cut, 72);
        assert!(!good);
        assert!(said.contains("no root answers this key"), "{said}");
    }

    #[test]
    fn the_coverage_reading_counts_the_silent_stretches() {
        let facts = facts();
        let mut plan = plan();
        assert!(silent(&facts, &plan).is_empty());
        assert_eq!(coverage(&silent(&facts, &plan)), "every key answered");

        plan.range = Some(36..=96);
        let gaps = silent(&facts, &plan);
        assert_eq!(gaps, [(21, 35), (97, 108)]);
        assert_eq!(coverage(&gaps), "2 silent ranges");
        assert_eq!(coverage(&gaps[..1]), "1 silent range");
    }

    /// A root's keys are one run in every specimen, and a map that splits them gets one
    /// cell per run rather than one cell over the keys between.
    #[test]
    fn a_roots_keys_read_as_the_stretches_they_run_in() {
        assert_eq!(runs(&[58, 59, 60, 61]), [(58, 61)]);
        assert_eq!(runs(&[58, 59, 70, 71]), [(58, 59), (70, 71)]);
        assert!(runs(&[]).is_empty());
    }

    // ---- the capability table -------------------------------------------------------

    /// Every capability the Advanced face calls editable is something this editor
    /// actually does: a plan change that lands in the rebuilt bytes, a stroke it decodes
    /// when asked, or the send its header offers.
    #[test]
    fn every_editable_capability_is_something_the_editor_does() {
        let saved = bytes();
        let facts = facts();
        for row in CAPABILITIES.iter().filter(|row| row.state == Cap::Editable) {
            let Some(edit) = plan_for(row.name) else {
                match row.name {
                    "decode / audition" => {}
                    "replace / add a stroke" => assert!(crate::newproject::Making::FROM_WAVS
                        .contains(&crate::newproject::Making::Piano)),
                    "write to the instrument" => {
                        assert!(crate::device::sendable(ObjectClass::Piano))
                    }
                    other => panic!("{other} claims editable with no plan behind it"),
                }
                continue;
            };
            let mut plan = plan();
            edit(&mut plan, &facts);
            assert_ne!(
                rebuild(&saved, &plan).unwrap(),
                saved,
                "{} changed nothing",
                row.name
            );
        }
    }

    /// The plan change one capability names, where it names one.
    fn plan_for(name: &str) -> Option<fn(&mut Plan, &Facts)> {
        match name {
            "name" => Some(|plan, _| plan.name = Some("Wurly 200A".to_string())),
            "key zones: root / top / low" => Some(|plan, facts| {
                plan.route(facts, facts.roots[1].keys[0], Some(0));
            }),
            "per-zone gain / detune" => Some(|plan, facts| {
                plan.trims.insert(
                    (facts.roots[0].note, Bank::Attack.code(), facts.layers[0]),
                    6,
                );
            }),
            "instrument gain" => Some(|plan, facts| plan.gain = Some(facts.gain - 20)),
            "velocity layers" => Some(|plan, facts| plan.switch_layer(facts.layers[2], false)),
            "per-key table" => Some(|plan, _| {
                plan.fine_tune.insert(60, -4);
            }),
            "release samples" => Some(|plan, _| plan.switch_bank(Bank::Release, false)),
            "pedal resonance samples" => Some(|plan, _| plan.switch_bank(Bank::Resonance, false)),
            "cut / move / drop strokes" => Some(|plan, facts| {
                plan.roots
                    .insert((facts.roots[0].note, facts.layers[0]), false);
            }),
            "size trim" => Some(|plan, facts| plan.range = Some(default_range(facts))),
            _ => None,
        }
    }

    // ---- the painted editor ---------------------------------------------------------

    /// The piano editor over the test library, in a headless window.
    struct Editor {
        ctx: egui::Context,
        workspace: Workspace,
        device: Device,
        log: Log,
        state: State,
        id: u64,
    }

    /// What one frame put on screen, and what it asked for.
    struct Painted {
        words: Vec<String>,
        /// The white keys of the keyboard, in ascending order.
        whites: Vec<egui::Rect>,
        /// Every lamp, in the order they were drawn: the trim switches, then one per
        /// lane, then the ones on each root's row.
        lamps: Vec<egui::Rect>,
        asked: Option<Ask>,
    }

    impl Painted {
        fn said(&self, wanted: &str) -> bool {
            self.words.iter().any(|word| word.contains(wanted))
        }

        /// The white key `note` is painted at.
        fn white(&self, note: u8) -> egui::Pos2 {
            let index = (SPAN.low..note).filter(|key| !keys::is_black(*key)).count();
            self.whites[index].center()
        }
    }

    impl Editor {
        fn new(free: u64) -> Editor {
            let ctx = egui::Context::default();
            // Dressed the way the app dresses it: without the bold face bound, laying
            // out a root's name panics mid-frame.
            ctx.set_fonts(crate::app::fonts());
            ctx.all_styles_mut(crate::app::metrics);
            let mut workspace = Workspace::new(ctx.clone());
            let mut log = Log::default();
            let id = workspace.ingest(
                "Test Piano.npno".into(),
                Origin::File("Test Piano.npno".into()),
                bytes(),
                &mut log,
            );
            Editor {
                device: attached(free),
                ctx,
                workspace,
                log,
                state: State::default(),
                id,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Painted {
            self.driven(events, |_| {})
        }

        /// One frame, with `edit` throwing whatever switch a click on its lamp would.
        fn driven(&mut self, events: Vec<egui::Event>, edit: impl FnOnce(&mut Plan)) -> Painted {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 900.0),
                )),
                ..Default::default()
            };
            let mut asked = None;
            let mut edit = Some(edit);
            let output = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let entity = self.workspace.get(self.id).expect("it is open");
                    self.state.begin(self.id, entity, &self.device.state);
                    if let Some(edit) = edit.take() {
                        edit(&mut self.state.draft);
                    }
                    asked = self.state.map(ui);
                    asked = self.state.ui(ui, None).or(asked);
                });
            });
            if let Some(plan) = self.state.drafted() {
                let saved = self.workspace.get(self.id).unwrap().saved.bytes.clone();
                match rebuild(&saved, &plan) {
                    Ok(made) => {
                        self.state.commit(plan);
                        self.workspace.replace_bytes(self.id, made, &mut self.log);
                    }
                    Err(_) => self.state.discard(),
                }
            }
            let mut painted = Painted {
                words: Vec::new(),
                whites: Vec::new(),
                lamps: Vec::new(),
                asked,
            };
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut painted);
            }
            painted.lamps.dedup();
            painted
        }
    }

    fn walk(shape: &egui::Shape, into: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => into.words.push(text.galley.text().to_string()),
            egui::Shape::Rect(drawn) if drawn.rect.height() == keys::KEYBOARD_H => {
                into.whites.push(drawn.rect)
            }
            // A lamp is the one thing drawn at its own fixed width; it is painted
            // filled and then stroked, so the pair is deduplicated after the walk.
            egui::Shape::Rect(drawn) if drawn.rect.width() == LAMP.x => into.lamps.push(drawn.rect),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
            _ => {}
        }
    }

    fn press(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// Every section paints, and the map names its roots and the keys they answer.
    #[test]
    fn the_editor_paints_its_sections_and_names_what_it_holds() {
        let mut editor = Editor::new(facts().total * 2);
        editor.frame(Vec::new());
        let painted = editor.frame(Vec::new());
        for heading in [
            "Key map",
            "Trim to fit",
            "Playback",
            "Velocity layers",
            "Roots",
            "Per key",
        ] {
            assert!(painted.said(heading), "{heading}: {:?}", painted.words);
        }
        assert!(painted.said("every key answered"));
        assert!(painted.said("3 roots"));
        assert!(painted.said("Soft layer"), "{:?}", painted.words);
        assert!(painted.said("Release samples"));
        assert!(painted.said("fits ·"));
        assert!(painted.said("INSTRUMENT GAIN"), "{:?}", painted.words);
        assert!(
            painted.said("v 78–102"),
            "a layer lane names its velocities"
        );
        assert_eq!(painted.whites.len(), 52, "a full piano's white keys");
    }

    /// Clicking a key auditions it: the root that answers it is asked for, and the line
    /// under the keyboard says what played.
    #[test]
    fn a_key_click_asks_for_the_root_that_answers_it_and_says_so() {
        let mut editor = Editor::new(facts().total * 2);
        let laid = editor.frame(Vec::new());
        let at = laid.white(62);

        let struck = editor.frame(press(at));
        assert_eq!(
            struck.asked,
            Some(Ask::Strike {
                root: 60,
                semitones: 2
            }),
            "root C4 answers D4, two semitones up"
        );

        let after = editor.frame(Vec::new());
        assert!(
            after.said("D4 at vel 90 → root C4 · shifted +2 st"),
            "{:?}",
            after.words
        );
    }

    /// Clicking a root's cell in the map opens that root's row, which is where its own
    /// facts and actions are.
    #[test]
    fn a_size_cell_click_opens_that_roots_row() {
        let mut editor = Editor::new(facts().total * 2);
        let laid = editor.frame(Vec::new());
        assert!(!laid.said("ANSWERS FROM"), "no row is open yet");

        // The size lane sits directly above the keyboard, a hair inside its own rect.
        let keyboard = laid.whites[0];
        let at = egui::pos2(laid.white(72).x, keyboard.top() - 10.0);
        editor.frame(press(at));
        let opened = editor.frame(Vec::new());
        assert!(opened.said("ANSWERS FROM"), "{:?}", opened.words);
        assert!(opened.said("Audition") && opened.said("Drop this root"));
        assert_eq!(editor.state.view.picked, Some(2), "root C5 is the third");
    }

    /// A lamp on a root's row takes that layer off that root alone, and leaves the row
    /// under it closed — the row-wide click target must not swallow the switch.
    #[test]
    fn a_lamp_on_a_roots_row_drops_that_layer_there_alone() {
        let mut editor = Editor::new(facts().total * 2);
        let laid = editor.frame(Vec::new());
        // Six trim switches, then one per lane, then three on each root's row.
        assert_eq!(laid.lamps.len(), 6 + 3 + 3 * 3, "every switch has a lamp");
        let softest_on_the_lowest_root = laid.lamps[9];

        editor.frame(press(softest_on_the_lowest_root.center()));
        let plan = &editor.state.plans[&editor.id];
        assert_eq!(plan.roots.get(&(ROOTS[0], LAYERS[2])), Some(&false));
        assert!(plan.layers.is_empty(), "every other root keeps it");
        assert!(
            editor.state.view.open_rows.is_empty(),
            "the row under the lamp did not open"
        );
    }

    /// The keyboard says where the damper stops reaching, and says nothing at all
    /// where it reaches every key.
    #[test]
    fn the_key_map_marks_the_damper_limit_only_where_a_key_rings_on() {
        let mut editor = Editor::new(facts().total * 2);
        let every = editor.driven(Vec::new(), |plan| {
            plan.damper_top = Some(ALL_KEYS_DAMPED);
        });
        assert!(!every.said("damper"), "{:?}", every.words);

        let some = editor.driven(Vec::new(), |plan| plan.damper_top = Some(90));
        assert!(some.said("damper"), "{:?}", some.words);
    }

    /// Throwing a switch trims the library: the bytes shrink, the header says
    /// `trimmed`, and throwing it back puts the strokes back byte for byte.
    #[test]
    fn a_switch_thrown_and_put_back_leaves_the_library_as_it_was() {
        let mut editor = Editor::new(facts().total * 2);
        let saved = editor.workspace.get(editor.id).unwrap().bytes.clone();

        editor.driven(Vec::new(), |plan| plan.switch_bank(Bank::Release, false));
        let trimmed = editor.workspace.get(editor.id).unwrap().bytes.clone();
        assert!(trimmed.len() < saved.len(), "the release bank is gone");
        assert!(editor.workspace.get(editor.id).unwrap().is_unsaved());

        editor.driven(Vec::new(), |plan| plan.switch_bank(Bank::Release, true));
        assert_eq!(
            editor.workspace.get(editor.id).unwrap().bytes,
            saved,
            "putting the switch back put every stroke back"
        );
    }

    /// A save is the end of the plan: what was dropped is gone, and the rows show the
    /// file as it now is.
    #[test]
    fn a_save_starts_the_plan_again_from_the_bytes_that_were_saved() {
        let mut editor = Editor::new(facts().total * 2);
        editor.driven(Vec::new(), |plan| plan.switch_bank(Bank::Release, false));
        assert!(editor.state.plans[&editor.id]
            .banks
            .contains(&Bank::Release));

        editor.workspace.mark_saved(editor.id);
        editor.frame(Vec::new());
        assert_eq!(
            editor.state.plans[&editor.id],
            Plan {
                against: editor.state.plans[&editor.id].against,
                ..Plan::default()
            },
            "the plan starts again from what is now saved",
        );
        assert!(!editor.workspace.get(editor.id).unwrap().is_unsaved());
    }
}
