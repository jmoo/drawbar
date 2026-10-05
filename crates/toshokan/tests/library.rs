//! A library as an app uses it, through [`Library`]: each behavior runs on the
//! in-memory file system and on the native one, and every file effect is crashed at
//! each of its operations.

use std::collections::{BTreeMap, BTreeSet};

use pollster::block_on;
use toshokan::fs::{Capabilities, FileKind};
use toshokan::journal::Outcome;
use toshokan::undo::Refusal;
use toshokan::{
    BlobId, EntityId, Error, Fs, Kind, Layout, Library, MemFs, Precondition, RelPath, Value,
    WriterId,
};

const A: WriterId = WriterId::from_u128(0xa);
const B: WriterId = WriterId::from_u128(0xb);
const C: WriterId = WriterId::from_u128(0xc);

fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

fn text(text: &str) -> Value {
    Value::Text(text.to_owned())
}

fn holds(bytes: &[u8]) -> Precondition {
    Precondition::Holds(BlobId::of(bytes))
}

fn open<F: Fs>(fs: F, writer: WriterId) -> Library<F> {
    block_on(Library::open(fs, Layout::default(), writer)).unwrap()
}

/// Every directory and file in the folder: a file with its bytes and time.
type Tree = BTreeMap<RelPath, Option<(Vec<u8>, Option<u64>)>>;

fn tree<F: Fs>(fs: &F) -> Tree {
    let mut tree = BTreeMap::new();
    let mut dirs = vec![RelPath::ROOT];
    while let Some(dir) = dirs.pop() {
        for entry in block_on(fs.list(&dir)).unwrap() {
            let child = dir.join(&entry.name).unwrap();
            let node = match entry.kind {
                FileKind::Directory => {
                    dirs.push(child.clone());
                    None
                }
                FileKind::File => {
                    let bytes = block_on(fs.read(&child)).unwrap();
                    let modified = block_on(fs.metadata(&child)).unwrap().unwrap().modified;
                    Some((bytes, modified))
                }
            };
            tree.insert(child, node);
        }
    }
    tree
}

/// The library's own files, outside toshokan's root, with their bytes.
fn library_files<F: Fs>(fs: &F) -> BTreeMap<RelPath, Vec<u8>> {
    let layout = Layout::default();
    tree(fs)
        .into_iter()
        .filter(|(path, _)| !layout.owns(path))
        .filter_map(|(path, node)| Some((path, node?.0)))
        .collect()
}

fn stored<F: Fs>(fs: &F, bytes: &[u8]) -> bool {
    let blob = Layout::default().blob(BlobId::of(bytes));
    block_on(fs.metadata(&blob)).unwrap().is_some()
}

/// CRC-32/ISO-HDLC computed bit by bit, as the line format specifies it.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = match crc & 1 {
                1 => (crc >> 1) ^ 0xedb8_8320,
                _ => crc >> 1,
            };
        }
    }
    !crc
}

/// Append to `writer`'s first segment a well-formed line of a kind no build knows.
fn append_unknown<F: Fs>(fs: &F, writer: WriterId) -> String {
    let json = format!(
        r#"{{"version":"900@{writer}","intent":"{writer}:900","kind":"comment","text":"hi"}}"#
    );
    let line = format!("{json}\t{:08x}\n", crc32(json.as_bytes()));
    let segment = Layout::default().segment(writer, 1);
    block_on(fs.append(&segment, line.as_bytes())).unwrap();
    json
}

fn two_writers_tag_one_library_and_both_tags_survive<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    let mut b = open(disk(), B);
    block_on(a.add(song, "tags", text("brass"))).unwrap();
    block_on(b.add(song, "tags", text("warm"))).unwrap();
    block_on(b.add(song, "tags", text("brass"))).unwrap();
    block_on(a.remove(song, "tags", &text("brass"))).unwrap();

    let reader = open(disk(), C);
    let tags: BTreeSet<&Value> = reader.state().members(song, "tags");
    assert_eq!(
        tags,
        [&text("brass"), &text("warm")].into(),
        "a remove spares the add it did not see"
    );
}

fn two_writers_never_allocate_the_same_id<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let mut b = open(disk(), B);
    let mut created = Vec::new();
    for _ in 0..3 {
        created.push(block_on(a.create()).unwrap().0);
        created.push(block_on(b.create()).unwrap().0);
    }
    drop(a);
    let mut again = open(disk(), A);
    created.push(block_on(again.create()).unwrap().0);

    let distinct: BTreeSet<EntityId> = created.iter().copied().collect();
    assert_eq!(distinct.len(), created.len(), "{created:?}");
    assert_eq!(open(disk(), C).state().entities(), Vec::from_iter(distinct));
}

fn a_writer_never_drops_another_writers_unknown_entries<F: Fs>(disk: impl Fn() -> F) {
    let mut b = open(disk(), B);
    block_on(b.create()).unwrap();
    let unknown = append_unknown(b.fs(), B);
    let theirs = |fs: &F| {
        let dir = Layout::default().writer(B);
        tree(fs)
            .into_iter()
            .filter(|(path, _)| path.starts_with(&dir))
            .collect::<Tree>()
    };
    let before = theirs(b.fs());

    let mut a = open(disk(), A);
    assert_eq!(a.read_only(), None);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.set(song, "name", Some(text("one")))).unwrap();
    block_on(a.compact(0)).unwrap();
    block_on(a.collect(0)).unwrap();
    assert_eq!(theirs(a.fs()), before, "writer B's files changed");

    let b = open(disk(), B);
    assert!(b.read_only().is_some(), "B's own log has an unknown entry");
    let kept = block_on(toshokan::log::read_log(b.fs(), b.layout(), B)).unwrap();
    let kept: Vec<&Kind> = kept.entries.iter().map(|entry| &entry.kind).collect();
    assert!(
        kept.iter()
            .any(|kind| matches!(kind, Kind::Unknown { json, .. } if *json == unknown)),
        "{kept:?}"
    );
}

fn a_torn_log_tail_costs_one_entry<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.set(song, "name", Some(text("one")))).unwrap();
    block_on(a.set(song, "name", Some(text("two")))).unwrap();
    drop(a);

    let fs = disk();
    let segment = Layout::default().segment(A, 1);
    let mut bytes = block_on(fs.read(&segment)).unwrap();
    assert_eq!(bytes.pop(), Some(b'\n'));
    block_on(fs.remove_file(&segment)).unwrap();
    block_on(fs.create(&segment, &bytes)).unwrap();
    let offset = bytes.iter().rposition(|&b| b == b'\n').unwrap() as u64 + 1;

    let mut a = open(disk(), A);
    assert_eq!(a.state().field(song, "name"), Some(&text("one")));
    let torn: Vec<_> = a
        .torn()
        .iter()
        .map(|t| (t.writer, t.segment, t.torn.offset))
        .collect();
    assert_eq!(torn, [(A, 1, offset)]);

    block_on(a.set(song, "name", Some(text("three")))).unwrap();
    let reopened = open(disk(), A);
    assert_eq!(reopened.state().field(song, "name"), Some(&text("three")));
    assert_eq!(
        reopened.torn().len(),
        1,
        "the torn segment is left as it was"
    );
}

fn a_read_only_library_refuses_every_intent_before_anything_changes<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.add(song, "tags", text("brass"))).unwrap();
    block_on(a.save(song, &path("d/song"), b"old".to_vec(), Precondition::Absent)).unwrap();
    append_unknown(a.fs(), A);
    let before = tree(a.fs());

    let mut a = open(disk(), A);
    assert!(a.read_only().is_some());
    let results = [
        block_on(a.create()).map(drop),
        block_on(a.set(song, "name", None)).map(drop),
        block_on(a.add(song, "tags", text("warm"))).map(drop),
        block_on(a.remove(song, "tags", &text("brass"))).map(drop),
        block_on(a.delete(song)).map(drop),
        block_on(a.bind(song, &path("d/song"))).map(drop),
        block_on(a.save(song, &path("d/song"), b"new".to_vec(), holds(b"old"))).map(drop),
        block_on(a.rename(song, &path("d/renamed"))).map(drop),
        block_on(a.move_tree(&path("d"), &path("e"))).map(drop),
        block_on(a.delete_file(song, holds(b"old"))).map(drop),
        block_on(a.undo()).map(drop),
        block_on(a.redo()).map(drop),
        block_on(a.compact(0)).map(drop),
        block_on(a.collect(0)).map(drop),
    ];
    for (index, result) in results.into_iter().enumerate() {
        assert!(
            matches!(result, Err(Error::ReadOnly { writer: A, .. })),
            "intent {index}: {result:?}"
        );
    }
    assert_eq!(tree(a.fs()), before, "a refused intent wrote");
}

fn undo_of_a_save_restores_the_displaced_bytes<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    let at = path("Organ/song");
    block_on(a.save(song, &at, b"first".to_vec(), Precondition::Absent)).unwrap();
    let saved = block_on(a.save(song, &at, b"second".to_vec(), holds(b"first"))).unwrap();
    assert_eq!(saved.files.displaced, [(at.clone(), BlobId::of(b"first"))]);

    let undone = block_on(a.undo()).unwrap();
    assert_eq!(
        library_files(a.fs()),
        [(at.clone(), b"first".to_vec())].into()
    );
    assert!(stored(a.fs(), b"second"), "the undone bytes are kept");
    assert_eq!(
        undone.files.displaced,
        [(at.clone(), BlobId::of(b"second"))]
    );

    block_on(a.redo()).unwrap();
    assert_eq!(
        library_files(a.fs()),
        [(at.clone(), b"second".to_vec())].into()
    );

    block_on(a.undo()).unwrap();
    block_on(a.undo()).unwrap();
    assert_eq!(
        library_files(a.fs()),
        BTreeMap::new(),
        "undoing the first save"
    );
    assert!(stored(a.fs(), b"first") && stored(a.fs(), b"second"));
    let reopened = open(disk(), A);
    assert_eq!(reopened.state().field(song, "path"), None);
}

fn saving_an_entity_away_from_its_file_is_refused<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.save(song, &path("a"), b"first".to_vec(), Precondition::Absent)).unwrap();
    let before = tree(a.fs());

    let refused = block_on(a.save(song, &path("b"), b"second".to_vec(), Precondition::Absent));
    assert!(
        matches!(refused, Err(Error::Entity { entity, .. }) if entity == song),
        "{refused:?}"
    );
    assert_eq!(tree(a.fs()), before, "a refused save wrote");
    block_on(a.undo()).unwrap();
    assert_eq!(library_files(a.fs()), BTreeMap::new());
    assert!(stored(a.fs(), b"first"), "undoing the save kept its bytes");
}

fn undo_of_a_file_delete_puts_the_file_back<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.save(song, &path("song"), b"bytes".to_vec(), Precondition::Absent)).unwrap();
    block_on(a.delete_file(song, holds(b"bytes"))).unwrap();
    assert_eq!(library_files(a.fs()), BTreeMap::new());

    block_on(a.undo()).unwrap();
    assert_eq!(
        library_files(a.fs()),
        [(path("song"), b"bytes".to_vec())].into()
    );
    assert_eq!(a.state().field(song, "path"), Some(&text("song")));
    assert_eq!(
        block_on(a.rescan()).unwrap().bound,
        [(path("song"), song)].into()
    );
}

fn undo_is_refused_where_another_writer_changed_the_field_since<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.set(song, "name", Some(text("mine")))).unwrap();
    let mut b = open(disk(), B);
    block_on(b.set(song, "name", Some(text("theirs")))).unwrap();

    block_on(a.refresh()).unwrap();
    let before = tree(a.fs());
    let refused = block_on(a.undo());
    assert!(
        matches!(&refused, Err(Error::Refused(refusal))
            if matches!(**refusal, Refusal::FieldChanged { entity, .. } if entity == song)),
        "{refused:?}"
    );
    assert_eq!(tree(a.fs()), before, "a refused undo wrote");
    assert_eq!(a.state().field(song, "name"), Some(&text("theirs")));
}

fn compaction_never_turns_an_undo_into_a_redo<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.set(song, "name", Some(text("one")))).unwrap();
    block_on(a.undo()).unwrap();
    block_on(a.compact(1)).unwrap();

    for refused in [block_on(a.undo()), block_on(a.redo())] {
        assert!(
            matches!(&refused, Err(Error::Refused(refusal)) if **refusal == Refusal::Nothing),
            "{refused:?}"
        );
    }
    assert_eq!(a.state().field(song, "name"), None);
}

fn collection_never_removes_a_blob_a_live_value_names<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.save(song, &path("song"), b"first".to_vec(), Precondition::Absent)).unwrap();
    block_on(a.save(song, &path("song"), b"second".to_vec(), holds(b"first"))).unwrap();
    let (keeper, _) = block_on(a.create()).unwrap();
    let first = Value::Blob(BlobId::of(b"first"));
    block_on(a.set(keeper, "previous", Some(first))).unwrap();

    block_on(a.compact(0)).unwrap();
    let collection = block_on(a.collect(0)).unwrap();
    assert_eq!(collection.removed, []);
    assert!(stored(a.fs(), b"first"));

    block_on(a.set(keeper, "previous", None)).unwrap();
    block_on(a.compact(0)).unwrap();
    let collection = block_on(a.collect(0)).unwrap();
    assert_eq!(collection.removed, [BlobId::of(b"first")]);
    assert!(!stored(a.fs(), b"first"));
}

fn collection_reaches_only_blobs_older_than_the_undo_window<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    let at = path("song");
    block_on(a.save(song, &at, b"1".to_vec(), Precondition::Absent)).unwrap();
    block_on(a.save(song, &at, b"2".to_vec(), holds(b"1"))).unwrap();
    block_on(a.save(song, &at, b"3".to_vec(), holds(b"2"))).unwrap();

    assert_eq!(block_on(a.collect(0)).unwrap().removed, []);
    block_on(a.compact(1)).unwrap();
    assert_eq!(block_on(a.collect(0)).unwrap().removed, [BlobId::of(b"1")]);
    block_on(a.undo()).unwrap();
    assert_eq!(library_files(a.fs()), [(at, b"2".to_vec())].into());
}

fn opening_writes_nothing<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.save(song, &path("d/song"), b"old".to_vec(), Precondition::Absent)).unwrap();
    block_on(a.save(song, &path("d/song"), b"new".to_vec(), holds(b"old"))).unwrap();
    block_on(a.add(song, "tags", text("brass"))).unwrap();
    block_on(a.compact(1)).unwrap();
    let mut b = open(disk(), B);
    block_on(b.set(song, "name", Some(text("theirs")))).unwrap();
    let fs = disk();
    block_on(fs.create(&path("arrived"), b"outside")).unwrap();
    let before = tree(&fs);

    for writer in [A, B, C] {
        let library = open(disk(), writer);
        assert_eq!(library.scan().arrivals, [path("arrived")]);
        assert_eq!(tree(&fs), before, "opening as {writer} wrote");
    }
}

fn an_intent_the_entity_does_not_allow_is_refused_before_anything_changes<F: Fs>(
    disk: impl Fn() -> F,
) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    let (gone, _) = block_on(a.create()).unwrap();
    block_on(a.delete(gone)).unwrap();
    let before = tree(a.fs());
    let results = [
        block_on(a.set(gone, "name", None)).map(drop),
        block_on(a.set(song, "path", Some(text("elsewhere")))).map(drop),
        block_on(a.set(song, "content", None)).map(drop),
        block_on(a.set(song, "modified", None)).map(drop),
        block_on(a.remove(song, "tags", &text("never added"))).map(drop),
        block_on(a.rename(song, &path("to"))).map(drop),
        block_on(a.delete_file(song, Precondition::Absent)).map(drop),
    ];
    for (index, result) in results.into_iter().enumerate() {
        assert!(
            matches!(result, Err(Error::Entity { .. })),
            "intent {index}: {result:?}"
        );
    }
    assert_eq!(tree(a.fs()), before, "a refused intent wrote");
}

fn reopening_an_untouched_library_reads_no_file<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    for (at, bytes) in [("a", b"one"), ("d/b", b"two")] {
        let (entity, _) = block_on(a.create()).unwrap();
        block_on(a.save(entity, &path(at), bytes.to_vec(), Precondition::Absent)).unwrap();
    }
    block_on(a.fs().create(&path("c"), b"three")).unwrap();
    let (entity, _) = block_on(a.create()).unwrap();
    block_on(a.bind(entity, &path("c"))).unwrap();

    let reopened = open(disk(), A);
    let scan = reopened.scan();
    assert_eq!(scan.bound.len(), 3, "{scan:?}");
    assert!(scan.changed.is_empty(), "{scan:?}");
    for (at, print) in &scan.files {
        assert_eq!(print.hash, None, "{at} was read");
    }
}

fn binding_follows_a_file_renamed_outside_the_app<F: Fs>(disk: impl Fn() -> F) {
    let mut a = open(disk(), A);
    let (song, _) = block_on(a.create()).unwrap();
    block_on(a.save(
        song,
        &path("a/song"),
        b"bytes".to_vec(),
        Precondition::Absent,
    ))
    .unwrap();
    let fs = disk();
    block_on(fs.create_dir_all(&path("b"))).unwrap();
    block_on(fs.rename(&path("a/song"), &path("b/song"))).unwrap();

    let moves = block_on(a.rescan()).unwrap().moves.clone();
    let [moved] = moves.as_slice() else {
        panic!("{moves:?}");
    };
    assert_eq!((moved.entity, &moved.to), (song, &path("b/song")));
    block_on(a.bind(song, &moved.to)).unwrap();
    let scan = block_on(a.rescan()).unwrap();
    assert_eq!(scan.bound, [(path("b/song"), song)].into());
    assert!(scan.moves.is_empty() && scan.changed.is_empty(), "{scan:?}");
}

macro_rules! on_every_backend {
    ($($behavior:ident,)+) => {
        mod mem {
            use super::*;
            $(
                #[test]
                fn $behavior() {
                    let fs = MemFs::new();
                    super::$behavior(|| fs.clone());
                }
            )+
        }

        mod native {
            $(
                #[test]
                fn $behavior() {
                    let dir = tempfile::tempdir().unwrap();
                    super::$behavior(|| toshokan::native::NativeFs::new(dir.path()));
                }
            )+
        }
    };
}

on_every_backend!(
    two_writers_tag_one_library_and_both_tags_survive,
    two_writers_never_allocate_the_same_id,
    a_writer_never_drops_another_writers_unknown_entries,
    a_torn_log_tail_costs_one_entry,
    a_read_only_library_refuses_every_intent_before_anything_changes,
    undo_of_a_save_restores_the_displaced_bytes,
    saving_an_entity_away_from_its_file_is_refused,
    undo_of_a_file_delete_puts_the_file_back,
    undo_is_refused_where_another_writer_changed_the_field_since,
    compaction_never_turns_an_undo_into_a_redo,
    collection_never_removes_a_blob_a_live_value_names,
    collection_reaches_only_blobs_older_than_the_undo_window,
    opening_writes_nothing,
    an_intent_the_entity_does_not_allow_is_refused_before_anything_changes,
    reopening_an_untouched_library_reads_no_file,
    binding_follows_a_file_renamed_outside_the_app,
);

#[test]
fn a_full_disk_gives_up_undo_history_before_refusing_a_save() {
    let fs = MemFs::new();
    let mut a = open(fs.clone(), A);
    let (song, _) = block_on(a.create()).unwrap();
    let at = path("song");
    let [first, second, third] = b"123".map(|byte| vec![byte; 10_000]);
    block_on(a.save(song, &at, first.clone(), Precondition::Absent)).unwrap();
    block_on(a.save(song, &at, second.clone(), holds(&first))).unwrap();
    let used: usize = fs.files().values().map(Vec::len).sum();
    fs.set_capacity(Some(used as u64 + 5_000));

    block_on(a.save(song, &at, third.clone(), holds(&second))).unwrap();
    assert_eq!(library_files(&fs), [(at.clone(), third.clone())].into());
    assert!(
        !stored(&fs, &first),
        "the bytes only undo needed were collected"
    );
    let refused = block_on(a.undo());
    assert!(matches!(refused, Err(Error::NoSpace { .. })), "{refused:?}");
    assert!(stored(&fs, &second), "an undo keeps the bytes it restores");
    fs.set_capacity(None);
    block_on(a.undo()).unwrap();
    assert_eq!(library_files(&fs), [(at.clone(), second.clone())].into());
    let refused = block_on(a.undo());
    assert!(
        matches!(&refused, Err(Error::Refused(refusal)) if **refusal == Refusal::Nothing),
        "the history before the full disk is gone: {refused:?}"
    );

    let used: usize = fs.files().values().map(Vec::len).sum();
    fs.set_capacity(Some(used as u64));
    let refused = block_on(a.save(song, &at, first.clone(), holds(&second)));
    assert!(matches!(refused, Err(Error::NoSpace { .. })), "{refused:?}");
    assert_eq!(library_files(&fs), [(at, second)].into());
}

/// The facts and files a crash may leave: those before the intent, or those after.
#[derive(PartialEq, Debug)]
struct Observed {
    files: BTreeMap<RelPath, Vec<u8>>,
    facts: BTreeMap<EntityId, BTreeMap<String, Value>>,
}

impl Observed {
    fn of(library: &Library<MemFs>) -> Self {
        let state = library.state();
        let facts = state
            .entities()
            .into_iter()
            .map(|entity| {
                let fields = state.fields(entity);
                let fields = fields.into_iter().map(|(n, v)| (n.to_owned(), v.clone()));
                (entity, fields.collect())
            })
            .collect();
        Self {
            files: library_files(library.fs()),
            facts,
        }
    }
}

/// One intent with file effects, crashed at each of its operations and at each
/// operation of the recovery that follows.
struct Case {
    setup: fn(&mut Library<MemFs>) -> Vec<EntityId>,
    act: fn(&mut Library<MemFs>, &[EntityId]) -> toshokan::Result<toshokan::Change>,
}

impl Case {
    fn prepared(&self, disk: Disk) -> (Library<MemFs>, Vec<EntityId>) {
        let mut library = open(disk.fresh(), A);
        let entities = (self.setup)(&mut library);
        (library, entities)
    }

    /// The disk as a crash `crash` operations into the intent leaves it.
    fn crashed(&self, disk: Disk, crash: u64) -> MemFs {
        let (mut library, entities) = self.prepared(disk);
        library.fs().crash_after(crash);
        let result = (self.act)(&mut library, &entities);
        assert!(
            matches!(result, Err(Error::Crashed)),
            "crash {crash}: {result:?}"
        );
        library.fs().restart()
    }

    fn run(&self, kind: Disk) {
        let (mut clean, entities) = self.prepared(kind);
        let before = Observed::of(&clean);
        let start = clean.fs().mutations();
        (self.act)(&mut clean, &entities).unwrap();
        let operations = clean.fs().mutations() - start;
        let after = Observed::of(&clean);
        let durable = Observed::of(&open(clean.fs().restart(), A));
        assert_eq!(durable, after, "the clean intent is durable");
        assert_ne!(after, before, "the intent changes something");

        for crash in 0..operations {
            let disk = self.crashed(kind, crash);
            let start = disk.mutations();
            open(disk.clone(), A);
            let recovery = disk.mutations() - start;
            for interrupt in (0..recovery).map(Some).chain([None]) {
                let mut disk = self.crashed(kind, crash);
                if let Some(interrupt) = interrupt {
                    disk.crash_after(interrupt);
                    let result = block_on(Library::open(disk.clone(), Layout::default(), A));
                    assert!(matches!(result, Err(Error::Crashed)), "{:?}", result.err());
                    disk = disk.restart();
                }
                let at = format!("{kind:?}, crash {crash}, recovery crash {interrupt:?}");
                check(&disk, &before, &after).unwrap_or_else(|failure| panic!("{at}: {failure}"));
            }
        }
    }
}

/// Recover `disk` as a restarted app does, and check what the brief promises.
fn check(disk: &MemFs, before: &Observed, after: &Observed) -> Result<(), String> {
    let library = open(disk.clone(), A);
    let outcome = Observed::of(&library);
    if outcome != *before && outcome != *after && !kept_both(&library, &outcome, before, after) {
        return Err(format!("partial state {outcome:?}"));
    }
    let layout = Layout::default();
    let leftovers: Vec<RelPath> = disk
        .files()
        .into_keys()
        .filter(|path| path.starts_with(&layout.journal(A)) || path.starts_with(&layout.tmp(A)))
        .collect();
    if !leftovers.is_empty() {
        return Err(format!("recovery left {leftovers:?}"));
    }
    let adds = library.state().blob_adds();
    for (path, bytes) in disk.files() {
        if path.parent() != Some(layout.blobs()) {
            continue;
        }
        let blob = BlobId::of(&bytes);
        if path != layout.blob(blob) {
            return Err(format!("{path} holds other bytes"));
        }
        if !adds
            .get(&blob)
            .is_some_and(|by| by.values().any(|add| !add.removed))
        {
            return Err(format!("blob {blob} is stored but not logged"));
        }
    }
    let kept: BTreeSet<Vec<u8>> = disk.files().into_values().collect();
    if let Some(lost) = before.files.values().find(|bytes| !kept.contains(*bytes)) {
        return Err(format!("{:?} were lost", String::from_utf8_lossy(lost)));
    }
    let restarted = disk.restart();
    let mutations = restarted.mutations();
    open(restarted.clone(), A);
    if restarted.mutations() != mutations {
        return Err("recovery was not finished and durable".to_owned());
    }
    Ok(())
}

/// Whether recovery rolled the intent back and reported it, keeping the files both
/// before and after it. A directory renamed across parents, with a crash between
/// syncing the two, has both names, and neither can be removed without the other.
fn kept_both(
    library: &Library<MemFs>,
    outcome: &Observed,
    before: &Observed,
    after: &Observed,
) -> bool {
    let both: BTreeMap<RelPath, Vec<u8>> = before
        .files
        .iter()
        .chain(&after.files)
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();
    let reported = library
        .recovered()
        .iter()
        .any(|recovered| recovered.outcome == Outcome::RolledBack && !recovered.paths.is_empty());
    reported && outcome.facts == before.facts && outcome.files == both
}

/// The kind of disk a case runs on.
#[derive(Clone, Copy, Debug)]
struct Disk {
    capabilities: Capabilities,
    eager_names: bool,
}

impl Disk {
    fn fresh(self) -> MemFs {
        let fs = MemFs::with_capabilities(self.capabilities);
        fs.set_eager_names(self.eager_names);
        fs
    }
}

const ALL: Disk = Disk {
    capabilities: Capabilities::ALL,
    eager_names: false,
};

const WITHOUT_FSYNC: Disk = Disk {
    capabilities: Capabilities {
        fsync: false,
        ..Capabilities::ALL
    },
    ..ALL
};

const WITHOUT_DIRECTORY_RENAME: Disk = Disk {
    capabilities: Capabilities {
        rename_dir: false,
        ..Capabilities::ALL
    },
    ..ALL
};

/// A disk that may make a new name durable before its contents.
const EAGER_NAMES: Disk = Disk {
    eager_names: true,
    ..ALL
};

fn save_new(library: &mut Library<MemFs>, at: &str, bytes: &[u8]) -> EntityId {
    let (entity, _) = block_on(library.create()).unwrap();
    block_on(library.save(entity, &path(at), bytes.to_vec(), Precondition::Absent)).unwrap();
    entity
}

#[test]
fn every_crash_while_saving_a_new_file_recovers() {
    let case = Case {
        setup: |library| {
            save_new(library, "keep", b"k");
            vec![block_on(library.create()).unwrap().0]
        },
        act: |library, entities| {
            let bytes = b"new".to_vec();
            block_on(library.save(entities[0], &path("d/new"), bytes, Precondition::Absent))
        },
    };
    for disk in [ALL, WITHOUT_FSYNC, EAGER_NAMES] {
        case.run(disk);
    }
}

#[test]
fn every_crash_while_saving_over_a_file_recovers() {
    let case = Case {
        setup: |library| vec![save_new(library, "d/song", b"old")],
        act: |library, entities| {
            let bytes = b"new".to_vec();
            block_on(library.save(entities[0], &path("d/song"), bytes, holds(b"old")))
        },
    };
    for disk in [ALL, WITHOUT_FSYNC, EAGER_NAMES] {
        case.run(disk);
    }
}

#[test]
fn every_crash_while_deleting_a_file_recovers() {
    let case = Case {
        setup: |library| {
            save_new(library, "other", b"o");
            vec![save_new(library, "song", b"old")]
        },
        act: |library, entities| block_on(library.delete_file(entities[0], holds(b"old"))),
    };
    for disk in [ALL, WITHOUT_FSYNC, EAGER_NAMES] {
        case.run(disk);
    }
}

#[test]
fn every_crash_while_moving_a_tree_recovers() {
    let case = Case {
        setup: bound_tree,
        act: |library, _| block_on(library.move_tree(&path("a/x"), &path("b/x"))),
    };
    let without_both = Disk {
        capabilities: Capabilities {
            fsync: false,
            ..WITHOUT_DIRECTORY_RENAME.capabilities
        },
        ..ALL
    };
    for disk in [
        ALL,
        WITHOUT_FSYNC,
        WITHOUT_DIRECTORY_RENAME,
        without_both,
        EAGER_NAMES,
    ] {
        case.run(disk);
    }
}

/// Files `a/xy`, `a/x/1` and `a/x/y/2`, each saved by its own entity; returns the two
/// entities under `a/x`.
fn bound_tree(library: &mut Library<MemFs>) -> Vec<EntityId> {
    save_new(library, "a/xy", b"3");
    vec![
        save_new(library, "a/x/1", b"1"),
        save_new(library, "a/x/y/2", b"2"),
    ]
}

#[test]
fn every_crash_while_undoing_a_tree_move_recovers() {
    let case = Case {
        setup: |library| {
            let entities = bound_tree(library);
            block_on(library.move_tree(&path("a/x"), &path("b/x"))).unwrap();
            entities
        },
        act: |library, _| block_on(library.undo()),
    };
    for disk in [ALL, WITHOUT_FSYNC, WITHOUT_DIRECTORY_RENAME, EAGER_NAMES] {
        case.run(disk);
    }
}

#[test]
fn an_undo_refused_for_one_file_changes_none() {
    let mut a = open(MemFs::new(), A);
    let entities = bound_tree(&mut a);
    block_on(a.move_tree(&path("a/x"), &path("b/x"))).unwrap();
    block_on(a.fs().create_dir_all(&path("a/x/y"))).unwrap();
    block_on(a.fs().create(&path("a/x/y/2"), b"theirs")).unwrap();
    let before = tree(a.fs());
    let facts = Observed::of(&a).facts;

    let refused = block_on(a.undo());
    assert!(
        matches!(&refused, Err(Error::AlreadyExists { path: at }) if *at == path("a/x/y/2")),
        "{refused:?}"
    );
    assert_eq!(tree(a.fs()), before, "a refused undo wrote");
    assert_eq!(Observed::of(&a).facts, facts);
    assert_eq!(a.state().field(entities[0], "path"), Some(&text("b/x/1")));
}

/// The bytes of the file at the entity's `path` hash to its `content`, for every
/// entity bound to a file.
fn bound_files_hold_their_contents(library: &Library<MemFs>) -> Result<(), String> {
    let state = library.state();
    for entity in state.entities() {
        let (Some(Value::Text(at)), Some(Value::Blob(content))) =
            (state.field(entity, "path"), state.field(entity, "content"))
        else {
            continue;
        };
        let held = block_on(library.fs().read(&path(at))).ok();
        if held.as_deref().map(BlobId::of) != Some(*content) {
            return Err(format!("{entity} is bound to {at}, which holds {held:?}"));
        }
    }
    Ok(())
}

#[test]
fn a_tree_move_recovered_around_files_put_in_its_way_never_binds_them() {
    let case = Case {
        setup: bound_tree,
        act: |library, _| block_on(library.move_tree(&path("a/x"), &path("b/x"))),
    };
    let destinations = [path("b/x/1"), path("b/x/y/2")];
    for kind in [ALL, WITHOUT_DIRECTORY_RENAME] {
        let (mut clean, entities) = case.prepared(kind);
        let start = clean.fs().mutations();
        (case.act)(&mut clean, &entities).unwrap();
        for crash in 0..clean.fs().mutations() - start {
            let disk = case.crashed(kind, crash);
            let mut planted = Vec::new();
            for at in &destinations {
                if block_on(disk.metadata(at)).unwrap().is_none() {
                    block_on(disk.create_dir_all(&at.parent().unwrap())).unwrap();
                    block_on(disk.create(at, b"theirs")).unwrap();
                    planted.push(at.clone());
                }
            }
            let library = open(disk.clone(), A);
            let at = format!("{kind:?}, crash {crash}");
            bound_files_hold_their_contents(&library).unwrap_or_else(|f| panic!("{at}: {f}"));
            for planted in &planted {
                assert_eq!(library_files(&disk)[planted], b"theirs", "{at}");
            }
            let finished = library
                .recovered()
                .iter()
                .any(|recovered| recovered.outcome == Outcome::Finished);
            assert!(
                planted.is_empty() || !finished,
                "{at}: {:?}",
                library.recovered()
            );
        }
    }
}

#[test]
fn a_read_only_writer_reports_the_effect_a_crash_interrupted_and_writes_nothing() {
    let case = Case {
        setup: |library| vec![save_new(library, "song", b"old")],
        act: |library, entities| {
            let bytes = b"new".to_vec();
            block_on(library.save(entities[0], &path("song"), bytes, holds(b"old")))
        },
    };
    let (mut clean, entities) = case.prepared(ALL);
    let start = clean.fs().mutations();
    (case.act)(&mut clean, &entities).unwrap();
    let mut reported = 0;
    for crash in 0..clean.fs().mutations() - start {
        let disk = case.crashed(ALL, crash);
        append_unknown(&disk, A);
        let journal = Layout::default().journal(A);
        let journaled = disk.files().keys().any(|at| at.starts_with(&journal));
        let before = tree(&disk);

        let library = open(disk.clone(), A);
        assert!(library.read_only().is_some(), "crash {crash}");
        assert_eq!(tree(&disk), before, "crash {crash}: opening wrote");
        let reports: Vec<_> = library
            .recovered()
            .iter()
            .map(|recovered| (recovered.outcome, recovered.paths.clone()))
            .collect();
        let expected = match journaled {
            true => vec![(Outcome::Pending, vec![path("song")])],
            false => vec![],
        };
        assert_eq!(reports, expected, "crash {crash}");
        reported += usize::from(journaled);
    }
    assert!(reported > 0, "no crash left the save journaled");
}
