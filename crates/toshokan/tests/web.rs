//! The library in a browser: the async driver in a dedicated worker, on an executor
//! of each kind of folder.

#![cfg(all(target_arch = "wasm32", feature = "web"))]

use std::collections::BTreeMap;
use std::rc::Rc;

use toshokan::asynch::Library;
use toshokan::env::{ExactNames, Identify, PrefixIdentity, Random, SeededRandom, TestClock};
use toshokan::io::{Range, CHUNK};
use toshokan::plan::{Piece, Splice};
use toshokan::report::{Mode, Presence, Start};
use toshokan::web::{private_dir, CryptoRandom, Executor, Folder};
use toshokan::{
    EntityId, Env, Expect, Identity, Io, Layout, Opened, Register, RelPath, Reply, Root, Schema,
    Set,
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

const ORIGIN: Register<String> = Register::new("origin");
const TAGS: Set<String> = Set::new("tags");
const IDENTIFY: PrefixIdentity = PrefixIdentity { prefix: 1 << 16 };

fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

fn identity(bytes: &[u8]) -> Identity {
    IDENTIFY.of_bytes(&RelPath::ROOT, bytes)
}

fn schema() -> Schema {
    Schema::of(&[ORIGIN.key(), TAGS.key()]).unwrap()
}

#[derive(Clone, Copy)]
enum Kind {
    Private,
    Picked { rename: bool },
}

/// One test's folder, and the local roots of the installs that open it.
struct Place {
    kind: Kind,
    run: String,
}

type Lib = Library<Executor>;

impl Place {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            run: format!("library/{:032x}", CryptoRandom.next_u128()),
        }
    }

    async fn folder(&self) -> Folder {
        let path = path(&format!("{}/folder", self.run));
        match self.kind {
            Kind::Private => Folder::Private(path),
            Kind::Picked { rename } => Folder::Picked {
                dir: private_dir(&path).await.unwrap(),
                rename,
            },
        }
    }

    /// An instance of the install `install`, writing as `label`.
    async fn open(
        &self,
        install: &str,
        label: &str,
        seed: u64,
        clock: &TestClock,
    ) -> (Lib, Opened) {
        let local = path(&format!("{}/{install}", self.run));
        let executor = Executor::new(self.folder().await, &local).await.unwrap();
        let env = Env {
            clock: Box::new(clock.clone()),
            random: Box::new(SeededRandom::new(seed)),
            identify: Rc::new(IDENTIFY),
            names: Box::new(ExactNames),
            label: label.into(),
        };
        Library::open(executor, Layout::new(".t").unwrap(), &schema(), env)
            .await
            .unwrap()
    }
}

async fn read(library: &Lib, at: &str) -> Option<Vec<u8>> {
    let read = Io::Read {
        root: Root::Folder,
        path: path(at),
        range: Range {
            offset: 0,
            len: u64::MAX,
        },
    };
    match library.fs().perform(read).await {
        Ok(Reply::Bytes(bytes)) => Some(bytes),
        _ => None,
    }
}

/// Creates one entity with a file at `at`.
async fn create(library: &mut Lib, at: &str, bytes: &[u8]) -> EntityId {
    let (intent, _) = library.intent("Import").create(|e| {
        e.save(&path(at), bytes.to_vec(), Expect::Absent)
            .add(TAGS, "new".to_owned());
    });
    intent.commit().await.unwrap().created[0]
}

fn tags(library: &Lib, entity: EntityId) -> Vec<String> {
    library.view().entity(entity).unwrap().members(TAGS).values
}

type Facts = BTreeMap<EntityId, (Option<String>, Vec<String>, Option<RelPath>)>;

fn facts(library: &Lib) -> Facts {
    let view = library.view();
    let facts = view.entities().into_iter().map(|entity| {
        let origin = entity.get(ORIGIN).shown().cloned();
        let file = entity.file().map(|file| file.path);
        (entity.id(), (origin, entity.members(TAGS).values, file))
    });
    facts.collect()
}

async fn two_installs_converge_and_a_new_reader_agrees(kind: Kind) {
    let place = Place::new(kind);
    let clock = TestClock::at(1_000);
    let (mut a, _) = place.open("one", "a", 1, &clock).await;
    let song = create(&mut a, "song.npno", b"song").await;
    let (mut b, opened) = place.open("two", "b", 2, &clock).await;
    assert_eq!(opened.mode, Mode::Writable);
    assert_eq!(tags(&b, song), ["new"]);
    clock.advance(10);
    b.intent("Tag")
        .add(song, TAGS, "live".into())
        .commit()
        .await
        .unwrap();
    a.intent("Tag")
        .add(song, TAGS, "piano".into())
        .commit()
        .await
        .unwrap();
    a.refresh().await.unwrap();
    b.refresh().await.unwrap();
    assert_eq!(tags(&a, song), ["live", "new", "piano"]);
    assert_eq!(facts(&a), facts(&b));
    let (fresh, _) = place.open("three", "c", 3, &clock).await;
    assert_eq!(facts(&fresh), facts(&a), "a new reader agrees");
}

async fn undoing_a_save_restores_the_displaced_bytes(kind: Kind) {
    let place = Place::new(kind);
    let clock = TestClock::at(1_000);
    let (mut a, _) = place.open("one", "a", 1, &clock).await;
    let song = create(&mut a, "song.npno", b"one").await;
    let save = a.intent("Save").save(
        song,
        &path("song.npno"),
        b"two".to_vec(),
        Expect::Holds(identity(b"one")),
    );
    save.commit().await.unwrap();
    assert_eq!(read(&a, "song.npno").await.unwrap(), b"two");
    a.undo().await.unwrap();
    assert_eq!(read(&a, "song.npno").await.unwrap(), b"one");
    a.redo().await.unwrap();
    assert_eq!(read(&a, "song.npno").await.unwrap(), b"two");
}

/// The saved file is filled by several writes, which a picked folder takes
/// through one writable stream.
async fn a_save_streamed_in_chunks_lands_whole(kind: Kind) {
    let place = Place::new(kind);
    let clock = TestClock::at(1_000);
    let (mut a, _) = place.open("one", "a", 1, &clock).await;
    let old: Vec<u8> = (0..CHUNK * 5 / 2).map(|i| (i % 251) as u8).collect();
    let song = create(&mut a, "song.npno", &old).await;
    let splice = Splice {
        from: path("song.npno"),
        pieces: vec![
            Piece::Bytes(b"head".to_vec()),
            Piece::Kept(Range {
                offset: 0,
                len: old.len() as u64,
            }),
            Piece::Bytes(b"tail".to_vec()),
        ],
    };
    let save = a.intent("Save").save_from(
        song,
        &path("song.npno"),
        Box::new(splice),
        Expect::Holds(identity(&old)),
    );
    save.commit().await.unwrap();
    let saved = read(&a, "song.npno").await.unwrap();
    let expected = [&b"head"[..], &old, b"tail"].concat();
    assert_eq!(saved.len(), expected.len());
    assert!(
        saved == expected,
        "the saved file differs from what was spliced"
    );
}

async fn a_rename_keeps_the_tags_and_a_new_reader_finds_the_file(kind: Kind) {
    let place = Place::new(kind);
    let clock = TestClock::at(1_000);
    let (mut a, _) = place.open("one", "a", 1, &clock).await;
    let song = create(&mut a, "song.npno", b"song").await;
    let rename = a.intent("Rename").rename(
        song,
        &path("set/song.npno"),
        Expect::Holds(identity(b"song")),
    );
    rename.commit().await.unwrap();
    assert_eq!(read(&a, "song.npno").await, None);
    assert_eq!(read(&a, "set/song.npno").await.unwrap(), b"song");
    let (fresh, _) = place.open("two", "b", 2, &clock).await;
    let file = fresh.view().entity(song).unwrap().file().unwrap();
    assert_eq!(file.path, path("set/song.npno"));
    assert_eq!(tags(&fresh, song), ["new"]);
}

/// Each instance's executor holds its writer's Web Lock until it closes.
async fn another_instance_of_an_install_is_present_until_it_closes(kind: Kind) {
    let place = Place::new(kind);
    let clock = TestClock::at(1_000);
    let (mut a, _) = place.open("one", "a", 1, &clock).await;
    let song = create(&mut a, "song.npno", b"song").await;
    let (mut b, opened) = place.open("one", "b", 2, &clock).await;
    assert_eq!(opened.start, Start::New, "a's writer is in use");
    b.intent("Tag")
        .add(song, TAGS, "b".into())
        .commit()
        .await
        .unwrap();
    a.refresh().await.unwrap();
    let here = |others: Vec<toshokan::report::WriterInfo>| -> BTreeMap<String, Presence> {
        others.into_iter().map(|w| (w.label, w.here)).collect()
    };
    assert_eq!(
        here(a.others().await.unwrap()),
        BTreeMap::from([
            ("a".into(), Presence::This),
            ("b".into(), Presence::SameMachine)
        ])
    );
    let b_writer = b
        .view()
        .writers()
        .iter()
        .find(|w| w.label == "b")
        .unwrap()
        .writer;
    b.close().await.unwrap();
    assert_eq!(here(a.others().await.unwrap())["b"], Presence::Elsewhere);
    let (_, opened) = place.open("one", "b again", 3, &clock).await;
    assert_eq!(
        opened.start,
        Start::Resumed(b_writer),
        "b's writer is free again"
    );
}

async fn compaction_keeps_what_every_reader_sees(kind: Kind) {
    let place = Place::new(kind);
    let clock = TestClock::at(1_000);
    let (mut a, _) = place.open("one", "a", 1, &clock).await;
    let song = create(&mut a, "song.npno", b"song").await;
    for tag in ["x", "y", "z"] {
        clock.advance(1);
        a.intent("Tag")
            .add(song, TAGS, tag.into())
            .commit()
            .await
            .unwrap();
    }
    let before = facts(&a);
    a.compact().await.unwrap();
    assert_eq!(facts(&a), before);
    let (fresh, _) = place.open("two", "b", 2, &clock).await;
    assert_eq!(facts(&fresh), before);
}

macro_rules! on_each_folder {
    ($($scenario:ident),* $(,)?) => {
        mod private {
            $(#[super::wasm_bindgen_test] async fn $scenario() { super::$scenario(super::Kind::Private).await; })*
        }
        mod picked {
            $(#[super::wasm_bindgen_test] async fn $scenario() { super::$scenario(super::Kind::Picked { rename: true }).await; })*
        }
        mod picked_without_rename {
            $(#[super::wasm_bindgen_test] async fn $scenario() { super::$scenario(super::Kind::Picked { rename: false }).await; })*
        }
    };
}

on_each_folder!(
    two_installs_converge_and_a_new_reader_agrees,
    undoing_a_save_restores_the_displaced_bytes,
    a_save_streamed_in_chunks_lands_whole,
    a_rename_keeps_the_tags_and_a_new_reader_finds_the_file,
    another_instance_of_an_install_is_present_until_it_closes,
    compaction_keeps_what_every_reader_sees,
);

/// Writes `bytes` at `at` in `library`'s folder without logging anything, as
/// another program would.
async fn write_outside(library: &Lib, at: &str, bytes: &[u8]) {
    let file = path(at);
    let made = Io::MakeDir {
        root: Root::Folder,
        path: file.parent().unwrap(),
    };
    library.fs().perform(made).await.unwrap();
    let created = Io::Create {
        root: Root::Folder,
        path: file,
        bytes: bytes.to_vec(),
    };
    library.fs().perform(created).await.unwrap();
}

#[wasm_bindgen_test]
async fn a_refresh_reads_no_library_file_and_a_rescan_reads_them_all() {
    let place = Place::new(Kind::Private);
    let clock = TestClock::at(1_000);
    let (mut a, _) = place.open("one", "a", 1, &clock).await;
    a.rescan().await.unwrap();
    create(&mut a, "song.npno", b"song").await;
    write_outside(&a, "copy.npno", b"song").await;
    a.refresh().await.unwrap();
    assert!(
        a.view().unbound().cloned().collect::<Vec<_>>().is_empty(),
        "a refresh scans no file"
    );
    a.rescan().await.unwrap();
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [path("copy.npno")]
    );
}
