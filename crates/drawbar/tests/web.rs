//! The browser's library, kept by its worker: drawbar's commands and answers through the
//! page's backend over the origin private file system, and over a folder taken as one
//! the user picked, and two installs of one library through their workers.

#![cfg(target_arch = "wasm32")]

use std::io::Cursor;
use std::sync::Arc;

use drawbar::store::shown::Shown;
use drawbar::store::{
    Backend, Cmd, Event, Failure, Found, Intent, Key, LibPath, Moving, Picked, Root, Slot, Source,
    Worker,
};
use eframe::egui;
use js_sys::{Array, Function, Promise, Reflect, Uint8Array};
use nord_format::formats::nsmp;
use toshokan::web::{private_dir, Folder, Hint, Hints};
use toshokan::{Expect, RelPath};
use wasm_bindgen::JsCast as _;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_sys::FileSystemDirectoryHandle;

wasm_bindgen_test_configure!(run_in_browser);

/// A sample instrument drawbar rests in its file, made by drawbar's own demo.
const SAMPLE: &[u8] = include_bytes!("../../nord-format/tests/fixtures/demo/drawbar-pad.nsmp");

fn path(text: &str) -> LibPath {
    LibPath::parse(text).unwrap()
}

async fn sleep(ms: i32) {
    let promise = Promise::new(&mut |resolve, _| {
        let global = js_sys::global();
        let set: Function = Reflect::get(&global, &"setTimeout".into())
            .unwrap()
            .unchecked_into();
        set.call2(&global, &resolve, &ms.into()).unwrap();
    });
    JsFuture::from(promise).await.unwrap();
}

/// Empty `dir`, as a fresh browser profile holds it.
async fn wipe(dir: &FileSystemDirectoryHandle) {
    let names = Array::new();
    let entries = dir.entries();
    loop {
        let step = JsFuture::from(entries.next().unwrap()).await.unwrap();
        if Reflect::get(&step, &"done".into()).unwrap().is_truthy() {
            break;
        }
        let pair: Array = Reflect::get(&step, &"value".into())
            .unwrap()
            .unchecked_into();
        names.push(&pair.get(0));
    }
    let remove: Function = Reflect::get(dir, &"removeEntry".into())
        .unwrap()
        .unchecked_into();
    let options = js_sys::Object::new();
    Reflect::set(&options, &"recursive".into(), &true.into()).unwrap();
    for name in names.iter() {
        let removed: Promise = remove.call2(dir, &name, &options).unwrap().unchecked_into();
        JsFuture::from(removed).await.unwrap();
    }
}

async fn opfs() -> FileSystemDirectoryHandle {
    private_dir(&RelPath::ROOT).await.unwrap()
}

/// A file the browser hands the page, as a drop or a picker does.
fn file(name: &str, bytes: &[u8]) -> web_sys::File {
    let parts = Array::of1(&Uint8Array::from(bytes));
    web_sys::File::new_with_u8_array_sequence(&parts, name).unwrap()
}

/// A slot no command keeps an edit of, whose read marks every command before it answered.
fn fence() -> Slot {
    Slot {
        instrument: "fence".into(),
        class: 0,
        bank: 0,
        slot: 0,
    }
}

/// One library open in this tab, as the app's store sees it.
struct Tab {
    backend: Backend,
    shown: Shown,
}

impl Tab {
    async fn open(root: Root) -> (Tab, Event) {
        let backend = Backend::start(&egui::Context::default(), root);
        let mut tab = Tab {
            backend,
            shown: Shown::default(),
        };
        let mut answers = tab.ask(Cmd::Open).await;
        (tab, answers.remove(0))
    }

    /// Run `cmd`, and every answer it gave.
    async fn ask(&mut self, cmd: Cmd) -> Vec<Event> {
        self.backend.send(cmd);
        self.backend.send(Cmd::ReadSlot(fence()));
        let mut answers = Vec::new();
        for _ in 0..30_000 {
            let Some(event) = self.backend.try_recv() else {
                sleep(1).await;
                continue;
            };
            if matches!(&event, Event::SlotKept { slot, .. } if *slot == fence()) {
                return answers;
            }
            let viewed = match &event {
                Event::Opened(Ok(opened)) => Some(&opened.viewed),
                Event::Scanned(Ok(viewed)) => Some(viewed),
                Event::Committed {
                    result: Ok(viewed), ..
                } => Some(viewed),
                _ => None,
            };
            if let Some(viewed) = viewed {
                self.shown.apply(viewed.shown.clone());
            }
            answers.push(event);
        }
        panic!("no answer in 30 s: {answers:?}")
    }

    async fn close(mut self) {
        self.backend.finish();
        // The next library opens once this one let go.
        drop(self.backend);
    }

    fn key_of(&self, at: &str) -> Key {
        let files = self.shown.files.iter();
        let mut found = files.filter(|(_, (path, _))| path.as_str() == at);
        found.next().map(|(key, _)| key.clone()).expect(at)
    }
}

fn found(answers: &[Event]) -> Found {
    let found = answers.iter().find_map(|event| match event {
        Event::Imported {
            result: Ok(found), ..
        }
        | Event::Rewritten {
            result: Ok(found), ..
        } => Some(found.clone()),
        _ => None,
    });
    found.unwrap_or_else(|| panic!("{answers:?}"))
}

fn committed(answers: &[Event]) {
    let made = answers.iter().any(|event| {
        matches!(
            event,
            Event::Committed {
                made: Some(_),
                result: Ok(_),
                ..
            }
        )
    });
    assert!(made, "{answers:?}");
}

async fn read(tab: &mut Tab, at: &str) -> Result<Vec<u8>, Failure> {
    let files = vec![(1, path(at), None)];
    let answers = tab
        .ask(Cmd::Read {
            files,
            room: 1 << 20,
        })
        .await;
    let [Event::Read(read)] = &answers[..] else {
        panic!("{answers:?}")
    };
    let (_, found) = read[0].clone();
    let found = found?;
    if let Some(bytes) = found.bytes {
        return Ok(bytes);
    }
    let resting = found.file.expect("read whole, or left resting");
    let snapshot = resting.snapshot().expect("a resting file has its snapshot");
    let buffer = JsFuture::from(snapshot.array_buffer()).await.unwrap();
    Ok(Uint8Array::new(&buffer).to_vec())
}

/// Open, list, read, tag, save, copy in, rewrite a resting sample by its pieces, move,
/// delete, undo, redo, keep drafts and slot edits, and open again, in `root`.
async fn the_whole_workflow_in(root: Root) {
    let (mut tab, opened) = Tab::open(root.clone()).await;
    let Event::Opened(Ok(opened)) = opened else {
        panic!("{opened:?}")
    };
    assert_eq!(opened.writable, Ok(()));

    let saved = tab
        .ask(Cmd::Intent(Intent::Batch(vec![
            Intent::MakeDir { path: path("Set") },
            Intent::Save {
                id: 1,
                path: path("Set/a.ne5p"),
                bytes: b"a program".to_vec(),
                expect: Expect::Absent,
                came: None,
            },
        ])))
        .await;
    committed(&saved);
    assert_eq!(read(&mut tab, "Set/a.ne5p").await.unwrap(), b"a program");

    let tagged = tab
        .ask(Cmd::Intent(Intent::Tag {
            files: vec![Key::File(path("Set/a.ne5p"))],
            name: "Live".into(),
            make: true,
        }))
        .await;
    committed(&tagged);
    assert_eq!(tab.shown.wearing("Live").len(), 1, "{tagged:?}");
    assert!(tab.shown.names.contains("Live"));

    let copied = tab
        .ask(Cmd::Intent(Intent::Import {
            id: 2,
            path: path("pad.nsmp"),
            from: Source::Outside(file("pad.nsmp", SAMPLE)),
            expect: Expect::Absent,
            came: None,
        }))
        .await;
    committed(&copied);
    let pad = found(&copied);
    let resting = pad.file.clone().expect("a sample rests in its file");
    let index = nsmp::Index::read_from(&mut Cursor::new(SAMPLE)).unwrap();
    let mut outline = index.outline().clone();
    outline.set_name("Vibes").unwrap();
    let pieces = drawbar::rewrite::Rewrite::Sample(outline)
        .pieces(&resting)
        .unwrap();
    let rewritten = tab
        .ask(Cmd::Intent(Intent::Rewrite {
            id: 2,
            path: path("pad.nsmp"),
            pieces: Arc::new(pieces),
            expect: pad.fingerprint().expect(),
        }))
        .await;
    committed(&rewritten);
    let bytes = read(&mut tab, "pad.nsmp").await.unwrap();
    let index = nsmp::Index::read_from(&mut Cursor::new(&bytes)).unwrap();
    assert_eq!(index.outline().name().unwrap(), "Vibes");

    let a = tab.key_of("Set/a.ne5p");
    let moved = tab
        .ask(Cmd::Intent(Intent::Move {
            moves: vec![Moving {
                from: path("Set/a.ne5p"),
                to: path("Set/b.ne5p"),
                expect: Expect::Holds(drawbar_identity("Set/a.ne5p", b"a program")),
            }],
        }))
        .await;
    assert!(
        moved
            .iter()
            .any(|event| matches!(event, Event::Moved { result: Ok(()), .. })),
        "{moved:?}"
    );

    let pad = found(&rewritten);
    let deleted = tab
        .ask(Cmd::Intent(Intent::Delete {
            files: vec![(path("pad.nsmp"), pad.fingerprint().expect())],
        }))
        .await;
    committed(&deleted);
    assert_eq!(read(&mut tab, "pad.nsmp").await, Err(Failure::Moved));
    committed(&tab.ask(Cmd::Undo).await);
    assert!(read(&mut tab, "pad.nsmp").await.is_ok(), "undone");
    committed(&tab.ask(Cmd::Redo).await);
    assert_eq!(
        read(&mut tab, "pad.nsmp").await,
        Err(Failure::Moved),
        "redone"
    );

    let base = drawbar_identity("Set/b.ne5p", b"a program");
    let kept = tab
        .ask(Cmd::Draft {
            of: a.clone(),
            base,
            bytes: b"an unsaved edit".to_vec(),
            replaces: Vec::new(),
        })
        .await;
    assert!(kept.is_empty(), "{kept:?}");
    let slot = Slot {
        instrument: "Electro 5".into(),
        class: 4,
        bank: 1,
        slot: 2,
    };
    let hash = blake3::hash(b"the slot");
    tab.ask(Cmd::KeepSlot {
        slot: slot.clone(),
        base: hash,
        bytes: b"a slot's edit".to_vec(),
    })
    .await;
    let read_slot = tab.ask(Cmd::ReadSlot(slot)).await;
    let [Event::SlotKept {
        kept: Ok(Some((held, bytes))),
        ..
    }] = &read_slot[..]
    else {
        panic!("{read_slot:?}")
    };
    assert_eq!((*held, bytes.as_slice()), (hash, &b"a slot's edit"[..]));
    tab.close().await;

    let (again, opened) = Tab::open(root).await;
    let Event::Opened(Ok(opened)) = opened else {
        panic!("{opened:?}")
    };
    assert_eq!(opened.drafts.len(), 1, "{:?}", opened.drafts);
    let shown: Vec<&str> = again
        .shown
        .files
        .values()
        .map(|(at, _)| at.as_str())
        .collect();
    assert_eq!(shown, ["Set/b.ne5p"], "{shown:?}");
    assert_eq!(again.shown.wearing("Live").len(), 1);
    again.close().await;
}

/// The identity the library takes the bytes of a file at `at` by.
fn drawbar_identity(at: &str, bytes: &[u8]) -> toshokan::Identity {
    drawbar::store::identity_of_bytes(&path(at), bytes).unwrap()
}

#[wasm_bindgen_test]
async fn the_whole_workflow_runs_in_the_private_file_system() {
    wipe(&opfs().await).await;
    the_whole_workflow_in(Root::Private).await;
}

/// A folder of the private file system, opened as a folder the user picked is: through
/// its handle, written by writable streams.
#[wasm_bindgen_test]
async fn the_whole_workflow_runs_in_a_picked_folder() {
    let root = opfs().await;
    wipe(&root).await;
    let handle = private_dir(&RelPath::new("Picked").unwrap()).await.unwrap();
    let picked = Root::Picked(Picked {
        id: 7,
        handle,
        warned: true,
    });
    the_whole_workflow_in(picked).await;
}

/// Two installs keep one library, each through its worker: a refresh shows one what the
/// other committed, and a tab hears of another tab's commit.
#[wasm_bindgen_test]
async fn two_installs_see_each_others_changes() {
    wipe(&opfs().await).await;
    let folder = || Folder::Private(RelPath::new("Shared").unwrap());
    let local = |name: &str| RelPath::new(&format!("local/{name}")).unwrap();
    let mut shown = [Shown::default(), Shown::default()];
    let run = async |worker: &Worker, at: usize, shown: &mut [Shown; 2], cmd: Cmd| {
        let answers = worker.run(cmd).await.unwrap();
        for event in &answers {
            let viewed = match event {
                Event::Opened(Ok(opened)) => Some(&opened.viewed),
                Event::Scanned(Ok(viewed)) => Some(viewed),
                Event::Committed {
                    result: Ok(viewed), ..
                } => Some(viewed),
                _ => None,
            };
            if let Some(viewed) = viewed {
                shown[at].apply(viewed.shown.clone());
            }
        }
        answers
    };
    let first = Worker::start(folder(), &local("first")).await.unwrap();
    let second = Worker::start(folder(), &local("second")).await.unwrap();
    run(&first, 0, &mut shown, Cmd::Open).await;
    run(&second, 1, &mut shown, Cmd::Open).await;
    let (mut heard, told) = (
        Hints::new("Shared", None).unwrap(),
        Hints::new("Shared", None).unwrap(),
    );

    let saved = run(
        &first,
        0,
        &mut shown,
        Cmd::Intent(Intent::Save {
            id: 1,
            path: path("c3.ne5p"),
            bytes: b"c3".to_vec(),
            expect: Expect::Absent,
            came: None,
        }),
    )
    .await;
    let tagged = run(
        &first,
        0,
        &mut shown,
        Cmd::Intent(Intent::Tag {
            files: vec![Key::File(path("c3.ne5p"))],
            name: "Gig".into(),
            make: true,
        }),
    )
    .await;
    let made = tagged.iter().find_map(|event| match event {
        Event::Committed { made, .. } => *made,
        _ => None,
    });
    told.announce(made.unwrap_or_else(|| panic!("{saved:?} {tagged:?}")));
    assert_eq!(heard.next().await, Hint::Committed(made.unwrap()));

    run(&second, 1, &mut shown, Cmd::Refresh).await;
    assert_eq!(
        shown[1].wearing("Gig").len(),
        1,
        "the refresh shows the tag"
    );
    first.close().await;
    second.close().await;
}
