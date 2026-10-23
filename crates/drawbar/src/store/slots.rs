//! The unsaved edits of slot views, kept in this install's own state for the library,
//! one file per slot: the hash of the bytes the view was opened over, on a line of its
//! own, then the bytes it holds.

use std::io;

use toshokan::asynch::Fs;
use toshokan::io::Root;
use toshokan::RelPath;

use super::storage;
use super::Slot;

/// The folder of this install's state for the library that holds them.
const SLOTS: &str = "slots";

/// Where the edit of `slot` is kept in the local root.
fn path(slot: &Slot) -> RelPath {
    let instrument = blake3::hash(slot.instrument.as_bytes());
    let instrument: String = instrument.as_bytes()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let name = format!("{instrument}-{}-{}-{}", slot.class, slot.bank, slot.slot);
    RelPath::new(&format!("{SLOTS}/{name}")).expect("a slot's name is one component")
}

/// Keep `bytes` as the edit of `slot`, made over the slot's bytes of hash `base`. A
/// crash leaves the edit kept before, or this one, whole.
pub async fn keep<F: Fs>(fs: &F, slot: &Slot, base: blake3::Hash, bytes: &[u8]) -> io::Result<()> {
    let mut kept = format!("{}\n", base.to_hex()).into_bytes();
    kept.extend_from_slice(bytes);
    storage::replace(fs, Root::Local, &path(slot), kept).await
}

/// The edit kept of `slot`, with the hash of the bytes it was made over; `None` where
/// none is.
pub async fn read<F: Fs>(fs: &F, slot: &Slot) -> io::Result<Option<(blake3::Hash, Vec<u8>)>> {
    let Some(kept) = storage::read_replaced(fs, Root::Local, &path(slot)).await? else {
        return Ok(None);
    };
    let unreadable =
        || io::Error::new(io::ErrorKind::InvalidData, "it is not an edit drawbar kept");
    let end = kept
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(unreadable)?;
    let (line, bytes) = (&kept[..end], &kept[end + 1..]);
    let base = std::str::from_utf8(line)
        .ok()
        .and_then(|hex| blake3::Hash::from_hex(hex).ok())
        .ok_or_else(unreadable)?;
    Ok(Some((base, bytes.to_vec())))
}

/// Let go of the edit kept of `slot`, if one is.
pub async fn discard<F: Fs>(fs: &F, slot: &Slot) -> io::Result<()> {
    storage::remove(fs, Root::Local, &path(slot)).await
}

#[cfg(test)]
mod tests {
    use toshokan::io::Io;
    use toshokan::MemDisk;

    use super::*;

    fn slot(instrument: &str, bank: u32) -> Slot {
        Slot {
            instrument: instrument.to_string(),
            class: 7,
            bank,
            slot: 3,
        }
    }

    fn now<T>(future: impl std::future::Future<Output = T>) -> T {
        nord_usb::block_on(future)
    }

    #[test]
    fn an_edit_kept_of_a_slot_reads_back_until_it_is_discarded() {
        let local = MemDisk::new();
        let base = blake3::hash(b"slot");
        now(keep(&local, &slot("A", 1), base, b"edit\nwith lines")).unwrap();
        let kept = now(read(&local, &slot("A", 1))).unwrap();
        assert_eq!(kept, Some((base, b"edit\nwith lines".to_vec())));
        let other = now(read(&local, &slot("B", 1))).unwrap();
        assert_eq!(other, None, "another instrument");
        let other = now(read(&local, &slot("A", 2))).unwrap();
        assert_eq!(other, None, "another bank");
        now(keep(&local, &slot("A", 1), base, b"again")).unwrap();
        let kept = now(read(&local, &slot("A", 1))).unwrap();
        assert_eq!(kept, Some((base, b"again".to_vec())), "kept over");
        now(discard(&local, &slot("A", 1))).unwrap();
        assert_eq!(now(read(&local, &slot("A", 1))).unwrap(), None);
        now(discard(&local, &slot("A", 1))).unwrap();
    }

    /// One tab's worker over the install's local root, which lets other tabs run
    /// before each of its requests.
    struct Tab(MemDisk);

    impl Fs for Tab {
        fn capabilities(&self, root: Root) -> toshokan::io::Capabilities {
            Fs::capabilities(&self.0, root)
        }

        async fn perform(&self, io: Io) -> toshokan::io::IoResult {
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
    fn two_tabs_keeping_one_slot_at_once_leave_one_edit_whole() {
        let base = blake3::hash(b"slot");
        for lead in 0..24 {
            let disk = MemDisk::new();
            let (one, two) = (Tab(disk.process()), Tab(disk.process()));
            now(keep(&one, &slot("A", 1), base, b"before")).unwrap();
            let (kept_one, kept_two) = interleaved(
                lead,
                keep(&one, &slot("A", 1), base, b"one"),
                keep(&two, &slot("A", 1), base, b"two"),
            );
            assert!(
                kept_one.is_ok() && kept_two.is_ok(),
                "{lead}: {kept_one:?} {kept_two:?}"
            );
            let kept = now(read(&one, &slot("A", 1)))
                .unwrap()
                .map(|(_, bytes)| bytes);
            assert!(
                kept.as_deref() == Some(b"one") || kept.as_deref() == Some(b"two"),
                "{lead}: {kept:?}"
            );
            let (_, seen) = interleaved(
                lead,
                keep(&one, &slot("A", 1), base, b"three"),
                read(&two, &slot("A", 1)),
            );
            let seen = seen.unwrap().map(|(_, bytes)| bytes);
            assert!(
                seen.is_some_and(|seen| seen != b"before"),
                "{lead}: {kept:?}"
            );
        }
    }

    #[test]
    fn a_slot_file_without_its_hash_does_not_read() {
        let local = MemDisk::new();
        let at = path(&slot("A", 1));
        let dir = at.parent().unwrap();
        local
            .perform(Io::MakeDir {
                root: Root::Local,
                path: dir,
            })
            .unwrap();
        let bytes = b"not a hash\nbytes".to_vec();
        let root = Root::Local;
        local
            .perform(Io::Create {
                root,
                path: at,
                bytes,
            })
            .unwrap();
        assert!(now(read(&local, &slot("A", 1))).is_err());
    }
}
