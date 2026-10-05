//! Each writer's append-only log: its entries, their line encoding, and its segments.
//!
//! A writer appends only to its own directory, `writers/<writer>/` (see
//! [`crate::layout`]), and never edits or removes another writer's bytes. Each line
//! is an entry's JSON, a tab, and the CRC-32 of the JSON in eight lowercase
//! hexadecimal digits, ending in a newline. A line that fails its checksum, or a
//! final line without its newline, ends the readable segment: everything before it
//! counts, and nothing after it does.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::compact::Snapshot;
use crate::error::{Error, Result};
use crate::fs::{ensure_dir, FileKind, Fs, RelPath};
use crate::ids::{EntityId, IntentId, Version, WriterId};
use crate::layout::{Layout, LogFile};
use crate::value::{BlobId, Value};

/// One line of a writer's log.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    /// Unique across all writers; orders this entry against every other.
    pub version: Version,
    /// The intent this entry belongs to, which undo reverses as a whole.
    pub intent: IntentId,
    pub kind: Kind,
}

/// What an entry records.
///
/// In JSON the kind is the entry's `kind` member, in snake case, beside the kind's
/// own members; an absent optional member is omitted.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Kind {
    /// The first entry of every intent.
    Intent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// The intent this one undoes or redoes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reverses: Option<IntentId>,
        /// The intent changed files by journaled effects, so reversing it changes
        /// them back. An intent without it, as a bind, reverses only its entries.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        files: bool,
    },
    /// Sets the entity's last-writer-wins `exists` register to true.
    Create { entity: EntityId },
    /// Sets the entity's `exists` register to false. Its fields are kept.
    Delete { entity: EntityId },
    /// Writes a last-writer-wins field; `None` clears it.
    Field {
        entity: EntityId,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<Value>,
        /// What this writer's merged state held when it wrote, which undo restores.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prior: Option<Value>,
    },
    /// Adds `value` to an add-wins set, tagged with this entry's version.
    SetAdd {
        entity: EntityId,
        name: String,
        value: Value,
    },
    /// Removes the adds of `value` whose tags this writer had observed. An add it
    /// had not observed survives.
    SetRemove {
        entity: EntityId,
        name: String,
        value: Value,
        observed: BTreeSet<Version>,
    },
    /// A step of the intent changed the file at `path` from holding `before` to
    /// holding `after`, where `None` is no file. Undo puts `before` back.
    File {
        path: RelPath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<BlobId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<BlobId>,
    },
    /// This writer put `blob`, of `len` bytes, in the blob store.
    BlobAdded { blob: BlobId, len: u64 },
    /// This writer's garbage collection removed `blob` from the blob store.
    BlobRemoved { blob: BlobId },
    /// An entry this build does not understand, kept verbatim and ignored by merge.
    #[serde(skip)]
    Unknown {
        kind: String,
        /// The entry's JSON exactly as it was read.
        json: String,
    },
}

#[derive(Serialize)]
struct EntryJson<'a> {
    version: Version,
    intent: IntentId,
    #[serde(flatten)]
    kind: &'a Kind,
}

/// The entry's JSON: `version`, `intent`, then its kind. An unknown entry is its
/// JSON as read.
pub(crate) fn entry_json(entry: &Entry) -> String {
    if let Kind::Unknown { json, .. } = &entry.kind {
        return json.clone();
    }
    serde_json::to_string(&EntryJson {
        version: entry.version,
        intent: entry.intent,
        kind: &entry.kind,
    })
    .expect("an entry of a known kind serializes")
}

/// The entry `json` holds; `None` when it lacks a readable version, intent or kind.
/// A kind or member this build does not know gives [`Kind::Unknown`].
pub(crate) fn parse_entry(json: &str) -> Option<Entry> {
    let serde_json::Value::Object(mut members) = serde_json::from_str(json).ok()? else {
        return None;
    };
    let mut text = |name: &str| match members.remove(name) {
        Some(serde_json::Value::String(text)) => Some(text),
        _ => None,
    };
    let version = text("version")?.parse().ok()?;
    let intent = text("intent")?.parse().ok()?;
    let kind_name = members.get("kind")?.as_str()?.to_owned();
    let kind = Kind::deserialize(serde_json::Value::Object(members)).unwrap_or(Kind::Unknown {
        kind: kind_name,
        json: json.to_owned(),
    });
    Some(Entry {
        version,
        intent,
        kind,
    })
}

/// Entries as a list of their JSON texts, each exactly as a log line holds it, so an
/// entry this build does not know stays verbatim.
pub(crate) mod json_texts {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::{entry_json, parse_entry, Entry};

    pub fn serialize<S: Serializer>(entries: &[Entry], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(entries.iter().map(entry_json))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Entry>, D::Error> {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .map(|json| {
                parse_entry(json)
                    .ok_or_else(|| D::Error::custom(format!("{json:?} is not an entry")))
            })
            .collect()
    }
}

/// The line for `entry`, newline included.
pub fn encode_line(entry: &Entry) -> String {
    let json = entry_json(entry);
    let crc = crc32(json.as_bytes());
    format!("{json}\t{crc:08x}\n")
}

/// The entries of one segment's bytes, up to any torn tail.
pub fn read_segment(bytes: &[u8]) -> Segment {
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let rest = &bytes[offset..];
        let torn = Some(Torn {
            offset: offset as u64,
        });
        let Some(end) = rest.iter().position(|&byte| byte == b'\n') else {
            return Segment { entries, torn };
        };
        let Some(entry) = decode_line(&rest[..end]) else {
            return Segment { entries, torn };
        };
        entries.push(entry);
        offset += end + 1;
    }
    Segment {
        entries,
        torn: None,
    }
}

fn decode_line(line: &[u8]) -> Option<Entry> {
    let (json, crc) = std::str::from_utf8(line).ok()?.rsplit_once('\t')?;
    let well_formed = crc.len() == 8 && crc.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed || u32::from_str_radix(crc, 16).ok()? != crc32(json.as_bytes()) {
        return None;
    }
    parse_entry(json)
}

/// CRC-32/ISO-HDLC: reflected polynomial `0x04C11DB7`, initial and final XOR all ones.
fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |crc, &byte| {
        CRC_TABLE[usize::from(crc as u8 ^ byte)] ^ (crc >> 8)
    })
}

const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = match crc & 1 {
                1 => 0xedb8_8320 ^ (crc >> 1),
                _ => crc >> 1,
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Segment {
    pub entries: Vec<Entry>,
    /// Where the readable entries end early; `None` when every byte was read.
    pub torn: Option<Torn>,
}

/// A segment's readable end, before the bytes that could not be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Torn {
    /// The byte offset of the first unreadable line.
    pub offset: u64,
}

/// One writer's log as read from its directory.
#[derive(Clone, PartialEq, Debug)]
pub struct WriterLog {
    pub writer: WriterId,
    /// Every snapshot in the directory, joined.
    pub snapshot: Option<Snapshot>,
    /// The entries of the segments after the snapshot, oldest first.
    pub entries: Vec<Entry>,
    /// The numbers of the segments read, in order. Segments the snapshot folded are
    /// not read.
    pub segments: Vec<u64>,
    /// The segments that ended early, by segment number.
    pub torn: Vec<(u64, Torn)>,
}

impl WriterLog {
    pub fn new(writer: WriterId) -> Self {
        Self {
            writer,
            snapshot: None,
            entries: Vec::new(),
            segments: Vec::new(),
            torn: Vec::new(),
        }
    }

    /// The snapshot's undo window, then the segments' entries.
    pub fn all_entries(&self) -> impl Iterator<Item = &Entry> {
        let retained = self.snapshot.iter().flat_map(|s| &s.retained);
        retained.chain(&self.entries)
    }

    /// Whether the log holds an entry this build does not understand.
    pub fn has_unknown(&self) -> bool {
        self.all_entries()
            .any(|entry| matches!(entry.kind, Kind::Unknown { .. }))
    }

    /// The highest Lamport time of this log's entries, unknown ones included.
    pub fn max_lamport(&self) -> u64 {
        let folded = self.snapshot.as_ref().map_or(0, |s| s.state.max_lamport());
        self.all_entries()
            .map(|entry| entry.version.lamport)
            .fold(folded, u64::max)
    }

    /// The segment number after every segment this log has had.
    fn next_segment(&self) -> Option<u64> {
        let folded = self.snapshot.as_ref().map_or(0, |s| s.through);
        let last = self.segments.iter().copied().fold(folded, u64::max);
        last.checked_add(1)
    }
}

/// `writer`'s log; empty when the writer has no directory.
pub async fn read_log<F: Fs>(fs: &F, layout: &Layout, writer: WriterId) -> Result<WriterLog> {
    let dir = layout.writer(writer);
    let mut log = WriterLog::new(writer);
    if fs.metadata(&dir).await?.is_none() {
        return Ok(log);
    }
    let mut segments = Vec::new();
    for file in fs.list(&dir).await? {
        if file.kind != FileKind::File {
            continue;
        }
        match LogFile::parse(&file.name) {
            Some(LogFile::Segment(number)) => segments.push(number),
            Some(LogFile::Snapshot(hash)) => {
                let snapshot = read_snapshot(fs, &layout.snapshot(writer, hash), hash).await?;
                log.snapshot = Some(match log.snapshot.take() {
                    Some(earlier) => earlier.combine(snapshot),
                    None => snapshot,
                });
            }
            None => {}
        }
    }
    segments.sort_unstable();
    let through = log.snapshot.as_ref().map(|s| s.through);
    for number in segments {
        if through.is_some_and(|through| number <= through) {
            continue;
        }
        let segment = read_segment(&fs.read(&layout.segment(writer, number)).await?);
        log.entries.extend(segment.entries);
        log.segments.push(number);
        if let Some(torn) = segment.torn {
            log.torn.push((number, torn));
        }
    }
    Ok(log)
}

async fn read_snapshot<F: Fs>(fs: &F, path: &RelPath, hash: BlobId) -> Result<Snapshot> {
    let bytes = fs.read(path).await?;
    if BlobId::of(&bytes) != hash {
        return Err(Error::Corrupt {
            path: path.clone(),
            reason: "its contents do not match the hash in its name".to_owned(),
        });
    }
    Snapshot::decode(path, &bytes)
}

/// Every writer's log, sorted by writer.
pub async fn read_logs<F: Fs>(fs: &F, layout: &Layout) -> Result<Vec<WriterLog>> {
    let dir = layout.writers();
    if fs.metadata(&dir).await?.is_none() {
        return Ok(Vec::new());
    }
    let mut writers: Vec<WriterId> = fs
        .list(&dir)
        .await?
        .into_iter()
        .filter(|entry| entry.kind == FileKind::Directory)
        .filter_map(|entry| entry.name.parse().ok())
        .collect();
    writers.sort_unstable();
    let mut logs = Vec::with_capacity(writers.len());
    for writer in writers {
        logs.push(read_log(fs, layout, writer).await?);
    }
    Ok(logs)
}

/// One writer's handle for appending: its clock and counters.
///
/// Each handle writes its own segments: the first append creates a new segment after
/// every segment the writer has had, later appends extend it where the backend can
/// append, and otherwise each append creates the next one.
pub struct LogWriter {
    writer: WriterId,
    /// The highest Lamport time seen.
    lamport: u64,
    /// The next counter for each kind of id; `None` once exhausted.
    entities: Option<u64>,
    intents: Option<u64>,
    /// Why the writer's history holds what this build cannot represent.
    unreadable: Option<&'static str>,
    next_segment: Option<u64>,
    /// The segment this handle created and may append to.
    open_segment: Option<u64>,
    /// The highest version this handle has appended.
    appended: Option<Version>,
}

impl LogWriter {
    /// A handle for `writer`, given every writer's log as read at open. The clock
    /// starts past every version in `logs`, the counters past every id `writer`
    /// has allocated. The handle is read-only when `writer`'s own log has an entry
    /// this build does not understand.
    pub fn open(writer: WriterId, logs: &[WriterLog]) -> Self {
        let mut handle = Self {
            writer,
            lamport: 0,
            entities: Some(0),
            intents: Some(0),
            unreadable: None,
            next_segment: Some(1),
            open_segment: None,
            appended: None,
        };
        for log in logs {
            handle.lamport = handle.lamport.max(log.max_lamport());
            if let Some(snapshot) = &log.snapshot {
                let (entity, intent) = snapshot.state.allocated(writer);
                entity
                    .into_iter()
                    .for_each(|counter| handle.past_entity(counter));
                intent
                    .into_iter()
                    .for_each(|counter| handle.past_intent(counter));
            }
            for entry in log.all_entries() {
                handle.past_ids(entry);
            }
            if log.writer == writer {
                handle.unreadable = log
                    .has_unknown()
                    .then_some("its log holds entries this build does not understand");
                handle.next_segment = log.next_segment();
            }
        }
        handle
    }

    fn past_ids(&mut self, entry: &Entry) {
        if entry.intent.writer == self.writer {
            self.past_intent(entry.intent.counter);
        }
        for entity in entry.kind.entities() {
            if entity.writer == self.writer {
                self.past_entity(entity.counter);
            }
        }
    }

    fn past_entity(&mut self, counter: u64) {
        self.entities = self.entities.zip(counter.checked_add(1)).map(max_pair);
    }

    fn past_intent(&mut self, counter: u64) {
        self.intents = self.intents.zip(counter.checked_add(1)).map(max_pair);
    }

    pub fn writer(&self) -> WriterId {
        self.writer
    }

    /// Why this writer cannot append, if it cannot.
    pub fn read_only(&self) -> Option<&str> {
        if let Some(reason) = self.unreadable {
            return Some(reason);
        }
        let exhausted = self.lamport == u64::MAX
            || self.entities.is_none()
            || self.intents.is_none()
            || self.next_segment.is_none();
        exhausted.then_some("its clock, counters or segment numbers are exhausted")
    }

    /// Make the handle read-only because the writer's history holds what this build
    /// cannot represent.
    pub(crate) fn unreadable(&mut self, reason: &'static str) {
        self.unreadable.get_or_insert(reason);
    }

    /// Refuse with [`Error::ReadOnly`] when [`Self::read_only`] says so.
    pub fn writable(&self) -> Result<()> {
        match self.read_only() {
            Some(reason) => Err(Error::ReadOnly {
                writer: self.writer,
                reason: reason.to_owned(),
            }),
            None => Ok(()),
        }
    }

    /// Raise the clock past a version seen since open.
    pub fn observe(&mut self, version: Version) {
        self.lamport = self.lamport.max(version.lamport);
    }

    /// Move the clock and counters past an entry stamped before a crash and not yet
    /// appended.
    pub(crate) fn pass(&mut self, entry: &Entry) {
        self.observe(entry.version);
        self.past_ids(entry);
    }

    /// A new entity id. Once the counter is exhausted the handle is read-only, and
    /// the id it returns is never appended.
    pub fn new_entity(&mut self) -> EntityId {
        EntityId::new(self.writer, take(&mut self.entities))
    }

    /// A new intent id, with the same exhaustion rule as [`Self::new_entity`].
    pub fn new_intent(&mut self) -> IntentId {
        IntentId::new(self.writer, take(&mut self.intents))
    }

    /// An entry of `intent` with the next version. Once the clock is exhausted the
    /// handle is read-only, and the entry it returns is never appended.
    pub fn stamp(&mut self, intent: IntentId, kind: Kind) -> Entry {
        self.lamport = self.lamport.saturating_add(1);
        Entry {
            version: Version::new(self.lamport, self.writer),
            intent,
            kind,
        }
    }

    /// Append `entries` to this writer's log; they are durable when this returns.
    /// Refuses with [`crate::Error::ReadOnly`] when [`Self::read_only`] says so. The
    /// clock and counters move past every appended entry, so entries stamped before a
    /// crash and appended by recovery are never stamped again.
    ///
    /// After an error the entries may or may not be in the log; appending them again
    /// is harmless, because merging an entry twice changes nothing.
    pub async fn append<F: Fs>(
        &mut self,
        fs: &F,
        layout: &Layout,
        entries: &[Entry],
    ) -> Result<()> {
        self.writable()?;
        if let Some(foreign) = entries
            .iter()
            .find(|e| e.version.writer != self.writer || e.intent.writer != self.writer)
        {
            return Err(Error::InvalidId {
                what: "entry of this writer",
                text: foreign.version.to_string(),
            });
        }
        if entries.is_empty() {
            return Ok(());
        }
        let lines: String = entries.iter().map(encode_line).collect();
        let written = self.write(fs, layout, lines.as_bytes()).await;
        if written.is_err() {
            self.open_segment = None;
            return written;
        }
        for entry in entries {
            self.observe(entry.version);
            self.past_ids(entry);
        }
        self.appended = self.appended.max(entries.iter().map(|e| e.version).max());
        Ok(())
    }

    async fn write<F: Fs>(&mut self, fs: &F, layout: &Layout, bytes: &[u8]) -> Result<()> {
        if let (Some(number), true) = (self.open_segment, fs.capabilities().append) {
            let path = layout.segment(self.writer, number);
            fs.append(&path, bytes).await?;
            return fs.sync(&path).await;
        }
        let number = take(&mut self.next_segment);
        let dir = layout.writer(self.writer);
        let path = layout.segment(self.writer, number);
        ensure_dir(fs, &dir).await?;
        fs.create(&path, bytes).await?;
        fs.sync(&path).await?;
        fs.sync(&dir).await?;
        self.open_segment = Some(number);
        Ok(())
    }

    /// Close this handle's segment and number later ones past `through`, which a
    /// snapshot is about to fold.
    pub(crate) fn close_through(&mut self, through: u64) {
        self.open_segment = None;
        self.next_segment = self.next_segment.zip(through.checked_add(1)).map(max_pair);
    }

    /// Whether `own` holds every entry this handle has appended.
    pub(crate) fn is_covered_by(&self, own: &WriterLog) -> bool {
        self.appended
            .is_none_or(|version| own.max_lamport() >= version.lamport)
    }
}

fn max_pair((a, b): (u64, u64)) -> u64 {
    a.max(b)
}

/// The counter's value, advancing it; an exhausted counter stays exhausted and
/// yields `u64::MAX`.
fn take(counter: &mut Option<u64>) -> u64 {
    let value = counter.unwrap_or(u64::MAX);
    *counter = counter.and_then(|value| value.checked_add(1));
    value
}

impl Kind {
    /// The `Intent` entry of an intent without a label that reverses nothing.
    pub const INTENT: Kind = Kind::Intent {
        label: None,
        reverses: None,
        files: false,
    };

    /// The entities this entry names, references in values included.
    pub(crate) fn entities(&self) -> Vec<EntityId> {
        let (entity, values): (Option<&EntityId>, [Option<&Value>; 2]) = match self {
            Self::Create { entity } | Self::Delete { entity } => (Some(entity), [None, None]),
            Self::Field {
                entity,
                value,
                prior,
                ..
            } => (Some(entity), [value.as_ref(), prior.as_ref()]),
            Self::SetAdd { entity, value, .. } | Self::SetRemove { entity, value, .. } => {
                (Some(entity), [Some(value), None])
            }
            Self::Intent { .. }
            | Self::File { .. }
            | Self::BlobAdded { .. }
            | Self::BlobRemoved { .. }
            | Self::Unknown { .. } => (None, [None, None]),
        };
        let references = values
            .into_iter()
            .flatten()
            .filter_map(|value| match value {
                Value::Ref(entity) => Some(entity),
                _ => None,
            });
        entity.into_iter().chain(references).copied().collect()
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::collections::BTreeMap;

    use super::*;
    use crate::merge::{merge, State};
    use crate::MemFs;

    pub(crate) fn writer(id: u128) -> WriterId {
        WriterId::from_u128(id)
    }

    pub(crate) fn text(text: &str) -> Value {
        Value::Text(text.to_owned())
    }

    pub(crate) fn field(entity: EntityId, name: &str, value: Option<Value>) -> Kind {
        Kind::Field {
            entity,
            name: name.to_owned(),
            value,
            prior: None,
        }
    }

    /// A fresh handle for `writer` over what `fs` holds now, as a restarted app opens it.
    pub(crate) fn reopen(fs: &MemFs, writer: WriterId) -> LogWriter {
        let logs = pollster::block_on(read_logs(fs, &Layout::default())).unwrap();
        LogWriter::open(writer, &logs)
    }

    /// Every entry `writer`'s log on `fs` holds.
    pub(crate) fn logged(fs: &MemFs, writer: WriterId) -> Vec<Entry> {
        let log = pollster::block_on(read_log(fs, &Layout::default(), writer)).unwrap();
        log.all_entries().cloned().collect()
    }

    /// Append to `writer`'s log a checksummed line of a kind no build knows.
    pub(crate) fn append_unknown(fs: &MemFs, writer: WriterId, lamport: u64) {
        let json =
            format!(r#"{{"version":"{lamport}@{writer}","intent":"{writer}:0","kind":"comment"}}"#);
        let line = format!("{json}\t{:08x}\n", crc32(json.as_bytes()));
        let layout = Layout::default();
        let dir = layout.writer(writer);
        pollster::block_on(ensure_dir(fs, &dir)).unwrap();
        let mut number = 1;
        while pollster::block_on(fs.metadata(&layout.segment(writer, number)))
            .unwrap()
            .is_some()
        {
            number += 1;
        }
        pollster::block_on(fs.create(&layout.segment(writer, number), line.as_bytes())).unwrap();
    }

    /// Whether `entries` give each version once and each intent one `Intent` entry.
    pub(crate) fn well_formed(entries: &[Entry]) -> std::result::Result<(), String> {
        let mut versions = BTreeSet::new();
        let mut heads: BTreeMap<IntentId, usize> = BTreeMap::new();
        for entry in entries {
            if !versions.insert(entry.version) {
                return Err(format!("two entries share version {}", entry.version));
            }
            let head = usize::from(matches!(entry.kind, Kind::Intent { .. }));
            *heads.entry(entry.intent).or_default() += head;
        }
        match heads.into_iter().find(|&(_, count)| count != 1) {
            Some((intent, count)) => Err(format!("intent {intent} has {count} intent entries")),
            None => Ok(()),
        }
    }

    /// The facts of a state, without what its applied entries name.
    pub(crate) fn facts(mut state: State) -> State {
        state.fold_window();
        state
    }

    /// One writer working on a disk, as an app would.
    pub(crate) struct Session {
        pub fs: MemFs,
        pub layout: Layout,
        pub log: LogWriter,
    }

    impl Session {
        pub fn open(fs: &MemFs, writer: WriterId) -> Self {
            let layout = Layout::default();
            let logs = pollster::block_on(read_logs(fs, &layout)).unwrap();
            Self {
                fs: fs.clone(),
                log: LogWriter::open(writer, &logs),
                layout,
            }
        }

        /// Append one intent of `kinds` after its `Intent` entry, each field's prior
        /// taken from the merged state.
        pub fn act(&mut self, kinds: Vec<Kind>) -> IntentId {
            self.act_as(Kind::INTENT, kinds)
        }

        /// As [`Self::act`], under the `Intent` entry `header`.
        pub fn act_as(&mut self, header: Kind, kinds: Vec<Kind>) -> IntentId {
            let state = self.state();
            let intent = self.log.new_intent();
            let kinds = std::iter::once(header).chain(kinds.into_iter().map(|kind| match kind {
                Kind::Field {
                    entity,
                    name,
                    value,
                    ..
                } => Kind::Field {
                    prior: state.field(entity, &name).cloned(),
                    entity,
                    name,
                    value,
                },
                other => other,
            }));
            self.write(intent, kinds.collect()).unwrap();
            intent
        }

        pub fn write(&mut self, intent: IntentId, kinds: Vec<Kind>) -> Result<Vec<Entry>> {
            let entries: Vec<Entry> = kinds
                .into_iter()
                .map(|kind| self.log.stamp(intent, kind))
                .collect();
            pollster::block_on(self.log.append(&self.fs, &self.layout, &entries))?;
            Ok(entries)
        }

        pub fn own(&self) -> WriterLog {
            pollster::block_on(read_log(&self.fs, &self.layout, self.log.writer())).unwrap()
        }

        pub fn state(&self) -> State {
            merge(&pollster::block_on(read_logs(&self.fs, &self.layout)).unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use pollster::block_on;

    use super::testing::*;
    use super::*;
    use crate::fs::Capabilities;
    use crate::MemFs;

    fn entry(writer: WriterId, lamport: u64, kind: Kind) -> Entry {
        Entry {
            version: Version::new(lamport, writer),
            intent: IntentId::new(writer, 1),
            kind,
        }
    }

    fn every_kind() -> Vec<Kind> {
        let entity = EntityId::new(writer(7), 3);
        let blob = BlobId::of(b"bytes");
        vec![
            Kind::Intent {
                label: Some("Rename\ttab".into()),
                reverses: Some(IntentId::new(writer(7), 2)),
                files: true,
            },
            Kind::INTENT,
            Kind::Create { entity },
            Kind::Delete { entity },
            Kind::Field {
                entity,
                name: "tag".into(),
                value: Some(Value::Ref(entity)),
                prior: Some(Value::Blob(blob)),
            },
            field(entity, "tag", None),
            Kind::SetAdd {
                entity,
                name: "tags".into(),
                value: Value::Int(-1),
            },
            Kind::SetRemove {
                entity,
                name: "tags".into(),
                value: Value::Bool(true),
                observed: [Version::new(1, writer(1)), Version::new(2, writer(7))].into(),
            },
            Kind::File {
                path: RelPath::new("a/b c").unwrap(),
                before: Some(blob),
                after: None,
            },
            Kind::BlobAdded {
                blob,
                len: u64::MAX,
            },
            Kind::BlobRemoved { blob },
        ]
    }

    fn lines(entries: &[Entry]) -> Vec<u8> {
        entries
            .iter()
            .map(encode_line)
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn the_checksum_is_crc32_iso_hdlc() {
        // The check value of CRC-32/ISO-HDLC from the CRC catalogue.
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_line_is_the_entry_json_a_tab_and_its_checksum() {
        let w = writer(0xa);
        let entry = Entry {
            version: Version::new(3, w),
            intent: IntentId::new(w, 1),
            kind: field(EntityId::new(w, 0), "tag", Some(text("Brass"))),
        };
        // The checksum is zlib.crc32 of the JSON.
        let expected = format!(
            "{{\"version\":\"3@{w}\",\"intent\":\"{w}:1\",\"kind\":\"field\",\
             \"entity\":\"{w}:0\",\"name\":\"tag\",\"value\":{{\"text\":\"Brass\"}}}}\tc8b0f391\n"
        );
        assert_eq!(encode_line(&entry), expected);
    }

    #[test]
    fn every_kind_reads_back_from_its_line() {
        let entries: Vec<Entry> = every_kind()
            .into_iter()
            .enumerate()
            .map(|(i, kind)| entry(writer(7), i as u64 + 1, kind))
            .collect();
        let segment = read_segment(&lines(&entries));
        assert_eq!(segment.torn, None);
        assert_eq!(segment.entries, entries);
    }

    #[test]
    fn a_final_line_without_its_newline_costs_one_entry() {
        let entries: Vec<Entry> = (1..=3)
            .map(|i| {
                entry(
                    writer(1),
                    i,
                    Kind::Create {
                        entity: EntityId::new(writer(1), i),
                    },
                )
            })
            .collect();
        let mut bytes = lines(&entries);
        bytes.pop();
        let offset = lines(&entries[..2]).len() as u64;
        let segment = read_segment(&bytes);
        assert_eq!(segment.entries, entries[..2]);
        assert_eq!(segment.torn, Some(Torn { offset }));
    }

    #[test]
    fn a_line_failing_its_checksum_ends_the_segment() {
        let entries: Vec<Entry> = (1..=3)
            .map(|i| {
                entry(
                    writer(1),
                    i,
                    Kind::Create {
                        entity: EntityId::new(writer(1), i),
                    },
                )
            })
            .collect();
        let offset = lines(&entries[..1]).len();
        let mut bytes = lines(&entries);
        let digit = bytes[offset..].iter().position(|&b| b == b'@').unwrap() - 1;
        bytes[offset + digit] = b'9';
        let segment = read_segment(&bytes);
        assert_eq!(segment.entries, entries[..1]);
        assert_eq!(
            segment.torn,
            Some(Torn {
                offset: offset as u64
            })
        );
    }

    #[test]
    fn a_checksummed_line_without_an_entry_envelope_ends_the_segment() {
        let w = writer(1);
        for json in [
            "[]".to_owned(),
            format!(r#"{{"intent":"{w}:1","kind":"create"}}"#),
            format!(r#"{{"version":"01@{w}","intent":"{w}:1","kind":"create"}}"#),
            format!(r#"{{"version":"1@{w}","intent":"{w}:1"}}"#),
            format!(r#"{{"version":"1@{w}","intent":"{w}:1","kind":7}}"#),
        ] {
            let line = format!("{json}\t{:08x}\n", crc32(json.as_bytes()));
            let segment = read_segment(line.as_bytes());
            assert_eq!(segment.entries, [], "{json}");
            assert_eq!(segment.torn, Some(Torn { offset: 0 }), "{json}");
        }
    }

    #[test]
    fn an_unknown_kind_or_member_is_kept_verbatim() {
        let w = writer(1);
        let entity = EntityId::new(w, 0);
        for json in [
            format!(r#"{{"version":"1@{w}","intent":"{w}:1","kind":"comment","text":"hi"}}"#),
            format!(
                r#"{{"kind":"create","entity":"{entity}","weight":2,"version":"1@{w}","intent":"{w}:1"}}"#
            ),
            format!(
                r#"{{"version":"1@{w}","intent":"{w}:1","kind":"field","entity":"{entity}","name":"x","value":{{"float":1.5}}}}"#
            ),
        ] {
            let line = format!("{json}\t{:08x}\n", crc32(json.as_bytes()));
            let segment = read_segment(line.as_bytes());
            let [read] = segment.entries.as_slice() else {
                panic!("{json} read as {segment:?}");
            };
            assert!(matches!(&read.kind, Kind::Unknown { json: kept, .. } if *kept == json));
            assert_eq!(
                (read.version, read.intent),
                (Version::new(1, w), IntentId::new(w, 1))
            );
            assert_eq!(encode_line(read), line);
        }
    }

    #[test]
    fn appended_entries_survive_a_crash_once_append_returns() {
        for capabilities in [Capabilities::ALL, Capabilities::NONE] {
            let fs = MemFs::with_capabilities(capabilities);
            let mut session = Session::open(&fs, writer(1));
            session.act(vec![Kind::Create {
                entity: EntityId::new(writer(1), 0),
            }]);
            session.act(vec![Kind::Delete {
                entity: EntityId::new(writer(1), 0),
            }]);
            let written = session.own().entries;
            let restarted = Session::open(&fs.restart(), writer(1));
            assert_eq!(restarted.own().entries, written, "{capabilities:?}");
            assert_eq!(written.len(), 4);
        }
    }

    #[test]
    fn a_session_writes_its_own_segments() {
        let cases = [
            (Capabilities::ALL, vec![1], vec![1, 2]),
            (Capabilities::NONE, vec![1, 2], vec![1, 2, 3]),
        ];
        for (capabilities, first, second) in cases {
            let fs = MemFs::with_capabilities(capabilities);
            let mut session = Session::open(&fs, writer(1));
            session.act(vec![]);
            session.act(vec![]);
            assert_eq!(session.own().segments, first, "{capabilities:?}");
            let mut next = Session::open(&fs, writer(1));
            next.act(vec![]);
            assert_eq!(next.own().segments, second, "{capabilities:?}");
        }
    }

    #[test]
    fn a_torn_tail_is_reported_and_left_behind() {
        let fs = MemFs::new();
        let mut session = Session::open(&fs, writer(1));
        session.act(vec![]);
        let segment = session.layout.segment(writer(1), 1);
        let torn = Torn {
            offset: block_on(fs.metadata(&segment)).unwrap().unwrap().len,
        };
        block_on(fs.append(&segment, b"{\"version\":")).unwrap();
        let mut next = Session::open(&fs, writer(1));
        next.act(vec![]);
        let own = next.own();
        assert_eq!(own.torn, [(1, torn)]);
        assert_eq!(own.segments, [1, 2]);
        assert_eq!(own.entries.len(), 2);
    }

    #[test]
    fn the_clock_starts_past_every_version_seen() {
        let fs = MemFs::new();
        let mut other = Session::open(&fs, writer(2));
        let unknown = format!(
            r#"{{"version":"40@{}","intent":"{}:0","kind":"comment"}}"#,
            writer(2),
            writer(2)
        );
        other.act(vec![]);
        let line = format!("{unknown}\t{:08x}\n", crc32(unknown.as_bytes()));
        block_on(fs.append(&other.layout.segment(writer(2), 1), line.as_bytes())).unwrap();

        let mut session = Session::open(&fs, writer(1));
        let intent = session.log.new_intent();
        let next = |log: &mut LogWriter| log.stamp(intent, Kind::INTENT).version;
        assert_eq!(next(&mut session.log), Version::new(41, writer(1)));
        session.log.observe(Version::new(99, writer(3)));
        assert_eq!(next(&mut session.log), Version::new(100, writer(1)));
        session.log.observe(Version::new(5, writer(3)));
        assert_eq!(next(&mut session.log), Version::new(101, writer(1)));
    }

    #[test]
    fn ids_never_collide_across_sessions_or_writers() {
        let fs = MemFs::new();
        let mut entities = BTreeSet::new();
        let mut intents = BTreeSet::new();
        for round in 0..3 {
            for id in [1, 2] {
                let mut session = Session::open(&fs, writer(id));
                let created = [session.log.new_entity(), session.log.new_entity()];
                for entity in created {
                    assert!(entities.insert(entity), "round {round}: {entity} reused");
                }
                let intent = session.act(created.map(|entity| Kind::Create { entity }).into());
                assert!(intents.insert(intent), "round {round}: {intent} reused");
            }
        }
    }

    #[test]
    fn a_writer_allocates_past_its_ids_that_other_writers_mention() {
        let fs = MemFs::new();
        let mentioned = EntityId::new(writer(1), 50);
        let mut other = Session::open(&fs, writer(2));
        other.act(vec![field(
            EntityId::new(writer(2), 0),
            "link",
            Some(Value::Ref(mentioned)),
        )]);
        let mut session = Session::open(&fs, writer(1));
        assert_eq!(session.log.new_entity(), EntityId::new(writer(1), 51));
    }

    #[test]
    fn a_writer_whose_own_log_holds_an_unknown_entry_is_read_only() {
        let fs = MemFs::new();
        let mut session = Session::open(&fs, writer(1));
        session.act(vec![]);
        let json = format!(
            r#"{{"version":"9@{}","intent":"{}:5","kind":"comment"}}"#,
            writer(1),
            writer(1)
        );
        let line = format!("{json}\t{:08x}\n", crc32(json.as_bytes()));
        block_on(fs.append(&session.layout.segment(writer(1), 1), line.as_bytes())).unwrap();
        let before = fs.files();

        let mut reopened = Session::open(&fs, writer(1));
        assert!(reopened.log.read_only().is_some());
        let intent = reopened.log.new_intent();
        let refused = reopened.write(intent, vec![Kind::INTENT]);
        assert!(
            matches!(refused, Err(Error::ReadOnly { .. })),
            "{refused:?}"
        );
        assert_eq!(fs.files(), before);

        let other = Session::open(&fs, writer(2));
        assert_eq!(other.log.read_only(), None);
    }

    #[test]
    fn append_refuses_another_writers_entries() {
        let fs = MemFs::new();
        let mut session = Session::open(&fs, writer(1));
        let foreign = entry(writer(2), 1, Kind::INTENT);
        let refused = block_on(session.log.append(&fs, &session.layout, &[foreign]));
        assert!(
            matches!(refused, Err(Error::InvalidId { .. })),
            "{refused:?}"
        );
        assert!(fs.files().is_empty());
    }
}
