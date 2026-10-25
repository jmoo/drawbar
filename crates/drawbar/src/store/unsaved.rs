//! Unsaved edits, kept in this install's own state for the library and never in its
//! folder: an asset's by the key the library knows its file by, and a slot view's by its
//! slot. Losing that state loses them and nothing else.
//!
//! Each keep is a new file in `unsaved/`, written whole under a name `.next` ends, then
//! moved to its own, by the instance that keeps it, and never changed after, so no two
//! windows of the install write one file. Of the files kept of
//! one edit, the one kept at the latest clock is the edit. An instance lets go of the
//! files it kept or read of an edit once it has kept the edit again, or the edit ends.
//!
//! A file is a JSON line, `{"of":…,"base":…,"at":…}`, then the bytes. Its name is
//! `<tag>-<instance>-<count>`: the first 16 bytes of the BLAKE3 hash of `of` as JSON, in
//! hex, then the keeping instance's random id and how many it had kept before.

use std::collections::{BTreeMap, VecDeque};
use std::io;

use serde::{Deserialize, Serialize};
use toshokan::asynch::Fs;
use toshokan::io::{Kind, Root};
use toshokan::{EntryHash, Identity, RelPath};

use super::{storage, Key, Slot};

const DIR: &str = "unsaved";

/// Whose unsaved edit it is.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Of {
    Asset(Key),
    Slot(Slot),
}

/// What an edit was made over: the identity of an asset's file, or the BLAKE3 hash of a
/// slot's bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Base {
    File(Identity),
    Slot(blake3::Hash),
}

/// An unsaved edit as one file keeps it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Unsaved {
    pub of: Of,
    pub base: Base,
    /// The store's clock when it was kept, which orders the files kept of one edit.
    pub at: u64,
    pub bytes: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Header {
    of: Of,
    base: Base,
    at: u64,
}

impl Unsaved {
    fn encode(&self) -> Vec<u8> {
        let header = Header {
            of: self.of.clone(),
            base: self.base,
            at: self.at,
        };
        let mut encoded = serde_json::to_vec(&header).expect("a header serializes");
        encoded.push(b'\n');
        encoded.extend_from_slice(&self.bytes);
        encoded
    }

    fn decode(kept: &[u8]) -> Option<Unsaved> {
        let end = kept.iter().position(|&byte| byte == b'\n')?;
        let header: Header = serde_json::from_slice(&kept[..end]).ok()?;
        Some(Unsaved {
            of: header.of,
            base: header.base,
            at: header.at,
            bytes: kept[end + 1..].to_vec(),
        })
    }
}

/// What the names of the files kept of `of` start with.
fn tag(of: &Of) -> String {
    let said = serde_json::to_vec(of).expect("an edit's owner serializes");
    hex(&blake3::hash(&said).as_bytes()[..16])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn dir() -> RelPath {
    RelPath::new(DIR).expect("one component")
}

fn file_at(name: &str) -> RelPath {
    dir()
        .join(name)
        .expect("a kept file's name is one component")
}

/// What a file of the store kept, as this instance last saw it.
struct Held {
    name: String,
    /// The clock it was kept at, and the hash of its base and bytes; `None` where it
    /// did not read.
    kept: Option<(u64, blake3::Hash)>,
}

/// The hash a keep of `bytes` over `base` is recognized by.
fn digest(base: Base, bytes: &[u8]) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&serde_json::to_vec(&base).expect("a base serializes"));
    hasher.update(bytes);
    hasher.finalize()
}

/// One instance's part in the store: what it keeps, and the files of each edit it kept
/// or read, which are its to let go of.
pub struct Store {
    instance: u128,
    made: u64,
    clock: Box<dyn toshokan::Clock>,
    /// The latest clock this instance kept or read an edit at.
    latest: u64,
    held: BTreeMap<String, Vec<Held>>,
}

/// How many times a read starts again where a file listed was gone before it was read.
const READS: usize = 8;

impl Store {
    /// The part of the instance `instance`, telling the time by `clock`.
    pub fn new(instance: u128, clock: Box<dyn toshokan::Clock>) -> Store {
        Store {
            instance,
            made: 0,
            clock,
            latest: 0,
            held: BTreeMap::new(),
        }
    }

    /// Keep `bytes` as the edit of `of`, made over `base`, then let go of the files of
    /// it this instance kept or read before. A crash leaves this keep, or what was kept
    /// before, whole.
    ///
    /// A keep of what a file read holds keeps that file's clock: it is the same edit,
    /// and an edit made since in another window stays the newer.
    pub async fn keep<F: Fs>(
        &mut self,
        fs: &F,
        of: Of,
        base: Base,
        bytes: Vec<u8>,
    ) -> io::Result<()> {
        let tag = tag(&of);
        let digest = digest(base, &bytes);
        let held = self.held.get(&tag).into_iter().flatten();
        let same = held
            .filter_map(|held| held.kept)
            .filter(|kept| kept.1 == digest);
        let at = match same.map(|(at, _)| at).max() {
            Some(at) => at,
            None => self.tick(),
        };
        let name = format!("{tag}-{:032x}-{}", self.instance, self.made);
        self.made += 1;
        let unsaved = Unsaved {
            of,
            base,
            at,
            bytes,
        };
        storage::create(fs, Root::Local, &file_at(&name), unsaved.encode()).await?;
        let kept = Held {
            name,
            kept: Some((at, digest)),
        };
        let before = self.held.insert(tag.clone(), vec![kept]);
        self.let_go(fs, &tag, before.unwrap_or_default()).await
    }

    /// Let go of every file of the edit of `of` this instance kept or read.
    pub async fn discard<F: Fs>(&mut self, fs: &F, of: &Of) -> io::Result<()> {
        let tag = tag(of);
        let held = self.held.remove(&tag).unwrap_or_default();
        self.let_go(fs, &tag, held).await
    }

    /// Remove each of `files` of the edit `tag` names. Those a failure leaves are still
    /// this instance's to let go of.
    async fn let_go<F: Fs>(&mut self, fs: &F, tag: &str, files: Vec<Held>) -> io::Result<()> {
        let mut files = VecDeque::from(files);
        while let Some(file) = files.front() {
            if let Err(e) = storage::remove(fs, Root::Local, &file_at(&file.name)).await {
                self.held.entry(tag.to_string()).or_default().extend(files);
                return Err(e);
            }
            files.pop_front();
        }
        Ok(())
    }

    /// Every edit kept of `of`, or of every owner where `of` is `None`, each as its file
    /// holds it, or `Err` with the file's name where it does not read. Each file found is
    /// this instance's to let go of once it keeps or ends the edit.
    pub async fn read<F: Fs>(
        &mut self,
        fs: &F,
        of: Option<&Of>,
    ) -> io::Result<Vec<Result<Unsaved, String>>> {
        let wanted = of.map(tag);
        for _ in 0..READS {
            if let Some(found) = self.read_once(fs, wanted.as_deref()).await? {
                return Ok(found);
            }
        }
        Err(io::Error::other("other windows kept changing what is kept"))
    }

    /// What [`Store::read`] reads, or `None` where a file listed was gone before it was
    /// read, as when another window kept the same edit again.
    async fn read_once<F: Fs>(
        &mut self,
        fs: &F,
        wanted: Option<&str>,
    ) -> io::Result<Option<Vec<Result<Unsaved, String>>>> {
        let listed = match storage::list(fs, Root::Local, &dir()).await {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            listed => listed?,
        };
        let mut found = Vec::new();
        for (name, meta) in listed {
            let Some((tag, _)) = name.split_once('-') else {
                continue;
            };
            if meta.kind != Kind::File || wanted.is_some_and(|wanted| wanted != tag) {
                continue;
            }
            let tag = tag.to_string();
            if name.ends_with(storage::STAGED) {
                self.hold(tag, Held { name, kept: None });
                continue;
            }
            let kept = match storage::read_whole(fs, Root::Local, &file_at(&name)).await {
                Ok(Some(kept)) => kept,
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                Ok(None) | Err(_) => return Ok(None),
            };
            let unsaved = Unsaved::decode(&kept).filter(|unsaved| self::tag(&unsaved.of) == tag);
            let Some(unsaved) = unsaved else {
                self.hold(
                    tag,
                    Held {
                        name: name.clone(),
                        kept: None,
                    },
                );
                found.push(Err(name));
                continue;
            };
            self.latest = self.latest.max(unsaved.at);
            let kept = Some((unsaved.at, digest(unsaved.base, &unsaved.bytes)));
            self.hold(tag, Held { name, kept });
            found.push(Ok(unsaved));
        }
        Ok(Some(found))
    }

    fn hold(&mut self, tag: String, file: Held) {
        let held = self.held.entry(tag).or_default();
        held.retain(|held| held.name != file.name);
        held.push(file);
    }

    /// The clock for a keep: past every edit this instance kept or read, so one kept after
    /// another was read is the newer whatever the time says.
    fn tick(&mut self) -> u64 {
        self.latest = self.clock.now_ms().max(self.latest.saturating_add(1));
        self.latest
    }
}

/// The newest of `found` that reads, by the clock it was kept at.
pub fn newest(found: Vec<Result<Unsaved, String>>) -> Option<Unsaved> {
    found.into_iter().flatten().max_by_key(|unsaved| unsaved.at)
}

/// Where this install remembers it told of unsaved edits an earlier drawbar kept.
const TOLD: &str = "notices/earlier-drafts";

/// Whether the local root holds unsaved edits an earlier drawbar kept, in a writer's
/// `drafts/`, the first time this install looks; then `false`, since it remembers it
/// told. They are never read.
pub async fn earlier<F: Fs>(fs: &F) -> io::Result<bool> {
    let told = RelPath::new(TOLD).expect("a fixed path");
    if storage::meta(fs, Root::Local, &told).await?.is_some() {
        return Ok(false);
    }
    let local = storage::list(fs, Root::Local, &RelPath::ROOT).await?;
    let writers = local
        .into_iter()
        .filter(|(name, meta)| meta.kind == Kind::Directory && name.parse::<EntryHash>().is_ok());
    for (name, _) in writers {
        let drafts = RelPath::new(&format!("{name}/drafts")).expect("two components");
        let meta = storage::meta(fs, Root::Local, &drafts).await?;
        if meta.is_some_and(|meta| meta.kind == Kind::Directory) {
            storage::mark(fs, Root::Local, &told).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use toshokan::io::Io;
    use toshokan::MemDisk;

    use super::*;

    /// A clock that always reads the time it holds.
    struct At(u64);

    impl toshokan::Clock for At {
        fn now_ms(&mut self) -> u64 {
            self.0
        }
    }

    fn store(instance: u128, now: u64) -> Store {
        Store::new(instance, Box::new(At(now)))
    }

    fn slot(instrument: &str, bank: u32) -> Of {
        Of::Slot(Slot {
            instrument: instrument.to_string(),
            class: 7,
            bank,
            slot: 3,
        })
    }

    fn asset(path: &str) -> Of {
        Of::Asset(Key::File(super::super::LibPath::parse(path).unwrap()))
    }

    fn base() -> Base {
        Base::Slot(blake3::hash(b"slot"))
    }

    fn now<T>(future: impl std::future::Future<Output = T>) -> T {
        nord_usb::block_on(future)
    }

    /// The bytes of the newest edit of `of` a new instance reads in `fs`.
    fn newest_of<F: Fs>(fs: &F, of: &Of) -> Option<Vec<u8>> {
        let found = now(store(99, 0).read(fs, Some(of))).unwrap();
        newest(found).map(|unsaved| unsaved.bytes)
    }

    /// The names in the store's folder.
    fn names(fs: &MemDisk) -> Vec<String> {
        match now(storage::list(fs, Root::Local, &dir())) {
            Ok(listed) => listed.into_iter().map(|(name, _)| name).collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn an_edit_kept_reads_back_until_it_is_let_go() {
        let local = MemDisk::new();
        let mut one = store(1, 10);
        now(one.keep(&local, slot("A", 1), base(), b"edit\nwith lines".to_vec())).unwrap();
        let found = now(store(2, 0).read(&local, Some(&slot("A", 1)))).unwrap();
        let expected = Unsaved {
            of: slot("A", 1),
            base: base(),
            at: 10,
            bytes: b"edit\nwith lines".to_vec(),
        };
        assert_eq!(found, [Ok(expected)]);
        assert_eq!(newest_of(&local, &slot("B", 1)), None, "another instrument");
        assert_eq!(newest_of(&local, &slot("A", 2)), None, "another bank");
        let file = Base::File(Identity::from_u128(5));
        now(one.keep(&local, asset("a.ne5p"), file, b"draft".to_vec())).unwrap();
        now(one.keep(&local, slot("A", 1), base(), b"again".to_vec())).unwrap();
        assert_eq!(newest_of(&local, &slot("A", 1)).unwrap(), b"again");
        assert_eq!(names(&local).len(), 2, "a keep lets go of the one before");
        now(one.discard(&local, &slot("A", 1))).unwrap();
        assert_eq!(newest_of(&local, &slot("A", 1)), None);
        assert_eq!(newest_of(&local, &asset("a.ne5p")).unwrap(), b"draft");
        now(one.discard(&local, &slot("A", 1))).unwrap();
    }

    #[test]
    fn of_two_instances_edits_of_one_asset_the_later_kept_is_the_newest() {
        let local = MemDisk::new();
        let (mut later, mut earlier) = (store(1, 20), store(2, 10));
        now(later.keep(&local, slot("A", 1), base(), b"later".to_vec())).unwrap();
        now(earlier.keep(&local, slot("A", 1), base(), b"earlier".to_vec())).unwrap();
        assert_eq!(names(&local).len(), 2, "neither lets go of the other's");
        assert_eq!(newest_of(&local, &slot("A", 1)).unwrap(), b"later");
    }

    #[test]
    fn an_edit_kept_after_another_was_read_is_newer_whatever_the_time_says() {
        let local = MemDisk::new();
        now(store(1, 100).keep(&local, slot("A", 1), base(), b"first".to_vec())).unwrap();
        let mut behind = store(2, 50);
        now(behind.read(&local, None)).unwrap();
        now(behind.keep(&local, slot("A", 1), base(), b"second".to_vec())).unwrap();
        let found = now(store(3, 0).read(&local, None)).unwrap();
        let newest = newest(found).unwrap();
        assert_eq!((newest.at, newest.bytes), (101, b"second".to_vec()));
    }

    #[test]
    fn what_another_instance_kept_goes_only_once_this_one_keeps_the_edit() {
        let local = MemDisk::new();
        now(store(1, 10).keep(&local, slot("A", 1), base(), b"theirs".to_vec())).unwrap();
        let mut mine = store(2, 20);
        now(mine.read(&local, Some(&slot("A", 1)))).unwrap();
        let read_only = Local(local.process(), false);
        let refused = now(mine.keep(&read_only, slot("A", 1), base(), b"mine".to_vec()));
        assert!(refused.is_err());
        assert_eq!(
            newest_of(&local, &slot("A", 1)).unwrap(),
            b"theirs",
            "still kept"
        );
        now(mine.keep(&local, slot("A", 1), base(), b"mine".to_vec())).unwrap();
        assert_eq!(names(&local).len(), 1, "{:?}", names(&local));
        assert_eq!(newest_of(&local, &slot("A", 1)).unwrap(), b"mine");
    }

    #[test]
    fn a_keep_lets_go_of_only_the_files_this_instance_read() {
        let local = MemDisk::new();
        let mut theirs = store(1, 10);
        now(theirs.keep(&local, slot("A", 1), base(), b"one".to_vec())).unwrap();
        let mut mine = store(2, 20);
        now(mine.read(&local, Some(&slot("A", 1)))).unwrap();
        theirs = Store {
            clock: Box::new(At(30)),
            ..theirs
        };
        now(theirs.keep(&local, slot("A", 1), base(), b"two".to_vec())).unwrap();
        now(mine.keep(&local, slot("A", 1), base(), b"three".to_vec())).unwrap();
        assert_eq!(names(&local).len(), 2, "the other's newer keep stays");
        assert_eq!(newest_of(&local, &slot("A", 1)).unwrap(), b"two");
    }

    #[test]
    fn a_keep_of_what_was_read_keeps_its_clock() {
        let local = MemDisk::new();
        let mut theirs = store(1, 10);
        now(theirs.keep(&local, slot("A", 1), base(), b"one".to_vec())).unwrap();
        let mut mine = store(2, 50);
        now(mine.read(&local, Some(&slot("A", 1)))).unwrap();
        now(mine.keep(&local, slot("A", 1), base(), b"one".to_vec())).unwrap();
        theirs = Store {
            clock: Box::new(At(20)),
            ..theirs
        };
        now(theirs.keep(&local, slot("A", 1), base(), b"two".to_vec())).unwrap();
        assert_eq!(
            newest_of(&local, &slot("A", 1)).unwrap(),
            b"two",
            "the edit made since in the other window"
        );
    }

    /// One window's storage over the install's local root, which lets other windows run
    /// before each of its requests, or refuses every write where `writes` is false.
    struct Local(MemDisk, bool);

    impl Fs for Local {
        fn capabilities(&self, root: Root) -> toshokan::io::Capabilities {
            Fs::capabilities(&self.0, root)
        }

        async fn perform(&self, io: Io) -> toshokan::io::IoResult {
            if !self.1 && matches!(io, Io::Create { .. }) {
                return Err(toshokan::IoError::Denied);
            }
            let mut yielded = false;
            std::future::poll_fn(|context| {
                if std::mem::replace(&mut yielded, true) {
                    return std::task::Poll::Ready(());
                }
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            })
            .await;
            Fs::perform(&self.0, io).await
        }
    }

    /// Run `first` and `second` a request at a time each, `second` starting once `first`
    /// made `lead` requests, until both are done.
    fn interleaved<A, B>(lead: usize, first: A, second: B) -> (A::Output, B::Output)
    where
        A: std::future::Future,
        B: std::future::Future,
    {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let (mut first, mut second) = (std::pin::pin!(first), std::pin::pin!(second));
        let (mut a, mut b) = (None, None);
        for turn in 0.. {
            if a.is_none() {
                a = ready(first.as_mut().poll(&mut context));
            }
            if b.is_none() && turn >= lead {
                b = ready(second.as_mut().poll(&mut context));
            }
            if let (Some(_), Some(_)) = (&a, &b) {
                break;
            }
        }
        (a.expect("done"), b.expect("done"))
    }

    fn ready<T>(poll: std::task::Poll<T>) -> Option<T> {
        match poll {
            std::task::Poll::Ready(done) => Some(done),
            std::task::Poll::Pending => None,
        }
    }

    #[test]
    fn two_windows_keeping_and_reading_one_edit_at_once_each_find_it_whole() {
        for lead in 0..24 {
            let disk = MemDisk::new();
            let (one, two) = (Local(disk.process(), true), Local(disk.process(), true));
            let (mut first, mut second) = (store(1, 10), store(2, 20));
            now(first.keep(&one, slot("A", 1), base(), b"before".to_vec())).unwrap();
            let (kept_one, kept_two) = interleaved(
                lead,
                first.keep(&one, slot("A", 1), base(), b"one".to_vec()),
                second.keep(&two, slot("A", 1), base(), b"two".to_vec()),
            );
            assert!(
                kept_one.is_ok() && kept_two.is_ok(),
                "{lead}: {kept_one:?} {kept_two:?}"
            );
            assert_eq!(
                newest_of(&one, &slot("A", 1)).as_deref(),
                Some(&b"two"[..]),
                "{lead}: the later clock"
            );
            first.clock = Box::new(At(30));
            let mut third = store(3, 0);
            let (_, seen) = interleaved(
                lead,
                first.keep(&one, slot("A", 1), base(), b"three".to_vec()),
                third.read(&two, Some(&slot("A", 1))),
            );
            let seen = newest(seen.unwrap()).map(|unsaved| unsaved.bytes);
            assert!(
                seen.as_deref() == Some(b"two") || seen.as_deref() == Some(b"three"),
                "{lead}: {seen:?}"
            );
        }
    }

    #[test]
    fn a_file_that_does_not_say_whose_edit_it_is_does_not_read() {
        let local = MemDisk::new();
        now(store(1, 10).keep(&local, slot("A", 1), base(), b"edit".to_vec())).unwrap();
        let named = |count| format!("{}-{:032x}-{count}", tag(&slot("A", 1)), 7);
        let elsewhere = Unsaved {
            of: slot("A", 2),
            base: base(),
            at: 20,
            bytes: b"another slot's".to_vec(),
        };
        let unread = [
            (named(0), b"no header\nbytes".to_vec()),
            (named(1), elsewhere.encode()),
        ];
        for (name, bytes) in unread {
            let root = Root::Local;
            let path = file_at(&name);
            local.perform(Io::Create { root, path, bytes }).unwrap();
        }
        let mut reader = store(2, 0);
        let found = now(reader.read(&local, Some(&slot("A", 1)))).unwrap();
        let unread = found.iter().filter(|found| found.is_err()).count();
        assert_eq!(unread, 2, "{found:?}");
        assert_eq!(newest(found).unwrap().bytes, b"edit");
        now(reader.discard(&local, &slot("A", 1))).unwrap();
        assert_eq!(
            names(&local),
            Vec::<String>::new(),
            "let go of with the edit"
        );
    }
}
