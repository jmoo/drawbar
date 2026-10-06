# The portable-root protocol, model-checked

[`Portable.tla`](Portable.tla) specifies how toshokan's writers keep logs in a
synced folder and how every reader merges them. TLC checks it against a sync
layer that delivers each file to each reader independently and in any order.

```sh
nix build .#nord.toshokan-spec       # small bounds; `nix flake check` runs it
nix build .#nord.toshokan-spec-deep  # larger bounds, about half an hour
```

To run one config by hand, `nix shell nixpkgs#tlaplus`, then
`tlc -workers auto -config Portable.cfg Portable.tla` in this directory.

## The protocol

Each writer has a random id, which names its directory, and only that writer
writes there. An entry is identified by its hash, which covers the hash of its
predecessor, so a writer's entries form a chain. A writer appends entries to
segments, folds its own chain into a snapshot that names the last entry it
folds, and later deletes the segments and snapshots the new snapshot supersedes.
A multi-step effect writes a pending record, logs the entry that closes it, then
deletes the record.

A reader takes every file in a writer's directory, whatever its name. It places
an entry when its predecessor is placed or folded by a snapshot it sees, holds an
entry after a gap back, and reports a fork when two entries share a predecessor.
It keeps a cached view that only grows.

A copied local root (a restored backup, a cloned disk) makes two instances write
one directory: a fork. An instance that sees its own directory forked, or that
has lost its local root, continues under a new writer id and never writes its old
directory again.

The protocol needs three rules beyond that. Each is a constant, and a config that
drops it shows the failure it prevents:

| Rule | Constant | Without it |
| --- | --- | --- |
| A snapshot records the hash of every entry it folds, with its predecessor. | `FoldHashes` | [`Anchors.cfg`](Anchors.cfg): a clone appends after an entry the original has folded and deleted. The reader cannot place the clone's entry, holds it back as a gap forever, and never reports the fork. |
| No two histories give a segment the same name; for example, a segment is named by the hash of its first entry. | `UniqueNames` | [`Names.cfg`](Names.cfg): the original and the clone each start a segment under the next counter name. Sync keeps both, one as a conflicted copy, and the original's deletion of its own segment by name removes the clone's. |
| A writer deletes only segments its own process sealed. The segment open when a process stopped, or when its local root was copied, is never deleted. | `SealedOnly` | [`Sealed.cfg`](Sealed.cfg): the clone compacts and deletes the segment the original still appends to, taking the original's newer entries with it. |

The fourth constant, `TwoPhase`, deletes superseded files one compaction after
the snapshot that replaces them rather than at once. No property depends on it:
[`OnePhase.cfg`](OnePhase.cfg) passes. It only narrows the window in which a
reader without a cached view shows a gap when sync delivers roughly in order,
which this model does not assume.

## What is checked

| Property | Meaning |
| --- | --- |
| `ChainOrder` | A reader never accepts an entry before its predecessor. |
| `NoFalseFork` | Every fork a reader reports is real; a gap is never reported as a fork. |
| `NothingIgnored` | Every entry a reader sees is merged or held back and reported. A reader names no file, so this holds by construction. |
| `OwnDirectory` | A writer creates, appends to and deletes files only in its own directory. |
| `Retained` | The folder keeps every entry written, in a segment or folded in a snapshot. |
| `EffectAccounted` | Every effect begun has a pending record or a closing entry in the folder. |
| `DeliveredConverges` | Once every file has reached a reader, it has accepted every entry written, holds back nothing, and reports every fork, whatever the delivery order. A reader without a cached view computes the same entries. |
| `DeliveredSettles` | Once every file has reached a reader, every effect is closed, reported, or that reader's own effect in flight; this includes the effects of a writer whose local root was lost. |
| `Monotone` | A reader's view never loses an entry, unless its local root is lost or replaced by a copy. |
| `Convergence` | With fair delivery, every reader eventually converges and settles or reports every effect. |
| `FreshMonotone` | False, as [`FreshView.cfg`](FreshView.cfg) shows: without a cached view, a deletion that arrives before the snapshot that replaces the file hides entries a reader has shown. |

| Config | Scenario | Run by |
| --- | --- | --- |
| `Portable` | a clone, crashes, compaction | `toshokan-spec` |
| `Recovery` | effects, a lost local root | `toshokan-spec` |
| `Liveness` | a clone, a lost local root, one sync fault | `toshokan-spec` |
| `Names`, `Anchors`, `Sealed`, `FreshView` | expected violations | `toshokan-spec` |
| `Deep` | a clone, effects and a lost local root together | `toshokan-spec-deep` |
| `OnePhase` | `Portable` without two-phase deletion | `toshokan-spec-deep` |
| `Sync` | two sync faults: torn, hidden or resurrected files | `toshokan-spec-deep` |
| `LivenessDeep` | `Liveness` with effects and more files | `toshokan-spec-deep` |

A config whose first line reads `\* Violates <Property>: …` must fail with that
violation; every other config must pass.

## What is abstracted away

Fact values and the merge itself: the merge is a join over entries, so a
reader's state is the set of entries it has merged. Entry contents, clocks and
conflicts between field values are out of scope. So are user files, trash and
staging: an effect is its pending record and its closing entry. A reader reads
after every change it sees, and a sync fault is a step any reader can take at
any time.

The model does not cover a running process cloned with its open file handles,
a sync client that keeps one of two conflicting versions and drops the other,
a folder restored from an older backup, which removes entries no writer
deleted, or hash collisions.
