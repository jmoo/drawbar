# RFC-0271: Per-writer library store

- Start date: 2026-10-27
- Authors: jmoo
- RFC PR: [jmoo/drawbar#271](https://github.com/jmoo/drawbar/pull/271)
- Tracking issue: none yet

## Summary

drawbar keeps what a library's files cannot say about themselves (tags, where a sound
came from, and in time other metadata and the relations between files) in toshokan, a
store that lives in a hidden directory beside the files. Each writer, meaning each running
drawbar, appends only to a hash-chained log of its own, and every reader merges all the
logs to the same state in whatever order they arrive. No file in the store is written by
two writers, so a library in a synced folder, on a network share, or open in two windows
or tabs needs no lock and no server. The user's files change only through journaled
steps that move the bytes they displace into the writer's own trash. The store replaces
`.drawbar/library.ron`. It is also where per-file metadata goes instead of a sidecar file
per asset, and where relations between files are kept. The proof of concept is
[jmoo/drawbar#226](https://github.com/jmoo/drawbar/pull/226).

## Motivation

Since [#146](https://github.com/jmoo/drawbar/pull/146), a drawbar library is a folder of
real files. What the files cannot say lives in one file, `.drawbar/library.ron`, which
every drawbar reads whole and rewrites whole. The release test pass found that this one
shared file is behind most of the library's failures:

| Issue | What happens |
|---|---|
| [#179](https://github.com/jmoo/drawbar/issues/179) | Two browsers write one folder library at once and erase each other's tags |
| [#205](https://github.com/jmoo/drawbar/issues/205) | Two desktop instances both open a new library writable, and the second strands its edits |
| [#223](https://github.com/jmoo/drawbar/issues/223) | Opening a library rewrites its index, so the last browser to open a folder wins |
| [#203](https://github.com/jmoo/drawbar/issues/203) | An index with a field this build does not know is rewritten without it |
| [#181](https://github.com/jmoo/drawbar/issues/181) | A damaged index leaves the library read-only with no way out |
| [#186](https://github.com/jmoo/drawbar/issues/186) | A read-only tab says it deleted files it did not |
| [#192](https://github.com/jmoo/drawbar/issues/192) | On a nearly full disk, an import's working copy takes the room its save needs |

A lock does not fix the first three. Sync clients ignore locks, a browser tab and the
desktop app on one picked folder have no channel to share one, and two machines with one
Dropbox folder never see each other's processes. Every surveyed application with a shared
database in the user's folder answers this by refusing synced and network locations (see
Prior art). drawbar cannot: its users keep libraries in Dropbox, iCloud Drive and OneDrive,
and use the browser build beside the desktop app.

Sync also goes wrong in ordinary ways for someone who only ever runs one drawbar at a time:
another computer holds stale state, files arrive out of order, a backup restores part of
the folder, a conflicted copy appears. The store has to treat each of these as normal.

The folder can also sit on any volume the user picks: APFS, NTFS, ext4, FAT or exFAT on a
removable drive, a network share, the browser's private storage, or a folder picked in the
browser. They differ in whether a rename can replace a file or be torn by a crash, how names
compare by case and Unicode normalization, and whether files can be renamed at all.

The library is also about to hold more than tags and origins. Unpacking a bundle forgets
which pianos and samples its programs play, so exporting it again leaves them out
([#183](https://github.com/jmoo/drawbar/issues/183)). A piano's or sample's library id is
not stored in its file, so drawbar can only learn it from the instrument or a bundle's
manifest, and must then keep it somewhere. Inferred from specimens; not confirmed on
hardware. The obvious homes for such facts are a sidecar per file or more rows in the shared
index, and both repeat the problems above.

Goals:

- Normal syncing needs no intervention.
- Once a sync settles, every drawbar shows the same library.
- An interrupted or conflicting change leaves every sound file recoverable.
- When recovery is needed, drawbar says what happened without asking the user to understand
  the store.
- drawbar never leaves a library in a state the user cannot recover from. It always opens and
  can be read, and whatever stops drawbar writing comes with a step the user can take that
  lets it write again.
- A library that stays the same size does not get slower or larger with every edit. A heavy
  user writes about 36,000 entries a year; kept whole, they would add about 10 MB of folder
  and 100 ms of open time a year to a library that never changed.

Not goals: live collaboration, and version history. The store keeps only as much history
as stops one drawbar overwriting another's work.

## Guide-level explanation

### What a user sees

- **Every change is one step.** Tagging, renaming, moving, deleting, importing and saving
  each go to the library as one change. Undo and Redo take them back while the library is
  open. Undo is refused, saying why, when another drawbar has since changed the same thing,
  or when the trash no longer holds the bytes it needs. These refusals let go of the step,
  so the next Undo reaches the step before it.
- **Nothing is overwritten in place.** Deleted and overwritten files go to drawbar's trash
  inside the library, which keeps at most 1 GiB and is emptied when drawbar quits. The
  trash keeps the newest changes first, so deleting a file larger than 1 GiB leaves that
  delete with no undo.
- **A synced folder stays whole.** Each drawbar writes only its own files in
  `.drawbar/library/`, so a sync client never has two versions of one file to reconcile.
  Another drawbar's changes appear when the window is focused again, and at once in another
  tab of the same browser.
- **Concurrent edits are kept.** If two machines set an origin differently while offline,
  drawbar shows the latest and keeps both; neither is silently dropped. Tags are sets, so
  if two machines rename one tag differently while offline, both new names appear.
- **Kept apart.** History that cannot be merged yet is listed in a Kept apart sheet rather
  than mixed in: two branches made by a copied app-data folder, part of a history a sync
  replaced with an older version, or changes that arrived before the ones they follow. The
  user takes the changes they want, or dismisses a branch or the changes an older version
  left. Changes waiting on earlier ones cannot be dismissed, and join by themselves when
  those arrive.
- **After a restore from a backup, the folder wins** the next time drawbar looks.
- **Unsaved edits stay on this computer**, in drawbar's own data, and come back after a quit
  or a crash. drawbar saves them every few seconds, so a crash can lose the last few
  seconds. A read-only library holds unsaved edits in memory only, except one that is
  read-only because a change could not finish, which still keeps them, also when it opens
  read-only again.
- **Older libraries are not migrated.** drawbar does not read what earlier versions kept in
  `.drawbar/`: tags in `library.ron`, unsaved edits and rescued slot contents stay on disk,
  unread, and opening such a library says so.
- **An action too large to keep as one step is refused whole**, before anything changes.

### What a contributor sees

toshokan is a workspace crate that knows nothing about Nord. An app declares typed keys,
opens a library as one writer, renders immutable views, and changes the library only by
intents:

```rust
const ORIGIN: Register<Came> = Register::new("origin");
const TAGS: Set<String> = Set::new("tags");

let schema = Schema::of(&[ORIGIN.key(), TAGS.key()])?;
let (mut lib, opened) = Library::open(backend, Layout::new(".drawbar/library")?, &schema, env)?;

let (import, sound) = lib.intent("Import").create(|e| {
    e.save(&path, bytes, Expect::Absent).set(ORIGIN, Came::New);
});
import.add(sound, TAGS, "Sunday".into()).commit()?;

let view = lib.view();
let tags = view.entity(sound).unwrap().members(TAGS);
lib.undo()?;
```

One user action is one intent: the unit of commit, undo and attribution. A refused intent
changes nothing. The UI renders a `View` and sends intents, so it cannot show a change the
store did not accept. A register's value is a `Field`, which is `Unset`, a value, a
`Conflict` with the value to show and all the others, or `Unreadable` when another writer
stored something this build cannot decode, so an app cannot ignore a conflict by accident.

### What it can hold

Anything the app declares as a key, about any file, without a file per fact:

- **Plain metadata.** A register or a set of the app's own JSON values, such as a learned
  library id with where it was learned, a note, or a rating. See "Plain metadata instead of
  per-file sidecars" below.
- **Relations.** A key whose values name other entities, such as the piano and sample a
  program plays, or the programs of a set list. They follow files through renames made
  outside drawbar. See "Relations between files" below.

drawbar's proof of concept declares three keys: the tags on an asset, the library's list of
tag names, and an asset's origin.

## Reference-level explanation

The store's format and protocol are specified in `crates/toshokan/SPEC.md`, its history
rules in `RETENTION.md`, and the protocol's TLA+ model in `spec/`, all on
[#226](https://github.com/jmoo/drawbar/pull/226). This RFC proposes adopting that design and
does not restate it. What follows is the shape of it, with the section of `SPEC.md` that
specifies each part.

### Three kinds of state

| Kind | What | Where | Rules | Losing it costs |
|---|---|---|---|---|
| Shared truth | The user's files; each writer's log, snapshots, pending records, trash and staging | The library folder | One writer per file; changed only by append or by a journaled step; synced | That writer's facts |
| Local truth | drawbar's unsaved edits; toshokan's record of which writer this instance is, and its lock | The app's own data, per install | Never synced | This install's unsaved edits; the next open writes as a new writer |
| Derived | The merged view, bindings of files, file identities | Memory, and one identity cache per install | Rebuildable from the above at any time; never trusted for a write | Time |

### The design in brief

- **One writer per file** (Layout). Everything a writer writes lives under
  `.drawbar/library/writers.2/<writer>/`, and no file sits at the level of the library.
  Readers read every file there by its contents, whatever it is called, so a sync client's
  conflicted copy is read like any other.
- **Hash-chained logs with random ids** (Segments, Entries, Writers). Each line carries the
  hash of the one before it. A copied app-data folder or a restored backup shows up in the
  chain as a fork or a missing head, never as a collision, and the instance that finds one
  continues as a new writer.
- **A merge that shows conflicts** (Merging). Registers keep every concurrent value, sets
  keep concurrent adds, and a delete keeps an entity that has a concurrent edit it did not
  see. Whatever a reader does not understand, it keeps.
- **Bounded history** (Snapshots, `RETENTION.md`). A writer compacts its own log. The store
  keeps only as much history as stops one instance overwriting another's work. At a fixed
  library size, what remains grows only with deleted entities, removed tags, a certificate
  of about 92 bytes per segment its writer ended, and a pair in `settled` of about 70 bytes
  per commit that moved files and per settlement; `RETENTION.md` sizes each term.
- **Derived bindings** (Binding). Which file is an entity's is worked out from paths and
  file identities, with Godot's rule for copies, and is never written at open. The identity
  function is the app's. drawbar reads only the header and the last two bytes of a Nord
  file, because the container's checksum covers the rest. Inferred from specimens; not
  confirmed on hardware.
- **Journaled file effects** (File effects and pending records, Recovery). A save or a
  delete stages, records, and moves the displaced bytes into the writer's trash, so every
  user path holds the old bytes, nothing, or the new bytes. Where moves copy, a path may
  also hold a cut-short copy of its new bytes while a pending record names it. Recovery
  finishes or reports every step. A refused commit changes nothing. A step that fails logs
  nothing and leaves its record open, and the next write or open settles it forward. A step
  the next write finishes joins Undo ahead of that write. If it cannot finish, the library
  turns read-only until it is opened again (File ▸ Open recent library). If what it moves
  is gone, so no retry could finish it, the writer gives it up and goes on as a new one,
  and drawbar asks whether to roll it back or leave it.
- **Plans from what the volume can do** (Durability, Backends). Each backend states whether
  it can sync, append, rename, and rename without replacing, and toshokan plans from that,
  never from a request that failed. Where a crash can tear a rename (FAT, exFAT, NTFS, HFS,
  UDF, any FUSE file system) or the browser cannot rename, a move copies and then removes
  its source under the same pending record. Paths compare under the volume's rules for case
  and Unicode normalization. A folder that cannot append opens read-only.
- **A core with no I/O** (Design, Backends). One state machine, with a blocking driver for
  native and an async driver for the browser. The crash harness, the sync simulator and the
  value-level harness drive the real code.

### Plain metadata instead of per-file sidecars

The usual home for metadata about one file is a sidecar beside it: an XMP file per photo
(darktable, Lightroom), a `.meta` file per asset (Unity), `.import` and `.uid` files
(Godot), a `metadata.opf` per book (Calibre). Sidecars travel with the folder and other
tools can read them. For a drawbar library they cost more than they give:

- They double the files the user sees, and NSM and the instrument know nothing of them.
- A sidecar must move, rename and copy with its file. Unity loses an asset's identity when
  its `.meta` is left behind, and a copied `.meta` duplicates it.
- Each sidecar is a shared mutable file. Two writers editing one sidecar is the
  `library.ron` problem again, once per asset.

In the store, a per-file fact is a key the app declares and a value it writes:

```rust
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Provides { id: u32, instrument: String, learned: Source }

const PROVIDES: Register<Provides> = Register::new("provides");
const NOTE: Register<String> = Register::new("note");

lib.intent("Unpack bundle").set(piano, PROVIDES, provides).commit()?;
```

What this gives that a sidecar does not:

- **No file per asset.** An entity exists only when something is said about its file. A
  library of a few thousand files with tags on a few hundred holds a few hundred entities in
  a handful of log files.
- **Renames and moves outside drawbar keep the facts.** Binding finds the file by identity,
  with no second file to move alongside it.
- **Copies do not duplicate identity.** A copy is a new file with no facts, as in Godot,
  rather than a second claimant to the same id, as in Unity.
- **Concurrent edits merge.** Two machines writing different notes keep both, as a conflict
  the app can show. Two machines writing one sidecar get a sync client's conflicted copy or
  lose one.
- **Typed and versioned.** A value that does not decode as its key's type is shown as
  unreadable and kept, and an older build keeps keys it does not know.
- **Indexed.** `view.find(KEY, &value)`, `view.with(KEY)` and `view.values(KEY)` answer
  exact lookups without opening any file.

What it gives up: other tools cannot read the facts, and copying one file out of the library
leaves its facts behind. Facts derived from the file's own bytes (a program's name,
category, model) are never stored; they are recomputed, so the store holds only what the
file cannot say.

### Relations between files

A relation is a key whose value names another entity:

```rust
const PLAYS: Set<Link> = Set::new("plays");      // a program's pianos and samples
const HOLDS: Set<Link> = Set::new("holds");      // a set list's programs
struct Link { to: EntityId, learned: Source }
```

- **Stable targets.** Entity ids are random and never reused, and binding follows a file
  through renames and moves, so a relation needs no rewriting when the user reorganizes the
  folder outside drawbar. Relations by path break on rename (Obsidian's links, Ableton's
  sets) or require rewriting every referrer (Logseq's page renames).
- **Created with their target.** Being linked is something a file cannot say, so an intent
  that adds a relation to a file with no entity creates that entity in the same step.
- **Merged as sets.** Links added on two machines are both kept; a removal kills only the
  links it saw. A single-target relation uses a register, and two targets set offline show
  as a conflict.
- **Reverse lookups.** Listing every program that plays a piano is what a delete needs to
  warn about and an export needs to collect a bundle's members. `view.find` matches whole
  values, provenance included, so this needs a lookup by target alone.
- **Dangling targets are reported.** Deleting a linked file leaves its relations in place,
  shown as unresolved, so an undo or a restored file reconnects them.
- **Provenance travels with the link.** Each value records how it was learned (the
  instrument, a bundle's manifest, the user), so two sources that disagree are both shown.

For [#183](https://github.com/jmoo/drawbar/issues/183), unpacking a bundle would record
`PLAYS` from its manifest and `PROVIDES` on each piano and sample it can match, and export
would collect members from the store before asking the instrument.

toshokan does not yet treat entity references specially: a link is the app's JSON like any
other value. Indexing links by target, resolving targets in the view and reporting dangling
ones is the work this needs.

### Evidence from the proof of concept

[#226](https://github.com/jmoo/drawbar/pull/226) implements this design and moves drawbar
onto it.

| Check | Result |
|---|---|
| Every crate's tests, in its Nix build with its declared features | Pass |
| I/O-fault sweep: every request of the eight power-loss workloads, in renaming, picked and copying folders and both drivers, made to fail, lose its reply, unplug the backend or land short, then written again or reopened, with the write that settles it faulted too | Every run satisfies the power-loss invariants; a failed commit is fully absent or fully settled, and no writer stays wedged |
| Retention sweep: an oracle that keeps every entry, the retention rules and the implementation, over every history of writers, a clone, registers, sets, deletes, takes and dismissals, every compaction point, delivery orders up to a cap, conflicted copies, restores, dropped file versions and older versions a sync brings back | 6,729,273 histories; the implementation matches the rules exactly, no field shows a conflict the oracle resolves, and every value the oracle shows appears in the merge or an item |
| TLA+ model of the protocol between writers, sync and readers, deep configuration (`Deep`) | No error: 45,766,142 distinct states under symmetry over instances, the whole space |
| TLA+ liveness, deep configuration (`LivenessDeep`) | No error: 11,519,322 distinct states |
| Power loss: every flush point, sampled write subsets between flushes, each single write omitted, eight workloads | ext4, XFS, Btrfs, exFAT: 0 of 32 runs failed, over 14,039 crash states |
| No growth: two histories ten times apart at a fixed library | Equal state, requests and folder outside snapshots; certificates and record pairs within their stated bounds |
| drawbar's hardware suite on a Nord Electro 5 | 9 of 9 at 05c8f5dd; not yet rerun since the keeper's read-only and fault changes |
| drawbar's and toshokan's browser suites, headless Chromium and Firefox | Pass |

At 2,000 entities and three writers, a native open takes about 160 ms after 200,000
entries of history and about 200 ms after 2 million. In a browser at 10,000 files, it takes
about half a second. #226 has the full figures.

toshokan is about 19,000 lines of library code and 26,000 of tests, harnesses and model.
drawbar grows by about 2,900 lines in the areas it changes.

## Drawbacks

- **Size.** A store of 19,000 lines and a format of its own is a large thing for one
  application to own, and a second implementation must follow `SPEC.md` exactly to share a
  library.
- **Opaque to other tools.** Tags and relations live in JSON lines in a hidden directory.
  Nothing but toshokan reads them, and a file copied out of the library leaves its facts
  behind.
- **The hidden root must sync.** Some sync tools skip dot-directories. If one in use does,
  the library's facts stay on one machine.
- **The trash syncs.** A displaced piano is copied to every synced machine until the trash
  is emptied. The 1 GiB cap and emptying at quit bound this.
- **Writers accumulate.** Every cleared browser profile and every copied app-data folder
  leaves a writer behind. Each is bounded by what it wrote, and nothing folds a writer that
  will not write again, so the count only grows.
- **Every open derives everything.** Nothing derived is kept between opens except file
  identities, so a second open costs as much as the first.
- **A narrow race remains** where renames can replace a file, as in every browser and on
  network shares: a file another program makes at a path between drawbar's check and its
  move is replaced, and its bytes leave the folder.
- **An older copy can show as a branch.** A certificate names a segment by its first line
  and a position, so an older, shorter copy of a compacted segment that a sync brings back
  cannot be told from another instance's copy that left the chain there. It shows as a Kept
  apart branch, never merged silently, even when nothing forked.

## Rationale and alternatives

| Option | Why not |
|---|---|
| Keep `library.ron` and add a lock | A lock cannot span a sync client, a picked folder shared by a browser and the desktop, or two machines |
| SQLite in the folder | Every surveyed tool that does this (Calibre, Zotero, Lightroom, Photos) refuses synced and network locations; on the web it needs the JS `sqlite-wasm` build |
| SQLite per install, synced by export | A second copy of the truth per machine, and something must still merge them |
| Metadata written into each file, as DJ and sound-library tools do | Nord files have no place for it, and drawbar keeps them byte-exact for the instrument and Nord Sound Manager |
| A sidecar per file | See "Plain metadata instead of per-file sidecars": doubles the files, must follow renames and copies, and each one is a shared mutable file |
| One shared operation log | Two writers append to one file; sync clients produce conflicted copies of it |
| Per-writer logs plus a shared content-addressed store for displaced bytes | Built in an earlier revision of the proof of concept. Sharing the store needed coordination sync cannot give, and every serious review finding came from it |
| An off-the-shelf CRDT engine (Automerge, Loro) | Binary formats, dynamic schemas, about 1 MB of wasm, and nothing for files, binding or crash-safe file effects |
| The operating system's trash | Absent on the web and on many network and removable volumes, and restorable only while the app remembers where the OS put each item |
| A sync server | drawbar has no server and its users already have a sync tool |

Within the design:

- **Multi-value registers over last-writer-wins.** Last-writer-wins drops one of two offline
  edits without a word. Multi-value registers cost a few hundred lines and no library, and
  drawbar has few single-valued facts, so conflicts are rare.
- **Random ids and hash references.** Writer ids are not unique in practice: restored
  backups, cloned disks, VM snapshots and copied app data all duplicate them. Random entity
  ids and references by hash make a duplicated writer collide on nothing.
- **Derived bindings.** Logging every move a scan finds would make opening write to the
  folder, and every reader that saw the move would log it. Deriving, then pinning at the
  next commit, keeps readers that see the same files in agreement with no writes.
- **No cached view.** A cached merged view held state correctness depended on, and at
  drawbar's library sizes it saved tens of milliseconds.
- **Bounded history.** History exists to stop overwrites, not for version control, so
  undo is per session and a restored folder wins.
- **Sans-IO.** The web is async only and native is simpler blocking; one core with two
  drivers serves both, and lets the crash harness, the sync simulator and the value-level
  harness drive the real code without a file system.

If nothing is done, the issues under Motivation stay open, and every new kind of fact
(relations, learned ids) lands in the same shared file.

## Prior art

### Desktop applications over a folder of real files

Surveyed in October 2026 from each project's documentation and source; claims that could not
be confirmed are left out.

| App | In the folder (travels) | Per machine | Identity | Outside rename | Several writers | Deletes and history |
|---|---|---|---|---|---|---|
| VS Code | `.vscode/` settings | Backups, workspace storage, local history | Path | Shows deleted | Prevented: focuses the open window | OS trash; local history |
| Zed | `.zed/` settings | SQLite: workspace and unsaved buffers | Path | Heuristic within a session | One process; collaboration through its server | OS trash |
| Neovim, Vim | Vim: swap and undo beside the file; Neovim: nothing | Neovim: swap and undo in its state directory | Path; undo checked by content hash | Undo lost | Detected by the swap file, allowed | Persistent undo, by path |
| JetBrains | `.idea/` except `workspace.xml` | Caches, local history | Path | — | Focus and watcher | Local history, 5 working days |
| Obsidian | `.obsidian/`, including `workspace.json` | Metadata cache, recovery snapshots | Path, front matter | Links break | One window per vault | OS trash or `.trash/` |
| Logseq (files) | `logseq/`, `bak/`; block ids in Markdown | Cache | Path and block ids | Page rename rewrites referrers | Many reports of iCloud data loss | `bak/` |
| Calibre | `metadata.db` and a `metadata.opf` per book | — | Database row | — | Refuses: GUI against CLI | In-library trash, a few days |
| Zotero | `zotero.sqlite` and `storage/` | — | Database key | — | Exclusive SQLite lock | In-database trash, 30 days |
| Lightroom Classic | Optional XMP per file | Catalog and previews | Path and file name | Missing; relinked by hand | Lock file; no network volumes | Remove, or OS trash |
| darktable | XMP sidecar per image | `library.db`, thumbnail cache | Path and sidecar | — | Not supported: sync XMPs, never edit on two machines at once | OS trash |
| digiKam | Optional XMP | Databases | Path and a hash of metadata, the first 8 KB and size | Re-identified at the next scan | One client per database | — |
| Apple Photos | Managed bundle | — | Internal | Referenced files lost | No network or cloud locations | — |
| Unity | A `.meta` GUID per asset, committed | `Library/`, rebuildable | GUID in the sidecar | Lost if the `.meta` does not follow; copies duplicate the GUID | Merge `.meta` files by hand | — |
| Godot 4 | `.import` and `.uid` sidecars | `.godot/`, rebuildable | UID with path fallback | Old path gone: a move; both present: the copy gets a new UID | — | — |
| Unreal | Redirectors at old paths | Derived data cache | Path and redirectors | Redirector until fixed up | — | — |
| Apple Music | Central library | Caches | Path | Shown missing | — | — |
| Git | Objects, refs | Reflog | Content hash | — | Merges without a server | Reflog 90 and 30 days; prune grace 2 weeks |
| SolidCraft | One `.sc` design: a ZIP of `document.json`, blobs named by SHA-256, and an optional model cache checked against the document's hash and the app version; a `.bak` beside it | An autosave, its metadata and a lock per open design; a command journal for crash reports; OPFS in the browser | Path; links to other designs kept relative to the linking design's folder | Links survive a folder moved whole; otherwise relinked by hand | Not addressed; the browser design plans one SharedWorker as the only writer of an origin's storage | `.bak` of the previous save; autosave while unsaved |

**Where metadata lives.** Of twenty tools, ten keep their primary store in SQLite, six in
text (XML, JSON, YAML, Markdown), and four in a custom binary format. SQLite is always
single-writer, per machine or behind a lock, and never in a folder two machines write.
Every file meant to travel, committed or synced, is text. Custom binary formats hold caches,
indexes, previews and history. Calibre and darktable pair a database with text sidecars it
can be rebuilt from. SolidCraft keeps a derived model cache inside the design file and uses it only
when it matches the document's hash and the app's version. No surveyed tool keeps its primary store as per-writer append-only logs
in the user's folder; Git's objects and reflogs are the nearest relative.

**Preventing data loss.** Identity and user metadata travel with the folder; unsaved edits,
caches, history and locks stay per machine and can be rebuilt. VS Code, Zed, JetBrains and
Neovim all keep recovery data out of the working folder. Displaced data is expired by
policy: Git's prune grace, Zotero's 30 days, JetBrains' 5 working days. Path-only tools lose
metadata on an outside rename; sidecar tools lose it when the sidecar is separated and
duplicate it when copied. The ones that cope re-identify by content (digiKam) or have a rule
for copies (Godot). SolidCraft keeps links between designs relative to the linking design,
so a folder moved whole keeps them and any other move needs a relink. SolidCraft tells a crashed
session's autosave from a running one's by a lock the live process holds, as this design's
writer pool does ([recovery](https://github.com/bherbruck/solidcraft/blob/main/crates/engine/src/recovery.rs)). Watchers are an accelerator, never the truth: Vim, JetBrains, Unity and Godot recheck
on focus and on save, and inotify limits break VS Code and Obsidian on large trees.

**Read and write contention.** No surveyed tool merges concurrent writers without a server.
Editors prevent a second window or warn (Vim's swap file); libraries lock; darktable says
never to edit on two machines at once; Zed routes collaboration through its server. Every
tool with a database in the folder refuses synced and network locations, because sync
ignores locks. Obsidian survives sync because notes are separate files, and still churns on
its one shared mutable file, `workspace.json`.

What this changed in the design: Godot's copy rule for binding, with a cheap identity the
app supplies and digiKam's content re-identification as the fallback; unsaved edits keyed by
entity and checked by content on restore, as Vim checks undo files, never by a path-derived
workspace id; the watcher as an accelerator, with a rescan on focus and a precondition before
every write; trash retention by policy; presence shown without writing to the folder; and
the local root rebuildable from the folder alone, as Calibre rebuilds from its OPF files.

### Audio production and music libraries

Surveyed in October 2026 from vendor documentation, support articles and, for Mixxx, its
source. 

| App | In the folder (travels) | Per machine | Identity | Outside rename | Several writers | Deletes and history |
|---|---|---|---|---|---|---|
| Ableton Live 12 | Tags in `.xmp` files in an `Ableton Folder Info/` folder inside each tagged folder | `Live Database` (an index, rebuildable); collection labels in `Library.cfg` | Path | Move the whole folder so its Folder Info follows | Not addressed; moving machines means copying Folder Info and `Library.cfg` | None; tags survive a database reset because they are in the XMP files |
| Komplete Kontrol | — | `komplete.db3`; favorites in a separate `favorites.db3` | Path | Rebuilt by deleting the database | Not addressed | Back up favorites by hand before a reset |
| Cubase, Nuendo MediaBay | Optional volume database at the top of a drive; optional tags written into files | A local database | Path | Rescan; tags not yet written are lost if the file changed since the last scan | A volume changed on another system needs a rescan | — |
| REAPER Media Explorer | Optional metadata written into files | Plain-text `.ReaperFileList` databases | Path | Relinked by hand, by find and replace | Not addressed | — |
| Logic Pro | Loop metadata inside each Apple Loop | Loop index | Path, under the Apple Loops folders | Loops moved out may not be found; Reindex All Loops | Not addressed | — |
| Serato DJ | A `_Serato_/` folder at the root of every drive used, with its database and crates; cue points in MP3 tags | `_Serato_/` on the system drive | Path relative to the drive's mount point | Shown missing; Relocate Lost Files searches | Not addressed; users report conflicts when `_Serato_` folders sync | One automatic backup, overwritten every 24 hours and at exit |
| rekordbox | — | `master.db`, SQLite encrypted with SQLCipher | Full path, plus a row UUID | Auto Relocate searches folders the user names | Cloud Library Sync between one person's devices, music moved by Dropbox or Google Drive; "multiple people cannot share the same library" | Manual backup and restore |
| Traktor | Optional tag writing into files | `collection.nml` (XML) | Path | Check Consistency, then Relocate | Not addressed | A timestamped backup at every exit; restore asks whether collection or file tags win |
| Mixxx | Optional sync of metadata into file tags | `mixxxdb.sqlite` | Path; a rescan matches a moved file by file name and duration within a second | Found again on rescan, else listed as missing | Not addressed | Missing tracks keep their cues until purged by hand |
| Sononym | `sononym.db` at the root of each library folder, by default | Optionally the same database in the app's own folder | Path | Refresh picks up moves and changes | Needs an exclusive lock; read-only media open read-only; one process writes a library | — |
| ADSR Sample Manager | — | A `.db3` with tags and collections | Path | Folders grey out until a rescan | Moving machines means copying the database with the app closed on both | Backups with restore scripts; deleting the database loses tags |
| Splice | Samples in the Splice folder | Settings | Path | A missing drive at launch makes a new empty folder silently; relinked by hand | A local drive recommended | — |
| Soundly | A shared database file in the sound-effects folder on a NAS; metadata embedded in files | A local database | Path | — | Editing restricted to admins; concurrent writes not addressed | — |
| BaseHead | Burn-In writes database values into the files; unwritten edits are listed | SQLite | Path | Rescan | Its forum advises one database per machine, never one SQLite file on a NAS; a paid add-on relays edits through one host and queues offline ones | — |

What the audio field does that the others do not:

- **Metadata goes into the files themselves.** Serato, Traktor, REAPER, Cubase, BaseHead and
  Soundly write tags, cue points or database fields into ID3, RIFF or XMP chunks, treating the
  file as the portable record and the database as a cache, and track which edits are not
  written yet. Nord files have no place for user metadata, and drawbar keeps them byte-exact
  for the instrument and Nord Sound Manager, so this route is closed here.
- **A database per drive or per library folder.** Serato's `_Serato_/`, Cubase's volume
  databases and Sononym's `sononym.db` live at the root of what they index, with paths
  relative to it, so a drive works on any machine. These are the nearest relatives of a store
  in the library folder, and each is a single-writer file.
- **Several writers are refused or sold as a service.** Sononym needs an exclusive lock;
  BaseHead's own forum warns against one SQLite file on a NAS; rekordbox syncs one person's
  devices through its own cloud and refuses shared libraries; BaseHead relays edits through a
  host. None says its live database is safe in a synced folder, and users report Serato's
  conflicting when it is.
- **Moved files are found by search, not identity.** rekordbox and Serato search folders the
  user names; Mixxx matches file name and duration. Nobody uses a content hash. Mixxx keeps a
  missing track's cues, so they return with the file.
- **Backups, not undo.** Traktor keeps a copy at every exit and Serato one overwritten copy.
  No surveyed app documents undo for library edits.

Sources:
[Ableton tags](https://help.ableton.com/hc/en-us/articles/11425042663708),
[Ableton collections](https://help.ableton.com/hc/en-us/articles/360000268570),
[Ableton database reset](https://help.ableton.com/hc/en-us/articles/360000794970),
[Komplete Kontrol](https://support.native-instruments.com/support/solutions/articles/69000879643-my-komplete-kontrol-browser-is-empty-or-incomplete),
[Cubase volume databases](https://archive.steinberg.help/nuendo/v12/en/cubase_nuendo/topics/mediabay/mediabay_volume_databases_working_with_c.html),
[Cubase write-protected files](https://steinberg.help/nuendo/v12/en/cubase_nuendo/topics/mediabay/mediabay_attributes_of_write_protected_files_editing_c.html),
[REAPER 6.16](https://reaper.blog/2020/11/616_update),
[REAPER databases](https://reaper.blog/2022/10/how-to-fix-reaper-media-databases),
[Logic loops](https://support.apple.com/en-us/101873),
[Serato database](https://support.serato.com/hc/en-us/articles/204194250-What-is-the-DatabaseV2-file),
[Serato backup](https://support.serato.com/hc/en-us/articles/227561667-Library-Auto-Backup),
[Serato relocate](https://support.serato.com/hc/en-us/articles/14359604291599-Relocate-Lost-Files),
[rekordbox library sync](https://rekordbox.com/en/support/faq/library-sync-7/),
[rekordbox database format](https://pyrekordbox.readthedocs.io/en/stable/formats/db6.html),
[Traktor backups](https://support.native-instruments.com/support/solutions/articles/69000879494-how-to-restore-the-traktor-track-collection-from-a-backup),
[Mixxx library](https://manual.mixxx.org/2.5/en/chapters/library),
[Mixxx moved tracks](https://github.com/mixxxdj/mixxx/blob/main/src/library/dao/trackdao.cpp),
[Sononym libraries](https://sononym.net/docs/manual/libraries/),
[ADSR moving](https://adsr.ladesk.com/851603-How-to-Move-Sample-Manager-to-a-New-Computer),
[ADSR restore](https://adsr.tawk.help/article/restore-adsr-sample-manager-database-from-backup),
[Splice external drives](https://support.splice.com/en/articles/8652664-can-i-put-my-splice-folder-on-an-external-hard-drive),
[Soundly shared database](https://getsoundly.com/faq/how-do-i-set-up-a-shared-network-database/),
[BaseHead burn-in](https://baseheadinc.com/kb/metadata-import-export-burn-in/),
[BaseHead DSS](https://baseheadinc.com/dss/),
[BaseHead on a NAS](https://baseheadinc.com/community/basehead-mac-specific/synology-ds918-sql/).

### Per-writer logs and replicated metadata

Systems that share one part of the design: facts merged into a rebuildable view, per-writer
logs, or concurrent writers on a synced file system. None combines all three with a sync
client as the only transport.

| System | Close in | Differs in |
|---|---|---|
| [Perkeep](https://perkeep.org/doc/schema/permanode) | Permanodes are anchors changed by signed attribute claims; state is the claims replayed; the index is disposable | A content-addressed blob store, not a folder of real files |
| [git-annex metadata](https://git-annex.branchable.com/internals/) | Tags as a log of adds and removes, merged by concatenation; removals are kept so a late add does not resurrect | Keyed by content, not an entity bound to a path; travels by Git |
| [Core Data with iCloud](https://developer.apple.com/library/archive/documentation/DataManagement/Conceptual/UsingCoreDataWithiCloudPG/Introduction/Introduction.html) (deprecated 2016) | Per-device transaction logs in a synced folder, imported into a private local store | [Widely judged flawed](https://mjtsai.com/blog/2016/06/17/the-deprecation-of-icloud-core-data/): a compaction baseline that arrived before or without its logs stopped sync for good |
| [Hypercore and Autobase](https://docs.pears.com/building-blocks/autobase) | Per-writer logs with causal references; a deterministic reducer rebuilds the view | Linearizes, then undoes and reapplies when new causal information reorders entries |
| Secure Scuttlebutt | One signed, hash-chained feed per identity | [One key on two devices forks the feed](https://weblog.masukomi.org/2017/12/19/using-1-scuttlebutt-identity-on-multiple-devices/) and peers reject it, with no repair |
| [Earthstar](https://deno.land/x/earthstar@v9.3.3/mod.browser.ts?s=Replica) | Documents per author and path; latest at a path is a query over every author | A sync bug [picked the wrong latest and resurrected deleted documents](https://deno.land/x/earthstar@v9.3.1/CHANGELOG.md) |
| [Jujutsu operation log](https://docs.jj-vcs.dev/latest/technical/concurrency/) | Lock-free, documented as usable on Dropbox, rsync and NFS; several heads are merged by readers | One repository history, not per-writer logs |
| [Automerge Repo](https://automerge.org/docs/reference/under-the-hood/storage/) | Change files named by hash, so writers never write one file | Two tabs compacting one store leave two snapshots |
| [Fossil](https://www.fossil-scm.org/index.html/doc/3580fce8/src/tag.c) | Tags as their own immutable records beside the object | A repository database, not files |

What this changed in the design: Core Data with iCloud is the cautionary twin, so a missing
snapshot or segment is a gap that waits, never a halt, and the sync simulator delivers
snapshots before their segments and segments without their snapshots. git-annex and
Earthstar both had to keep removals so an old add merged late does not resurrect; sets here
keep the same property. Autobase reorders and reapplies; this merge is order-free, so a
reader absorbs late entries without undoing anything. Scuttlebutt rejects a forked feed;
this design detects the fork, keeps both branches and has the forked instance start a new
writer.

### Earlier decisions here

The design went through four revisions before this RFC process existed: one shared
operation log; per-writer logs with a shared content-addressed store, built as the first
version of the proof of concept; a revision without durable file undo; and the per-writer
trash described here, later simplified to drop the cached view, the index layer and
unbounded history.

## Unresolved questions

- **Dot-directory sync.** Do Dropbox, iCloud Drive, OneDrive, Google Drive and Syncthing all
  carry `.drawbar/`? If any skips it, the root needs a visible name.
- **References as a value kind.** Whether toshokan gains a reference type that the view
  indexes by target, resolves, and reports when dangling, or the app resolves links itself
  and scans for referrers.
- **Conflict display per field kind.** How drawbar shows a register conflict, a deletion
  conflict and a conflicted file, beyond showing the latest value.
- **A downgraded build writing.** Whether an older drawbar may keep writing when its own
  earlier entries carry members it does not know. They survive compaction, so the
  recommendation is yes.

## Future possibilities

- **Learned ids and bundle relations** ([#183](https://github.com/jmoo/drawbar/issues/183),
  [#184](https://github.com/jmoo/drawbar/issues/184)): `PROVIDES` and `PLAYS` as described
  above, learned from the instrument and from bundle manifests, so offline export carries
  every member.
- **Set list relations**, from set lists read off the instrument.
- **User metadata**: notes, ratings and colors on tags, each one key.
- **Export of facts** into a bundle or a sidecar on request, for tools that cannot read the
  store.
- **A name search index** fed by the per-entity changes each commit and refresh return.
- **Presence**: "also open on MacBook, 2 minutes ago", from writers' last entries.
- **Other applications** that keep metadata beside a synced folder of real files; toshokan
  knows nothing about Nord.
