//! An instance of an app writing and reading through the log, for tests.

#![allow(dead_code, reason = "each test file uses part of the harness")]

use std::collections::BTreeSet;

use toshokan::blocking::run;
use toshokan::compaction::compact;
use toshokan::env::SeededRandom;
use toshokan::log::{Entry, EntryKind, Logged};
use toshokan::reader::{CachedView, Reader};
use toshokan::report::{Compacted, Start};
use toshokan::simulator::Machine;
use toshokan::writer::{Claimed, Writer};
use toshokan::{EntryHash, Hlc, Layout, MemDisk, Nonce, Random, Result, SegmentName, WriterId};

pub fn layout() -> Layout {
    Layout::new(".lib").unwrap()
}

pub fn machine() -> Machine {
    Machine {
        folder: MemDisk::new(),
        local: MemDisk::new(),
    }
}

/// One running instance: it reads the folder, claims a writer, and creates one at
/// its first write when the pool gave none.
pub struct Instance {
    pub machine: Machine,
    pub reader: Reader,
    pub writer: Option<Writer>,
    pub start: Start,
    random: SeededRandom,
    clock: u64,
}

impl Instance {
    pub fn open(mut machine: Machine, seed: u64) -> Self {
        let mut reader = Reader::new(layout(), CachedView::default());
        run(&mut machine, reader.read()).unwrap();
        let Claimed { writer, start } =
            run(&mut machine, Writer::claim(&layout(), reader.logs())).unwrap();
        Self {
            machine,
            reader,
            writer,
            start,
            random: SeededRandom::new(seed),
            clock: 0,
        }
    }

    pub fn id(&self) -> WriterId {
        self.writer.as_ref().expect("a writer").id()
    }

    fn tick(&mut self) -> Hlc {
        self.clock += 1;
        Hlc {
            wall_ms: self.clock,
            counter: 0,
        }
    }

    /// Appends one intent, creating the writer first when there is none. Returns
    /// every entry it made durable, the genesis entry included.
    pub fn write(&mut self, label: &str) -> Result<Vec<EntryHash>> {
        let mut made = Vec::new();
        if self.writer.is_none() {
            let id = WriterId::from_u128(self.random.next_u128());
            let segment = SegmentName::from_u128(self.random.next_u128());
            let at = self.tick();
            let writer = run(
                &mut self.machine,
                Writer::create(layout(), id, segment, "instance".into(), at),
            )?;
            made.push(writer.genesis());
            self.writer = Some(writer);
        }
        let kind = EntryKind::Intent(Logged {
            label: label.into(),
            ops: Vec::new(),
            displaced: Vec::new(),
            reverses: None,
        });
        let at = self.tick();
        let fresh = SegmentName::from_u128(self.random.next_u128());
        let writer = self.writer.as_mut().expect("created above");
        let entries = run(&mut self.machine, writer.append(vec![(at, kind)], fresh))?;
        made.extend(entries.iter().map(Entry::hash));
        Ok(made)
    }

    pub fn compact(&mut self) -> Result<Compacted> {
        let name = Nonce::from_u128(self.random.next_u128());
        let writer = self.writer.as_mut().expect("a writer");
        run(&mut self.machine, self.reader.read_writer(writer.id()))?;
        let own = &self.reader.logs()[&writer.id()];
        run(&mut self.machine, compact(writer, own, name))
    }

    pub fn close(mut self) -> Machine {
        if let Some(writer) = &mut self.writer {
            run(&mut self.machine, writer.close()).unwrap();
        }
        self.machine
    }

    pub fn crash(mut self) -> Machine {
        self.machine.crash();
        self.machine
    }
}

/// What a reader without a cached view places from `folder`.
pub fn fresh(folder: &MemDisk) -> CachedView {
    let mut disk = folder.clone();
    let mut reader = Reader::new(layout(), CachedView::default());
    run(&mut disk, reader.read()).unwrap();
    reader.cached().clone()
}

/// Every entry `view` holds, of every writer.
pub fn held(view: &CachedView) -> BTreeSet<EntryHash> {
    view.writers()
        .values()
        .flat_map(|log| {
            let folded = log.snapshots().iter().flat_map(|s| s.folded.clone());
            let placed = log.entries().iter().map(Entry::hash);
            folded.chain(placed).collect::<Vec<_>>()
        })
        .collect()
}

/// The files under `dir` in the folder of `disk`.
pub fn files_under(disk: &MemDisk, dir: &toshokan::RelPath) -> Vec<toshokan::RelPath> {
    disk.files(toshokan::Root::Folder)
        .into_keys()
        .filter(|path| path.starts_with(dir))
        .collect()
}
