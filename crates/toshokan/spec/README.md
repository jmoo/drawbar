# The portable-root protocol, model-checked

[`Portable.tla`](Portable.tla) specifies how toshokan's writers keep logs in a
synced folder and how every reader merges them. TLC checks it against a sync
layer that delivers each file to each reader independently and in any order.

```sh
nix build .#nord.toshokan-spec       # small bounds; `nix flake check` runs it
nix build .#nord.toshokan-spec-deep  # larger bounds, about 80 minutes
```

To run one config by hand, `nix shell nixpkgs#tlaplus`, then
`tlc -workers auto -config Portable.cfg Portable.tla` in this directory.

## The protocol

Each writer has a random id, which names its directory, and only that writer
writes there. An entry is identified by its hash, which covers the hash of its
predecessor, so a writer's entries form a chain. A writer appends entries to
segments, folds its own chain into a snapshot, and then deletes the segments
and the snapshot the new one supersedes. A multi-step effect writes a pending
record, logs the entry that closes it, then deletes the record. Another writer
that sees the record of an effect no entry closes reports it, and with the
user's consent closes it in its own log; only the record's owner deletes it.

A reader takes every file in a writer's directory, whatever its name. It places
an entry when its predecessor is placed or folded by a snapshot it sees, holds an
entry after a gap back, and reports a fork when two entries share a predecessor.
It keeps a cached view that only grows.

A copied local root (a restored backup, a cloned disk) makes two instances write
one directory: a fork. A copy may be taken at any time, even before the writer's
first entry. A folder restored from a backup takes away entries no writer
deleted. An instance that sees its own directory forked, whose last entry is no
longer in the folder, or that has lost its local root, continues under a new
writer id and never writes its old directory again.

The protocol needs four rules beyond that. Each is a constant, and a config that
drops it shows the failure it prevents:

| Rule | Constant | Without it |
| --- | --- | --- |
| A snapshot records the hash of every entry it folds, with its predecessor. | `FoldHashes` | [`Anchors.cfg`](Anchors.cfg): the original and a clone each write a first entry, and the original folds its own into a snapshot and deletes the segment. The snapshot names that entry but not its predecessor, so the reader never sees that the two entries share one and never reports the fork. |
| No two histories give a segment the same name: a segment's name is random. | `UniqueNames` | [`Names.cfg`](Names.cfg): the original and the clone each start a segment under the next counter name. Sync keeps both, one as a conflicted copy, and the original's deletion of its own segment by name removes the clone's. |
| A writer deletes only segments its own process sealed. The segment open when a process stopped, or when its local root was copied, is never deleted. | `SealedOnly` | [`Sealed.cfg`](Sealed.cfg): the clone compacts and deletes the segment the original still appends to, taking the original's newer entries with it. |
| Before each write, a writer confirms that the folder holds its last entry, and before deleting a superseded file, that it holds the snapshot superseding it. A writer whose last entry is gone takes a new id. | `CheckFirst` | [`Unchecked.cfg`](Unchecked.cfg): a restore takes the writer's last entry, and the writer keeps appending after it; no reader places what it writes. |

Snapshots and pending records are named at random too. The model gives every
file a unique name except segments under `UniqueNames = FALSE`.

Superseded files are deleted as soon as the snapshot that replaces them is
written. Deleting them one compaction later would protect no property here; it
only shortens the time a reader without a cached view shows a gap when sync
delivers files roughly in order.

## What is checked

| Property | Meaning |
| --- | --- |
| `ChainOrder` | A reader never accepts an entry before its predecessor. |
| `NothingIgnored` | Every entry a reader sees is merged or held back and reported. A reader names no file, so this holds by construction. |
| `OwnDirectory` | A writer creates, appends to and deletes files only in its own directory. |
| `Retained` | The folder keeps every entry written, in a segment or folded in a snapshot, unless a restore took it. |
| `EffectAccounted` | Every effect begun has a pending record or a closing entry in the folder, unless a restore took both. |
| `DeliveredConverges` | Once every file has reached a reader, it has accepted every entry the folder keeps, holds back only entries a restore took, and reports every fork among the kept entries, whatever the delivery order. A reader without a cached view computes the same entries. This is the evidence that every chained entry in the folder is merged or reported. |
| `DeliveredSettles` | Once every file has reached a reader, every effect is closed, reported, or that reader's own effect in flight; this includes the effects of a writer whose local root was lost. |
| `Monotone` | A reader's view never loses an entry, unless its local root is lost or replaced by a copy. |
| `ForksKept` | A reader never retracts a fork it has reported, under the same exception, so it reports each fork once. |
| `Convergence` | With fair delivery, every reader eventually converges and settles or reports every effect. |
| `FreshMonotone` | False, as [`FreshView.cfg`](FreshView.cfg) shows: without a cached view, a deletion that arrives before the snapshot that replaces the file hides entries a reader has shown. |

| Config | Scenario | Run by |
| --- | --- | --- |
| `Portable` | a clone, crashes, compaction | `toshokan-spec` |
| `Recovery` | effects, a lost local root | `toshokan-spec` |
| `Liveness` | a clone, a lost local root, one sync fault | `toshokan-spec` |
| `Names`, `Anchors`, `Sealed`, `Unchecked`, `FreshView` | expected violations | `toshokan-spec` |
| `PortableDeep` | `Portable` with five files | `toshokan-spec-deep` |
| `Entries` | `Portable` with four entries | `toshokan-spec-deep` |
| `Deep` | a clone, effects and a lost local root together | `toshokan-spec-deep` |
| `Restore` | a restored folder, effects | `toshokan-spec-deep` |
| `RestoreFork` | a restored folder, a clone | `toshokan-spec-deep` |
| `Sync` | two sync faults: torn, hidden or resurrected files | `toshokan-spec-deep` |
| `LivenessDeep` | `Liveness` with effects and more files | `toshokan-spec-deep` |

A config whose first line reads `\* Violates <Property>: …` must fail with that
violation; every other config must pass.

## Refinement

The spec applies to the code because the code keeps the model's rules and its
reader's obligations. [SPEC.md](../SPEC.md) states each in full.

- A snapshot lists every entry it folds, in chain order, and so does the cached
  view.
- A reader reads every parseable file in a writer's directory whatever its name,
  places entries by chain only, caches only entries whose predecessor it holds,
  and never shrinks its cache.
- A process deletes only segments it opened and sealed itself, and names each
  segment at random.
- No writer id is kept in the local root before its genesis entry is durable in
  the folder, and a writer that has to stop is replaced by a new random id.
- Another writer's pending record is settled only with the user's consent, in
  the settling writer's own log, and removed only by its owner.
- Each fork is reported once, found from the cached pairs of every entry ever
  seen rather than from the files present now.
- After the local root is lost, the install reads from scratch as a new writer
  and never writes the old writer's directory again.

The tests check the spec's properties against the real reader, writer and
library. A sync simulator delivers the folder to readers as `Deliver` does:
whole files, prefixes ending at a line, deletions before the files that replace
them, resurrections, hidden files and conflicted copies under other names.

| Property or rule | Checked by |
| --- | --- |
| `ChainOrder`, `NothingIgnored`, `Retained`, `DeliveredConverges`, `Monotone`, `ForksKept` | `tests/portable.rs`: every delivery order of small scenarios, and seeded interleavings of writes, compactions, crashes, clones and lost local roots with sync |
| `OwnDirectory` | `tests/library.rs`: every instance runs on a backend that panics on a write in another writer's directory |
| `EffectAccounted` | `tests/crash.rs`: every effect crashed after every operation, under torn, zeroed and lost tails |
| `DeliveredSettles` | `tests/crash.rs` and `tests/library.rs`: a crash, or the loss of the local root, at every step is settled once, by the writer itself or with consent by another |
| `FoldHashes` | `tests/portable.rs`: a clone branches from an entry the original folds and deletes, in every delivery order |
| `UniqueNames` | `tests/writer.rs`: one new, randomly named segment per process |
| `SealedOnly` | `tests/writer.rs` and `tests/library.rs`: compaction deletes only segments this process sealed |
| `CheckFirst` | `tests/writer.rs` and `tests/library.rs`: a writer whose last entry the folder lost appends and compacts nothing, and is replaced |
| `Convergence` | Liveness is not tested; `DeliveredConverges` is checked at the end of every complete delivery |

The code also checks more than the model needs. The model treats a file as its
lines, so a file replaced by another of the same length is a change it sees. The
code compares contents where it would otherwise trust a length:

- Before appending to its open segment, a writer reads the segment's last line
  and confirms it is the writer's last entry (`tests/writer.rs`).

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

After a restore, a reader with a cached view keeps the entries the folder lost,
as `Monotone` requires, while a reader that opens later never sees them. Both
count as converged. The model accepts this divergence; the code surfaces it and
waits for consent. A reader keeps showing the facts whose entries no file in the
folder holds, and reports them as removed by a restore. The user either lets
them go, which shows their writers as the folder holds them on that install, or
adopts them, which republishes them as new entries in the reader's own log.
Neither happens silently: a restore is often deliberate. Letting go changes what
is shown, not the cached view, which still only grows. `tests/library.rs` checks
both choices through both drivers.

The model does not cover a running process cloned with its open file handles,
a sync client that keeps one of two conflicting versions and drops the other,
a restore that lands between a writer's check and the write it guards, or hash
collisions.
