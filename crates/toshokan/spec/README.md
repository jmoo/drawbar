# The portable-root protocol, model-checked

[`Portable.tla`](Portable.tla) specifies how toshokan's writers keep logs in a
synced folder and how every reader merges them. TLC checks it against a sync
layer that delivers each file to each reader independently and in any order.

```sh
nix build .#nord.toshokan-spec       # small bounds, about nine minutes; `nix flake check` runs it
nix build .#nord.toshokan-spec-deep  # larger bounds, several hours
```

This model follows the bounded retention of [RETENTION.md](../RETENTION.md):
snapshots that certify the files their chain was written to instead of listing
every folded hash, and forks shown as runs instead of merged.

To run one config by hand, `nix shell nixpkgs#tlaplus`, then
`tlc -workers auto -config Portable.cfg Portable.tla` in this directory.

## The protocol

Each writer has a random id, which names its directory, and only that writer
writes there. An entry is identified by its hash, which covers the hash of its
predecessor, so a writer's entries form a chain. A writer appends entries to
segments, and a process that closes its segment seals it with a marker. A
writer folds its own chain into a snapshot, and then deletes the snapshot the
new one supersedes and the sealed segments that a snapshot in its directory
folds. A multi-step effect writes a pending record and logs the entry that
closes it, which names the record: the model closes an effect by its own
`logged` flag, and the code closes a record only by an entry naming its nonce,
never by its place in the chain. The record is then spent, and its owner deletes it at any later time,
after beginning other effects or never. Another writer that sees the record of
an effect no entry closes reports it, and with the user's consent closes it in
its own log; only the record's owner deletes it.

Each entry carries its number in its writer's chain. A snapshot lets a reader
chain only from its anchor, and recognize the lines it certifies: those it folds
of every segment its chain was ever written to.

A reader takes every file in a writer's directory, whatever its name. Its tips
are the snapshots no other snapshot certifies. It skips an entry a tip
certifies, places an entry whose predecessor is a tip's anchor or placed, holds
back an entry whose predecessor may still come after the tips, and shows any
other entry as a run. A fork point is an entry with two successors, one a tip
certifies before an entry no tip folds, or the highest entry every tip of a
writer certifies; every entry of that writer numbered above it is a run, so no
two branches are ever merged. Where a writer has several tips, the reader merges
only what they all fold. Folds are ghosts: the model treats a snapshot as the
whole chain it folds, so it checks placement only. What a snapshot's lossy
contribution keeps, where the code's tips part by the hashes they name, and how
references kill are checked by the value-level tests in `tests/retention`. A dismissal, an entry naming runs, takes
them out of view for good: a reader never merges what a dismissal names, and
shows an entry after one as a run. The reader remembers its tips, the entries it
placed after them and its fork points, and after a grace period drops what no
file it sees holds, unless something it sees descends from it.

A copied local root (a restored backup, a cloned disk) makes two instances write
one directory: a fork. A writer's local root exists only once its first entry is
durable, so a copy is taken after it. A folder, or one writer's directory, can be
restored from a backup, and a sync client can keep an earlier version of a file;
each takes away entries no writer deleted. An instance that sees an entry of its
own directory with two successors, whose last entry is no longer in the folder,
or that has lost its local root, continues under a new writer id and never writes
its old directory again.

The protocol needs these rules beyond that. Each is a constant, and a config
that drops it shows the failure it prevents:

| Rule | Constant | Without it |
| --- | --- | --- |
| A snapshot certifies the lines it folds of every segment its chain was written to, and a writer deletes only what a snapshot certifies. | `Certify` | [`Uncertified.cfg`](Uncertified.cfg): a writer cannot delete a segment below its snapshot's anchor, and a reader shows that segment's folded lines as a run. |
| A snapshot carries forward the certificates of the snapshots on its chain. | `Carry` | [`Uncarried.cfg`](Uncarried.cfg): a segment deleted two compactions ago arrives late, and a reader shows its folded lines as a run. |
| A reader tells a gap from a run by the entry's number. | `Numbered` | [`Unnumbered.cfg`](Unnumbered.cfg): an entry whose predecessor has not arrived is shown as a run, then merged once it arrives. |
| A tip that certifies an entry's predecessor, but not the entry, marks a fork. | `CertForks` | [`Anchors.cfg`](Anchors.cfg): once the original has moved two entries past a fork and compacted, only a reader's memory still shows the fork, and a reader without it merges the original's branch. |
| A reader never merges what a dismissal names, and shows an entry after one as a run. | `Quarantine` | [`Unquarantined.cfg`](Unquarantined.cfg): a reader that never saw a fork merges the branch a dismissal it holds names. |
| The grace step drops only what nothing it sees descends from. | `Guard` | [`Unguarded.cfg`](Unguarded.cfg): a reader that has received a compaction's deletions before its snapshot stops merging a writer whose newer entries it already sees. |
| No two histories give a segment the same name: a segment's name is random. | `UniqueNames` | [`Names.cfg`](Names.cfg): the original and the clone each start a segment under the next counter name. Sync keeps both, one as a conflicted copy, and the original's deletion of its own segment by name removes the clone's. |
| A writer deletes only segments that end with a seal marker, which the process that closed the segment wrote. The segment open when a process crashed, or when its local root was copied, has none and is never deleted. | `SealedOnly` | [`Sealed.cfg`](Sealed.cfg): the clone compacts and deletes the segment the original still has open, and the original's next entry goes with it. |
| A writer deletes only segments every entry of which a snapshot in its directory certifies. | `FoldedOnly` | [`Folded.cfg`](Folded.cfg): the original seals a segment after the fork, and the clone compacts and deletes it, though no snapshot folds the original's entry. |
| Before each write, a writer confirms that the folder holds its last entry, and before deleting a superseded file, that it holds the snapshot superseding it. A writer whose last entry is gone takes a new id. | `CheckFirst` | [`Unchecked.cfg`](Unchecked.cfg): a restore takes the writer's last entry, and the writer appends after it. |

Snapshots and pending records are named at random too. The model gives every
file a unique name except segments under `UniqueNames = FALSE`.

Superseded files are deleted as soon as the snapshot that replaces them is
written. Deleting them one compaction later would protect no property here; it
only shortens the time a reader without a cached view shows a gap when sync
delivers files roughly in order.

## What is checked

| Property | Meaning |
| --- | --- |
| `ChainOrder` | A reader never merges an entry without its predecessor. |
| `NothingIgnored` | Every entry a reader sees is merged, held back and reported, shown as a run, or dismissed. A reader names no file, so this holds by construction. |
| `ShownApart` | A reader never both merges and shows an entry. |
| `NoReapply` | A reader never places an entry from a file while a tip folds it. |
| `NoSpuriousConflict` | An entry is shown as a run only when its chain forked at or before it, a restore or a dropped version took it, or the reader still holds what one took of that writer. |
| `UnforkedMerged` | Once every file has reached a reader, it merges every kept entry no fork precedes and that follows nothing lost. |
| `Agree` | Once every file has reached a reader, it merges and shows the same kept entries as a reader without a cached view, unless it still holds what the grace step would drop. |
| `ForksFromFiles` | Once every file has reached a reader, the files show every fork point it remembers, unless a restore or a dropped version took one of the point's successors. |
| `NoMergedFork` | A reader never merges two branches of one writer. |
| `NoOrphan` | A writer never appends after an entry the folder no longer holds. |
| `DismissalsHold` | A reader never merges what a dismissal it merged names; once every file has reached it, it neither shows nor merges what a kept, merged dismissal names. |
| `OwnDirectory` | A writer creates, appends to and deletes files only in its own directory. |
| `Retained` | The folder keeps every entry written, in a segment or folded in a snapshot, unless a restore or a dropped version took it. |
| `EffectAccounted` | Every effect begun has a pending record or a closing entry in the folder, unless a restore took both. |
| `DeliveredConverges` | Once every file has reached a reader, it merges, shows or has dismissed every kept entry, holds back only what follows a lost entry, and reports every fork among the kept entries it sees, whatever the delivery order. A reader without a cached view does the same. |
| `DeliveredSettles` | Once every file has reached a reader, every effect is closed, reported, or that reader's own effect in flight; this includes the effects of a writer whose local root was lost. |
| `Monotone` | A reader stops merging an entry only when it now shows it or has dismissed it, when a restore or a dropped version took it, or when the grace step drops it with nothing in sight descending from it, unless its local root is lost or replaced by a copy. |
| `NoFlip` | A run stays shown until it is dismissed, no file the reader sees holds it, or the grace step. |
| `ForksKept` | A fork point, once found, stays found, except by the grace step. |
| `OwnFileNoRetire` | A writer takes a new id for a fork only when an entry of its directory has two successors. |
| `Convergence` | With fair delivery, every reader eventually converges and settles or reports every effect. |
| `FreshMonotone` | False, as [`FreshView.cfg`](FreshView.cfg) shows: without a cached view, a deletion that arrives before the snapshot that replaces the file hides entries a reader has shown. |

| Config | Scenario | Run by |
| --- | --- | --- |
| `Portable` | a clone, crashes, compaction | `toshokan-spec` |
| `Recovery` | two effects, a lost local root | `toshokan-spec` |
| `Liveness` | a clone, a lost local root, one sync fault | `toshokan-spec` |
| `Grace` | a restored folder, and the grace step | `toshokan-spec` |
| `GraceChaos` | the grace step with a sync fault | `toshokan-spec` |
| `LivenessGrace` | `Convergence` with the grace step and a restored folder | `toshokan-spec` |
| `RestoreDir` | a restored folder or directory, and the grace step | `toshokan-spec` |
| `Resurface` | a deleted segment arriving after later compactions | `toshokan-spec` |
| `Drop` | a sync client keeping an earlier version of a segment | `toshokan-spec` |
| `ResumeMid` | a clone resumed mid-segment, appending to its own copy of it | `toshokan-spec-deep` |
| `Names`, `Sealed`, `Folded`, `Unchecked`, `FreshView`, `Uncertified`, `Uncarried`, `Unnumbered`, `Unguarded`, `Unquarantined` | expected violations | `toshokan-spec` |
| `Anchors` | expected violation, four entries | `toshokan-spec-deep` |
| `PortableWide` | `Portable` with four files | `toshokan-spec-deep` |
| `PortableDeep` | `Portable` with five files | `toshokan-spec-deep` |
| `Entries` | `Portable` with four entries | `toshokan-spec-deep` |
| `Dismiss` | a clone and dismissals, four entries and four files | `toshokan-spec-deep` |
| `RestoreDirFork` | `RestoreDir` with a clone | `toshokan-spec-deep` |
| `Branches` | a clone, six entries and six files: both branches compacting past each other | by hand, by simulation |
| `Deep` | a clone, effects and a lost local root together | `toshokan-spec-deep` |
| `Restore` | a restored folder, two effects | `toshokan-spec-deep` |
| `RestoreFork` | a restored folder, a clone | `toshokan-spec-deep` |
| `Sync` | two sync faults: torn, hidden or resurrected files | `toshokan-spec-deep` |
| `LivenessDeep` | `Liveness` with effects and more files | `toshokan-spec-deep` |

A config whose first line reads `\* Violates <Property>: …` must fail with that
violation; every other config must pass.

`Branches` is too large to explore whole. Run it by random simulation:
`tlc -simulate -depth 40 -workers auto -config Branches.cfg Portable.tla`.

## Refinement

The spec applies to the code because the code keeps the model's rules and its
reader's obligations. [SPEC.md](../SPEC.md) states each in full.

- Entries carry their number. A snapshot certifies every entry it folds: by the
  segments its chain was written to, by the heads of the snapshots on its chain,
  and by hash.
- A reader reads every parseable file in a writer's directory whatever its name,
  and derives its tips, placement, fork points, runs and gaps from those files
  and what it remembers, the same whatever order they arrived in. It remembers
  what it placed until the grace step lets go of what nothing it sees descends
  from.
- A writer deletes only segments of its directory that end with a seal marker
  and every line of which its new snapshot certifies, and snapshots whose heads
  it certifies. A process seals only the segment it closes, and names each
  segment at random.
- No writer id is kept in the local root before its genesis entry is durable in
  the folder, and a writer that has to stop is replaced by a new random id.
- Another writer's pending record is settled only with the user's consent, in
  the settling writer's own log, and removed only by its owner. An owner
  removes a record only once its log holds the closing entry, and never after
  it stops writing.
- Each fork point is reported once, and remembered until the grace step.
- After the local root is lost, the install reads from scratch as a new writer
  and never writes the old writer's directory again.

The tests check the spec's properties against the real reader, writer and
library. A sync simulator delivers the folder to readers as `Deliver` does:
whole files, prefixes ending at a line, deletions before the files that replace
them, resurrections, hidden files and conflicted copies under other names.

| Property or rule | Checked by |
| --- | --- |
| `ChainOrder`, `NothingIgnored`, `Retained`, `DeliveredConverges`, `Monotone`, `ForksKept` | `tests/portable.rs`: every delivery order of small scenarios, and seeded interleavings of writes, compactions, crashes, clones and lost local roots with sync; at each step a reader that reads only what changed places what one reading every file places. A remembered entry no file holds whose number a tip has passed counts as folded into that tip |
| `NoMergedFork`, `DismissalsHold`, `Agree`, the grace step | `tests/retention`: the value-level checks of the retention design run toshokan's encoding, compaction and derivation against an oracle that never compacts |
| `OwnDirectory` | `tests/library.rs`: every instance runs on a backend that panics on a write in another writer's directory |
| `EffectAccounted` | `tests/crash.rs`: every effect crashed after every operation, under torn, zeroed and lost tails, where moves rename and where they copy |
| `DeliveredSettles` | `tests/crash.rs` and `tests/library.rs`: a crash, or the loss of the local root, at every step is settled once, by the writer itself or with consent by another, where moves rename and where they copy; a spent record is reported only by a reader its closing entry has not reached |
| `Certify`, `CertForks` | `tests/portable.rs`: a clone branches from an entry the original folds and deletes, in every delivery order |
| `UniqueNames` | `tests/writer.rs`: one new, randomly named segment per process |
| `SealedOnly`, `FoldedOnly` | `tests/writer.rs` and `tests/library.rs`: compaction leaves a segment another process has open, one a crash left open, and a sealed one holding an entry off its chain, and deletes the sealed segments of earlier processes |
| `CheckFirst` | `tests/writer.rs` and `tests/library.rs`: a writer whose last entry the folder lost appends and compacts nothing, and is replaced |
| `Convergence` | Liveness is not tested; `DeliveredConverges` is checked at the end of every complete delivery |

The code also checks more than the model needs. The model treats a file as its
lines, so a file replaced by another of the same length is a change it sees. The
code compares contents where it would otherwise trust a length:

- Before appending to its open segment, a writer reads the segment's last line
  and confirms it is the writer's last entry (`tests/writer.rs`).
- A reader skips a file it read before only while its length, modification
  time and last line are unchanged, and reads a grown segment from the end of
  its last line only while the bytes there still end that line
  (`src/reader.rs`).
- Just before deleting a segment, compaction confirms that it still has the
  length it read and ends with its seal marker (`tests/writer.rs`).

What the model abstracts away is checked in Rust: the merge by
`tests/facts.rs`, which compares every arrival order, split and compaction of
bounded histories; file effects by `tests/crash.rs`; binding by the bounded
search in `src/binding.rs`.

## What is abstracted away

Fact values and the merge itself: the merge is a join over entries, so a
reader's state is the set of entries it has merged. Entry contents, clocks and
conflicts between field values are out of scope. So are user files, trash and
staging: an effect is its pending record and its closing entry. A reader reads
after every change it sees, and a sync fault is a step any reader can take at
any time. The user's consent to settle another writer's effect is the choice to
take that step. A writer's check and the write it guards are one step, as is a
restore: the folder takes one earlier state that it held.

The modeled restore is applied to the folder the writer checks: by a backup tool
on the writer's machine, or by a sync client that applies the rewind to this
device before the writer's next write. A restore made on another device reaches
this one through its sync client, which must then reconcile the writer's newer
segment with the older copy; that is the excluded case below of a client that
keeps one of two conflicting versions.

After a restore, a reader with a cached view keeps the entries the folder lost
until a grace period passes, and then derives its view again from the files it
sees, so the folder wins. The model allows that step at any time some merged
entry is held by no file the reader sees (`Grace`), which also covers a sync
client slower than the grace period. A reader that opens later never sees the
lost entries.

The model does not cover these cases, and the code does not prevent what they
cause:

- A running process cloned with its open file handles, such as a virtual
  machine resumed twice. Both copies append to one segment, and a line can
  follow the other copy's. Readers then report a fork or hold entries back.
- A sync client that keeps one of two conflicting versions of a file and drops
  the other. Entries only in the dropped version leave the folder. A reader that
  had placed them keeps showing them and reports them as removed.
- A restore that lands between a writer's check and the write it guards. The
  writer appends after an entry the folder no longer holds, and readers hold
  what it wrote back as a gap.
- A cached view that cannot be written after an append. The commit still
  succeeds, and says so. Until the view is written, a crash followed by a
  restore that takes the commit's entries takes them from this install's view,
  which `Monotone` otherwise keeps.
- Two entries with one hash. Hashes are 128 bits of BLAKE3, so an accidental
  collision is negligible, but a forger who writes both entries can make one
  with about 2^64 work.

## Power loss

The crash harness checks toshokan against the file system it models. Two NixOS
VM tests check the native backend against ext4, XFS, Btrfs and exFAT, on x86_64
Linux with KVM:

```sh
nix build .#toshokan-power-loss-smoke  # crashes a VM mid-workload, three times
nix build .#toshokan-power-loss        # replays every flush, on each file system
nix build .#toshokan-power-loss.ext4   # on one
```

The replay logs each workload's writes with dm-log-writes, then rebuilds the
disk as it stood at every flush and at sets of the writes between two flushes,
and checks each state after the file system's own recovery. An exFAT state that
`fsck.exfat` finds corrupt is checked but counted apart. A write is replayed
whole, never torn; the crash harness covers torn writes.
[examples/power_loss.rs](../examples/power_loss.rs) holds the workloads and the
checker. Neither test covers APFS, NTFS or a browser's storage.
