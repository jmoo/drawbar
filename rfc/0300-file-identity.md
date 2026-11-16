# RFC-0300: File identity

- Start date: 2026-11-05
- Authors: jmoo
- RFC PR: [jmoo/drawbar#300](https://github.com/jmoo/drawbar/pull/300)
- Tracking issue: none

## Summary

toshokan shows every file of a library as a row with a `FileId`, a number that names
the file from one look at the folder to the next and is never written anywhere. A
row continues by one ordered rule: this instance's own file effect, else its
entity's binding, else the same path. Other writers' logs never move a row; they
only say who changed it and how. drawbar keys each asset by its row's `FileId`
instead of by entity or path, and drops its own record of which file is which.
With it come changes to binding, to how a reader learns of rewrites no length or
time shows, to which undos are refused, and to how a change is attributed to a
writer. The store, its log and its binding are those of RFC-0271.

## Motivation

Under RFC-0271, toshokan knows a tagged file by its entity's binding, derived from
the scan, and a file nothing is said about only by its path. drawbar kept a second
record of which file is which: a key per asset, `Key::Entity` or `Key::File(path)`,
the path of each asset's file, and a copy of the view. It kept that record in step
by replaying what toshokan reported: its own moves, and other writers' `path`,
`tree` and `pin` ops. Where no report covered a change, it guessed by path.

That replay was behind most of the bugs this pull request fixes, and it cannot be
made sound while sync lags. A writer's log and the files it moved reach another
computer in either order:

- **Log first.** drawbar renamed the asset at once and listed a name the folder did
  not hold yet.
- **File first.** The scan found the old name gone and a new file, and drawbar
  reported a deletion and a new file.

The author measured this on the earlier version of this pull request, which kept
the replay: it failed 772 of 800 seeded synced walks of two drawbars, with these
two cases as the failures. Two drawbars on one folder, on one computer or on two
that sync, need to follow each other's changes without those false reports.

## Guide-level explanation

### What a user sees

- **Another drawbar's changes.** Its renames, moves and new files show at the next
  refresh without a word. The activity log names the tagged files it deleted and
  the files whose tags it changed. A file it moved that a sync client has not
  delivered yet shows at its new place once it arrives.
- **Files without tags.** A file with no tags that another drawbar or another
  program renames shows as a new file under its new name. An unsaved edit of the
  old one stays, as an unsaved copy.
- **Saves.** A save over a file that changed since drawbar read it is refused, and
  drawbar asks whose to keep. This includes a rewrite at the same length and
  modification time, which instrument programs of one fixed length make likely.
- **Undo.** An undo is refused for good where another drawbar has since changed what
  it would take back, and Undo goes on to the change before it. An undo blocked by
  a file in the way, or by one a sync client has not delivered, waits: the log
  names the file, and Undo goes through once it is out of the way.
- **Missing and lost.** A file shows missing only while it holds work. Saving it
  writes it anew beside any file that took its name. A tagged file that is gone
  shows as lost until it comes back or the user lets it go.

### What a contributor sees

A view lists rows. A row is a `FileId`, a path, a `RowState` and the entity the
binding binds its file to, if any:

```rust
let before = lib.view();
lib.refresh()?;
let view = lib.view();
for row in view.rows() {
    match row.state {
        RowState::Present { len, modified } => { /* the listing holds it */ }
        RowState::Unknown => { /* under a path no scan can list now */ }
        RowState::Missing => { /* its entity's file is nowhere; logged path */ }
    }
}
let changes = view.changes_since(&before);  // per row: what, who, how
```

A `FileId` means something only to the `Library` value that drew it. An app keeps
its own per-session state by `FileId` and keeps anything that must outlast the
session by entity, or by path for a file nothing is said about. A commit names
the row of each file it wrote or moved in `Committed::files`, so the app never
has to guess which file a save produced.

## Reference-level explanation

The normative statement is in `crates/toshokan/spec/protocol.md` (Merging, Reading,
Binding, Rows, Undo and redo) and `spec/format.md` (Snapshots). This section gives
its shape.

A library is a folder of files and one append-only log per writer. A reader scans
the folder, merges every log, and binds each entity's file fact to a file. From
that it derives rows: each listed file, each entity whose file binding cannot find,
and each path under one the scan cannot list. A row keeps its `FileId` from look to
look only on evidence this reader has: its own effect, the binding, or the same
path. Logs explain changes; they never move a row. drawbar projects rows onto its
workspace by `FileId`.

### Three kinds of state

| Kind | What | Where | Change in this RFC |
|---|---|---|---|
| Shared truth | The user's files; each writer's log and snapshots | The library folder | One optional snapshot member, `binds` |
| Local truth | drawbar's unsaved edits; toshokan's record of which writer this instance is | The app's own data, per install | A draft key for an unsaved copy with no file |
| Derived | The merged view, bindings and file identities; now also rows and `FileId`s, row changes and their attribution, and drawbar's `Ids` and `Shown` | Memory, and one identity cache per install | `FileId`s belong to one `Library` value and are never written |

### The design in brief

- **Rows** (Rows). A row is `Present`, `Unknown` or `Missing`. `Present` rows are the
  listing, by path. `Missing` rows are bounded by the entities, `Unknown` rows by
  what the listing left out. A `FileId` is the `Library` value's random epoch plus
  a counter, so one from another value names no file of this one.
- **One ordered rule.** At each look a row continues by the first that applies:
  1. This instance's own effect moved or wrote its file: the row is where the
     effect left it. A tree move carries the rows of the unbound files it moved.
  2. Its entity is bound: the row is where binding puts the entity's file,
     `Missing` where binding finds none, `Unknown` where binding leaves it unscanned.
     An entity joined into another, or alike to it, shares that one's row.
  3. Its path is listed again, compared under the volume's rules for case and
     Unicode normalization; at another length or time the row has `Changed`.
- **Unknown first.** A row under a path no scan can list is `Unknown` before binding
  looks for its file elsewhere, so a tagged row in an unreadable folder never
  jumps to a copy.
- **Ending.** Every other row ends, and the file at a new path gets a new `FileId`:
  a file no entity is bound to that another program or writer renamed, a copy, a
  move that edited the file. Before a scan of every file ends a row whose path it
  did not find, it lists that path's directory once more, since the walk lists one
  directory at a time and a file can move from one not reached yet into one already
  listed.
- **Merges.** A file never has two `FileId`s. Where rows meet on one file, the row of
  the entity bound to it survives, else the row already there, and the others end
  `Merged { into }`. An untagged copy that becomes an entity's file once the
  original is deleted is the usual case.
- **Folders from the listing.** The scan lists every directory, empty ones included,
  and a view reports directories made and removed. drawbar's own walk of the
  folders after each commit is gone.

### Binding

Three rules of Binding changed so that every reader of the same facts binds alike
and a row has one place to be:

- **File facts equal in path, identity and length are one value**, whatever
  modification time each gives (Merging), since a move that copies gives a file a
  new time. The value keeps every write's reference, and a pin replaces them all.
  Builds before this rule show such a register as a conflict.
- **A conflicted file register** binds to the latest write whose file holds its
  fact, else the latest write; the files the others name are files no fact names.
- **Alike entities.** When several entities' facts name one file that holds none
  of their facts, the lowest gets it and the others are alike: bound to no file,
  not missing, not reported joined, and shown on its row. Facts that have not seen
  another writer's move can name one file for two entities, so only a file holding
  both facts joins them.
- **Pins only for this install's writers.** A commit pins a move binding found only
  for an entity whose file fact a writer of this install wrote: this writer, or one
  whose directory the local root holds. Another install's fact may name a move
  whose file has not arrived here, so that move stays unpinned until its install
  runs, and binding keeps finding it by identity.

### Rewrites no length or time shows

A reader reads a file's identity only where it decides a binding, and reuses one
it read before while the length and time hold. A file rewritten at the same length
and time would keep its old identity. Three rules close the gap where toshokan has
evidence:

- **Another writer's word.** A refresh that reads another writer's `path` or `file`
  op leaving a file at a path, or its snapshot holding a write of an entity's file
  this view did not hold, owes one identity read of that file at the next scan. It
  reports the file in `Refreshed::rewritten`, and drawbar reads the file of that
  asset again.
- **Own commits.** A commit logs each file it puts in place with the identity of the
  staged file, read before it moves, and reports it in `Committed::wrote`. drawbar
  reads such a file again only where it holds another identity for it.
- **Refused preconditions.** A precondition on an identity reads the file at the
  check. When it refuses, the view takes the length, time and identity the check
  read (`Mismatch::listed`), and the next report says that file was rewritten.

The limit, as protocol.md states it: another writer's rewrite at the same length
and time whose file arrives after the scan of the refresh that read its entry
shows the old bytes until the file changes again, or until a commit names the
identity this view held, which is refused and so asks.

### Attribution

`View::changes_since` says, per row, what changed: it appeared, moved, changed,
went, merged, became `Present`, `Unknown` or `Missing`, or gained or lost an entity,
and which directories were made and removed. For a row with an entity before or
after, it says who (`Who::This`, `Who::Writer(n)` into a writer table numbered in
the order first seen, alike in every view of one library, or `Who::Outside` for a
scan with no entry behind it) and, where known, how: moved, trashed, saved, made or
tagged. Who and how come from the merged state, so compaction does not lose them. A
write that was lost because its entry is no longer read, as when a sync client
evicts it, names no writer, nor does its return. A row with no entity carries no
attribution. Attribution only chooses log lines; whether a row keeps its tags
depends only on the merged state.

### Undo and redo

A refusal is either about history or about the folder now:

- **For good.** Another writer changed what the undo would take back since
  (`Refusal::ChangedSince`), the trash no longer holds the bytes it needs, or the
  entity is gone. drawbar lets go of the step, and the next Undo reaches the one
  before.
- **Waiting.** A file is in the way, a folder holds files, or a file the undo needs
  is away. drawbar keeps the step and logs what blocks it.

A file register holds what the intent left while every live write but a binding's
pin names the same path and identity, whatever modification time it gives, since
putting a file back where moves copy gives it a new one. A `bind` entry's pins
never refuse an undo or redo: a pin records where binding found a file, which is
no one's change. To keep that true after compaction, a snapshot gains an optional
member, `binds`, listing the folded `bind` entries whose pins its state still
holds. A snapshot without it, as earlier builds write, names none, and its pins
refuse as an intent's do. Earlier snapshots still read, and an earlier build that
compacts keeps the member as it found it. The fixture state
`one-writer-compacted-binds` covers it.

### drawbar

- **Assets by `FileId`.** `store::Ids` maps each workspace id to one row and back. An
  asset's file is its row's, so a move, a save or another drawbar's change cannot
  hand an asset another file.
- **What is shown.** `store::Shown` holds each row with its path, state, entity, tags
  and origin, and the folders. A row's tags are its entity's, those of entities
  joined into it, and those of entities alike to it. Each row has a class: `File` (a
  file drawbar opens, present or unknown), `Other` (a file it does not open), `Lost`
  (a missing row whose entity wears tags) and `Absent` (any other missing row).
- **Holding work.** An asset holds work when it has an unsaved edit, a place in the
  send queue, the slot it came off, a save not yet written, or a draft waiting to
  be put back. When its row ends, such an asset stays as an unsaved copy at its
  file's last path, and a save writes it as a file of its own beside whatever took
  the name. Any other asset goes with its row.
- **Merges.** Where an asset's row merges into another's, the asset holding work
  keeps the row. Where both hold work, the one with the older edit becomes an
  unsaved copy.
- **Drafts.** An unsaved edit is kept under the key of the unsaved copy it is, else
  its file's entity, else its path; one kept by entity also records its file's path.
  At open each draft resolves to a row: by entity, following joins, including one the
  same scan commits; or by its path, or the path it recorded where its entity has no
  row, where the file there holds the identity the edit was made over. A draft with
  no row, and the older of two for one row, comes back as an unsaved copy.
- **Answers.** A save, import or rewrite answers with the `FileId` of the row it
  wrote. A file a commit wrote but did not name is answered as unheard and read
  again. Where the app lost an answer, the backend carries the rows of the files
  that answer wrote into the next view, and each asset takes its row back quietly.
- **Taken names.** A path an `Absent` or `Lost` row holds counts as taken when drawbar
  names a new or renamed file, since the row's file may still arrive there.
- **Activity log.** It says when another drawbar deleted a tagged or held file, moved
  one whose file has not arrived (unless a file is already at the destination), or
  changed a file's tags; how many tagged files left the folder with no writer behind
  it; and when an asset holding work was kept apart from a file that left. It says
  nothing of another drawbar's renames, moves and new files, of changes to untagged
  files nothing holds work for, or of this drawbar's own changes.

### API changes

toshokan's public API breaks. The changes include:

- `View::rows`, `View::row`, `View::row_at`, `View::row_of` and
  `View::changes_since` replace `View::changed_since` and `View::is_unbound`.
- `Committed::files` and `Committed::wrote` replace `Committed::paths` and
  `Committed::trees`.
- `View::alike` is new: each entity alike to another, with that one.
- `Refreshed::rewritten` and `Mismatch::listed` are new fields.
- `Resumed::Applied` is boxed.

### Evidence

| Check | What it specifies |
|---|---|
| toshokan `tests/library.rs` | Rows, rewrites, refusals and undo, through both drivers, on renaming and copying folders |
| toshokan format suite | Every fixture state round-trips, the new `binds` state included |
| drawbar `store/tests/two_drawbars.rs` | One test per user-visible case above |
| drawbar `store/tests/walk.rs` | Seeded walks of two or three drawbars on one folder or on synced copies |

After each step the walk checks that each drawbar shows its own folder as a fresh
open does; that once sync settles and every drawbar rescans, all agree with each
other and with every folder; that only another drawbar's save, a conflicted copy or
an injected fault makes a drawbar say something changed outside or went wrong; that
an undo or redo another drawbar changed something under is refused once and the
next goes; that no unsaved edit is lost; that the drawbar that acted shows what it
did; and that only a log's writer changes it. CI runs a few seeds per profile. A
sweep of seeds 1–1000 passes in the desktop, browser and copying profiles.

Non-test lines, base to head: `crates/toshokan/src` 23,596 to 26,360 (+2,764);
`crates/drawbar/src/store` 14,198 to 14,926 (+728), of which `store/mirror.rs` 4,781
to 5,237. Tests add 3,999 lines to toshokan's `tests/` and 5,364 to drawbar's store
tests. `Library` grows from 34 fields to 45.

## Drawbacks

- **Size.** One aim was to move identity reasoning out of drawbar. toshokan grew by
  2,764 lines and drawbar's store by 728 more; `mirror.rs` alone grew by 456. The
  app lost its replay but gained merges, unsaved copies, claims and answer recovery.
- **Four changes in one.** Row identity, same-stat rewrite detection, the undo
  refusal rules with their format addition, and attribution could each have landed
  and been reviewed alone.
- **Attribution is heavy for what it buys.** `Shift`, `Since`, the `away` set, the
  ranking of causes, trashed paths and hosted rows exist to choose a few
  activity-log lines.
- **drawbar's per-asset state is spread out.** `ids`, `copies`, `claiming`, `unsaid`
  and the folder list's `missing` each hold part of one asset's story. One state per
  asset would be easier to reason about.
- **Untagged files lose continuity.** A file nothing is said about that another
  drawbar or program renames ends its row. An open or edited one becomes an unsaved
  copy, and the renamed file shows as new.
- **The limits protocol.md states.** An identity is a file's contents, so a different
  file saved with a moved file's bytes is taken for it. A same-length, same-time
  rewrite whose file arrives after the refresh that read its entry shows the old
  bytes until the file changes or a save is tried. Undo refusals for path, tree and
  directory ops see only entries the reader still holds, so after another writer
  compacts, an undo of a folder move carries along files that writer put there;
  nothing is lost or overwritten.

## Rationale and alternatives

| Option | Why not |
|---|---|
| drawbar replays other writers' logged moves to keep its own record | Unsound under sync lag: a log arriving before its files lists a name the folder lacks, and files arriving first read as a deletion and a new file. Failed 772 of 800 seeded synced walks |
| Continue an untagged file onto another file with its identity while drawbar holds work for it | Each fix needed more machinery: an order between instances' claims, arbitration, holds re-sent with every command. It could still move a document onto an old identical copy |
| A `FileId` written to the folder | Opening or scanning would write, which RFC-0271 rules out; it is a second identity beside the binding, and it grows with every untagged file |
| Keep path as drawbar's key and fix each case | Each case of sync lag, outside rename and coarse clock needed its own rule, and those rules were where the bugs were |
| Land the four parts as separate pull requests | Easier to review, and still possible. They were found and fixed together under one walk and were not separated afterward; whether each part passes the walk alone was not measured |
| Do nothing | Two drawbars on one folder misreport each other's changes, and a synced folder makes it worse |

Within the design:

- **Ordered rule over a candidate search.** Own effect, binding, path: each is
  evidence this reader holds now. Nothing waits for late evidence, so nothing grows
  with lag.
- **Logs only attribute.** A row that moved on a log's word would be wrong whenever
  the log and the files arrive apart. Attribution from the merged state survives
  compaction; untagged rows have none.
- **Blocking undo over putting things beside the obstacle.** An undo refused by what
  the folder holds now waits for it to move. Putting the file back elsewhere would
  be more mechanism and more ways to go wrong.
- **`FileId` per `Library` value.** Never written, so derived state stays deletable:
  reopening shows the same files, tags and edits, with only `FileId`s differing.

## Prior art

- **RFC-0271** derives bindings from paths and file identities and pins moves at the
  next commit. This RFC keeps those rules, narrows pinning to this install's
  writers, and gives every file a row on top of them.
- **Godot 4** gives a copy a new id when both paths are present and treats the old
  path's absence as a move. Binding keeps that rule; rows inherit it, so a copy is a
  new row.
- **digiKam** and **Mixxx** re-identify moved files at the next scan, by a content
  hash or by file name and duration. toshokan does this only for tagged files, by
  the app's identity.
- **Editors** such as VS Code know a file by its path and show an outside rename as a
  deletion. toshokan does the same for files nothing is said about.
- **Inode numbers** could give untagged files continuity on one volume. A browser's
  folder access exposes none, and a file synced to another computer gets a new one.

## Unresolved questions

- **A file joined and also moved or changed while this drawbar was closed.** A draft
  kept under the joined entity falls back on the path it recorded, so it comes back
  as an unsaved copy where the file has since moved or changed.
- **Undo's view of other writers' entries.** The check for another writer's later
  entries goes by clock reading, though the intent records which entries it was
  committed over. A tree another writer moved into the tree being moved back is not
  seen. protocol.md states both rules as implemented.
- **A tag added during another computer's edit.** A tag one drawbar adds while
  another computer edits the same file leaves a lost row. Whether the tag should
  follow the edited file is open.
- **"Changed on disk" for a file only read.** Whether another drawbar's save of a
  file this one has only read should say so in the log.
- **Undo after compaction.** After another writer compacts its log, undo refusals
  for path, tree and directory ops cannot see its entries.

## Future possibilities

- **Continuity for held untagged files**, if a rule turns up that cannot move a
  document onto an identical copy.
- **One state per asset in drawbar**, in place of the sets Drawbacks lists.
- **A smaller attribution**, if the activity log needs fewer lines than it has.
