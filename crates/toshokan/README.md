# toshokan

**Keep what a file can't say about itself beside the file, safely, for every
writer.** toshokan (図書館, "library") keeps tags, provenance, relations between
files, unsaved edits and history in a hidden directory beside a folder of real
files. Any number of writers can change it, offline and without coordinating,
and every change can be undone.

> ⚠️ Proof of concept. The on-disk format is unstable, and the crate is not
> published.

## Use

An app opens a library folder as one writer and changes it by intents, each of
which can be undone. The calls are async and need no particular runtime.

```rust
use toshokan::native::NativeFs;
use toshokan::{Layout, Library, Precondition, RelPath, Value, WriterId};

async fn tag_a_new_song(writer: WriterId, bytes: Vec<u8>) -> toshokan::Result<()> {
    let folder = NativeFs::new("/path/to/library");
    let mut library = Library::open(folder, Layout::default(), writer).await?;
    let (song, _) = library.create().await?;
    let path = RelPath::new("Organ/song.txt")?;
    library.save(song, &path, bytes, Precondition::Absent).await?;
    library.add(song, "tags", Value::Text("brass".into())).await?;
    library.undo().await?;
    Ok(())
}
```

Undo restores displaced bytes from a blob store beside the logs. To bound it,
compact to the undo history worth keeping, then collect to a byte budget:
`library.compact(50)`, then `library.collect(budget)`. Collection keeps every blob
an entry since the last compaction names. A save on a full disk gives up the whole
undo history before it refuses.

[SPEC.md](SPEC.md) specifies every file toshokan writes.

## Principles

1. **The folder belongs to the user.** Files keep their names, bytes and places
   unless the app asks. Opening writes nothing. Only the hidden root directory
   belongs to toshokan.
2. **One writer per file.** A writer appends only to its own files and never
   edits or deletes another writer's bytes.
3. **Merge is a pure function.** The same files give the same state, in any
   order, on any machine. No clock, hash order or arrival order decides anything.
4. **Append, don't edit.** Undo and redo are new entries.
5. **Nothing is overwritten.** Displaced bytes go to the blob store. Removing them
   is a garbage-collection decision within a stated budget.
6. **Every instant is recoverable.** After a crash at any point, the next open
   finishes or reports what was interrupted.
7. **Keep what you don't understand.** Entries and fields from a newer writer
   survive verbatim. A writer that cannot represent its own history opens
   read-only, and reports any change a crash interrupted without touching it.
8. **Conflicts are said, not hidden.** Metadata resolves by fixed rules. Byte
   conflicts are reported, and both sides are kept.
9. **Capabilities are declared.** A backend says what it can do, and plans follow
   from that, never from errors.
10. **Plain text on disk,** specified in [SPEC.md](SPEC.md) precisely enough for a
    second implementation.
11. **Small.** The standard library, serde, BLAKE3 and thiserror, plus rustix on
    Linux and Apple systems for a rename that never replaces. No async runtime.

## Scope

Entities and their facts; one append-only log per writer; an order-independent
merge; per-writer compaction; content-addressed blobs with a budgeted garbage
collector; file effects (save, delete, rename, move a tree) with preconditions
and a journal for recovery; binding files to entities across renames and outside
changes; undo and redo; a file-system trait with an in-memory backend that can
simulate crashes, and a native backend.

It does not interpret any file format, merge file bytes, sync over a network
(the folder's own sync carries the logs), encrypt, answer queries beyond
lookups, or draw a UI. It builds for `wasm32-unknown-unknown` without the native
backend, so browser backends can be added.
