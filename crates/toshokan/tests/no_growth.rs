//! At a constant library size, what the folder keeps, what a reader keeps and what
//! open, commit and refresh ask of the backend stay flat as history grows tenfold.
//! Two parts of a snapshot grow with use, by the bounds "What stays" in
//! `spec/protocol.md` states:
//! certificates, per segment a writer ended, and the record pairs of `settled`,
//! per commit that moved files.
//!
//! This proves that folding and opening stay flat, not the commit path: the
//! generated entries name records by nonce, and no pending-record file is ever
//! written.

mod scale;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::Ordering::Relaxed;

use scale::{Counted, Gen, IoStats, Rng, CURRENT};
use toshokan::blocking::{Backend, Library};
use toshokan::env::{ExactNames, PrefixIdentity, SeededRandom, TestClock};
use toshokan::simulator::Machine;
use toshokan::snapshot::Snapshot;
use toshokan::{Env, Io, Layout, MemDisk, Register, Root, Schema, Set};

const ENTITIES: usize = 10;
const TAGS: usize = 5;
const WRITERS: usize = 3;
const SHORT: usize = 10_000;
const LONG: usize = 10 * SHORT;

/// The most a segment a writer ended adds to its certificates: one certificate,
/// `[[n,"<hash>"],[n,"<hash>"]],`, its numbers of up to seven digits.
const PER_CERTIFICATE: usize = 92;
/// The most a pending record its writer closed adds to `settled`: one pair,
/// `["<writer>","<record>"],`.
const PER_RECORD: usize = 72;
/// Each number a snapshot holds may gain a digit as history grows tenfold.
const PER_NUMBER: usize = 1;
/// What a reader keeps per certificate and record pair, besides the bytes it read.
const KEPT_PER_NAME: usize = 160;

const NAME: Register<String> = Register::new("name");

fn schema() -> Schema {
    let keys = [
        NAME.key(),
        Register::<String>::new("origin").key(),
        Register::<u32>::new("rating").key(),
        Set::<String>::new("tags").key(),
        Set::<String>::new("related").key(),
    ];
    Schema::of(&keys).unwrap()
}

fn layout() -> Layout {
    Layout::new(".tk").unwrap()
}

/// What a library of [`ENTITIES`] keeps and costs after `entries` entries.
#[derive(Debug)]
struct Measured {
    /// The folder's bytes besides snapshots once the library closed, and the numbers
    /// their lines hold.
    besides: usize,
    besides_numbers: usize,
    /// Snapshot bytes besides certificates and `settled`.
    state: usize,
    /// Numbers those bytes hold, each of which may gain a digit.
    numbers: usize,
    /// Own writes, references, kills and takes the snapshots hold.
    held: [usize; 4],
    certificates: usize,
    settled: usize,
    /// Segments the writers ended, each of which a certificate names.
    segments: usize,
    /// Pending records the writers closed, and the pairs `settled` holds.
    records: usize,
    pairs: usize,
    /// Bytes open read, and what the library it opened keeps.
    read: u64,
    kept: isize,
    open: BTreeMap<&'static str, u64>,
    commit: BTreeMap<&'static str, u64>,
    refresh: BTreeMap<&'static str, u64>,
}

/// Requests by kind, roots summed.
fn requests(stats: &IoStats) -> BTreeMap<&'static str, u64> {
    let mut by_kind = BTreeMap::new();
    for ((kind, _), (n, _)) in &stats.0 {
        *by_kind.entry(*kind).or_default() += n;
    }
    by_kind
}

fn measure(entries: usize) -> Measured {
    let folder = MemDisk::new();
    let mut gen = Gen {
        layout: layout(),
        rng: Rng(0x9e37_79b9_7f4a_7c15),
        at: 1_700_000_000_000,
        segment_entries: 500,
        tags: TAGS,
        compacts: true,
        writers: Vec::new(),
        entities: Vec::new(),
        sink: Box::new(Machine {
            folder: folder.clone(),
            local: MemDisk::new(),
        }),
        staged: Some(BTreeMap::new()),
        records: 0,
    };
    for _ in 0..WRITERS {
        gen.add_writer();
    }
    gen.create(ENTITIES, &[]);
    let written: usize = gen.writers.iter().map(|w| w.entries).sum();
    gen.edit(entries - written, None);
    // Each writer closes, which compacts what it wrote since its last snapshot.
    for w in 0..WRITERS {
        if gen.compacts && !gen.writers[w].since.is_empty() {
            gen.compact(w);
        }
    }
    gen.flush_all();
    let snapshots: Vec<Snapshot> = besides_snapshots(&folder)
        .2
        .iter()
        .map(|bytes| Snapshot::decode(bytes).unwrap())
        .collect();
    let mut measured = Measured {
        besides: 0,
        besides_numbers: 0,
        state: 0,
        numbers: 0,
        held: [0; 4],
        certificates: 0,
        settled: 0,
        segments: gen.writers.iter().map(|w| w.segments).sum(),
        records: gen.records,
        pairs: 0,
        read: 0,
        kept: 0,
        open: BTreeMap::new(),
        commit: BTreeMap::new(),
        refresh: BTreeMap::new(),
    };
    for snapshot in &snapshots {
        let certificates = json_len(&snapshot.certified);
        let settled = json_len(&snapshot.state.settled);
        measured.certificates += certificates;
        measured.settled += settled;
        measured.pairs += snapshot.state.settled.len();
        measured.state += snapshot.encode().len() - certificates - settled;
        let whole: serde_json::Value = serde_json::from_slice(&snapshot.encode()).unwrap();
        let certified = serde_json::to_value(&snapshot.certified).unwrap();
        measured.numbers += numbers(&whole) - numbers(&certified);
        let state = &snapshot.state;
        let held = [
            state.own.len(),
            state.refs.len(),
            state.killed.len(),
            state.takes.len(),
        ];
        for (sum, n) in measured.held.iter_mut().zip(held) {
            *sum += n;
        }
    }

    let own = &gen.writers[0];
    let local = MemDisk::new();
    let head = format!(r#"{{"writer":"{}","head":"{}"}}"#, own.id, own.head.hash);
    let mut setup = Machine {
        folder: folder.clone(),
        local: local.clone(),
    };
    setup
        .perform(Io::MakeDir {
            root: Root::Local,
            path: Layout::local(own.genesis),
        })
        .unwrap();
    setup
        .perform(Io::Create {
            root: Root::Local,
            path: Layout::head(own.genesis),
            bytes: head.into_bytes(),
        })
        .unwrap();
    let stats = Rc::new(RefCell::new(IoStats::default()));
    let backend = Counted {
        inner: Box::new(Machine {
            folder: folder.clone(),
            local,
        }),
        stats: Rc::clone(&stats),
    };
    let env = Env {
        clock: Box::new(TestClock::at(gen.at + 1)),
        random: Box::new(SeededRandom::new(7)),
        identify: Rc::new(PrefixIdentity::default()),
        names: Box::new(ExactNames),
        label: "measured".into(),
    };
    // ⚠️ `kept` reads the process-wide allocator, so this file holds one test.
    let before = CURRENT.load(Relaxed);
    let (mut library, _) = Library::open(backend, layout(), &schema(), env).unwrap();
    measured.kept = CURRENT.load(Relaxed) as isize - before as isize;
    measured.read = stats.borrow().bytes(&["Read", "ReadMany"]);
    measured.open = requests(&stats.borrow());

    *stats.borrow_mut() = IoStats::default();
    let target = gen.entities[3].id;
    library
        .intent("Rename")
        .set(target, NAME, "Renamed".into())
        .commit()
        .unwrap();
    measured.commit = requests(&stats.borrow());

    gen.edit(10, Some(1));
    gen.flush(1);
    *stats.borrow_mut() = IoStats::default();
    library.refresh().unwrap();
    measured.refresh = requests(&stats.borrow());
    library.close().unwrap();
    (measured.besides, measured.besides_numbers, _) = besides_snapshots(&folder);
    measured
}

/// The folder's bytes besides snapshots, the numbers their lines hold, and each
/// snapshot's bytes.
fn besides_snapshots(folder: &MemDisk) -> (usize, usize, Vec<Vec<u8>>) {
    let (snapshots, rest): (Vec<_>, Vec<_>) =
        folder
            .files(Root::Folder)
            .into_iter()
            .partition(|(path, _)| {
                path.name()
                    .is_some_and(|name| name.starts_with("snapshot-"))
            });
    let besides = rest.iter().map(|(_, bytes)| bytes.len()).sum();
    let lines = rest
        .iter()
        .flat_map(|(_, bytes)| bytes.split(|&b| b == b'\n'));
    let json = lines.filter_map(|line| line.split(|&b| b == b'\t').next());
    let held = json.filter_map(|json| serde_json::from_slice(json).ok());
    let numbers = held.map(|value: serde_json::Value| numbers(&value)).sum();
    let snapshots = snapshots.into_iter().map(|(_, bytes)| bytes).collect();
    (besides, numbers, snapshots)
}

fn numbers(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Number(_) => 1,
        serde_json::Value::Array(items) => items.iter().map(numbers).sum(),
        serde_json::Value::Object(members) => members.values().map(numbers).sum(),
        _ => 0,
    }
}

fn json_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).unwrap().len()
}

#[test]
fn a_library_of_constant_size_keeps_and_costs_the_same_as_its_history_grows() {
    let short = measure(SHORT);
    let long = measure(LONG);
    eprintln!("{SHORT} entries: {short:#?}\n{LONG} entries: {long:#?}");
    let grew = |at: fn(&Measured) -> usize| at(&long) as isize - at(&short) as isize;
    let segments = long.segments - short.segments;
    let records = long.records - short.records;
    let digits = short.numbers * PER_NUMBER;
    let certified = segments * PER_CERTIFICATE;
    let allowed = (certified + records * PER_RECORD + digits) as isize;
    let besides = grew(|m| m.besides);
    assert!(
        besides <= (short.besides_numbers * PER_NUMBER) as isize,
        "the folder's bytes besides snapshots grew by {besides} B over {} numbers",
        short.besides_numbers
    );
    assert_eq!(
        short.held, long.held,
        "own writes, references, kills and takes"
    );
    let state = grew(|m| m.state);
    assert!(
        state.unsigned_abs() <= digits,
        "the state grew by {state} B over {} numbers",
        short.numbers
    );
    let certificates = grew(|m| m.certificates);
    assert!(
        certificates <= certified as isize,
        "certificates grew by {certificates} B for {segments} more segments"
    );
    for m in [&short, &long] {
        assert_eq!(m.pairs, m.records, "a pair per record closed");
    }
    let settled = grew(|m| m.settled);
    assert!(
        settled <= (records * PER_RECORD) as isize,
        "settled grew by {settled} B for {records} more records"
    );
    let read = long.read as isize - short.read as isize;
    assert!(read <= allowed, "open read {read} B more, of {allowed}");
    let kept = long.kept - short.kept;
    let names = ((segments + records) * KEPT_PER_NAME) as isize;
    assert!(
        kept <= read + names,
        "open kept {kept} B more, having read {read} B more"
    );
    assert_eq!(short.open, long.open, "requests at open");
    assert_eq!(short.commit, long.commit, "requests at commit");
    assert_eq!(short.refresh, long.refresh, "requests at refresh");
}
