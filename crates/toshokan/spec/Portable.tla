------------------------------ MODULE Portable ------------------------------
(***************************************************************************)
(* The portable-root protocol of toshokan: per-writer hash-chained logs in *)
(* a synced folder, compacted by their owner and merged by every reader.   *)
(*                                                                         *)
(* An entry is a natural number standing for its hash. Its record holds    *)
(* the writer directory it was appended to, its predecessor (None for the  *)
(* first entry of a chain) and the effect it closes, if any. Fact values   *)
(* are not modeled: the merge is a join over entries, so a reader's state  *)
(* is the set of entries whose effects it has merged (`acc`).              *)
(*                                                                         *)
(* A file is a segment (a growing sequence of entries), a snapshot (the    *)
(* chain it folds, anchored at its last entry) or a pending record (one    *)
(* effect in flight). Files are never edited in place: segments only grow, *)
(* and the owner deletes whole files. Each reader sees each file through   *)
(* `vis`: nothing, a readable prefix, or all of it, independently of every *)
(* other file and every other reader.                                      *)
(*                                                                         *)
(* Four protocol rules are constants, TRUE when in force, so that a config *)
(* can drop one and exhibit the failure it permits.                        *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Instances,      \* the first element starts as a writer, the rest as readers
    MaxEntries,     \* bound on entries written
    MaxFiles,       \* bound on files created
    MaxEffects,     \* bound on multi-step effects begun
    MaxChaos,       \* bound on sync behavior beyond late or in-order delivery
    MaxClones,      \* bound on cloned writer histories
    MaxLosses,      \* bound on lost local roots
    MaxWriters,     \* bound on writer ids, which name directories
    MaxRestores,    \* bound on restores of the folder from its backup
    FoldHashes,     \* a snapshot or cached view records every hash it folds
    UniqueNames,    \* no two histories give a segment the same name
    SealedOnly,     \* a writer deletes only segments its own process closed
    CheckFirst      \* a writer confirms the folder holds what it builds on

None == 0
First == CHOOSE i \in Instances : TRUE

VARIABLES
    ents,       \* entry records, indexed by entry
    files,      \* file records, indexed by file
    nW,         \* writer ids in use
    nEff,       \* effects begun
    chaos,      \* chaos budget spent
    clones,
    losses,
    backup,     \* {} or {the folder as it was when backed up}
    restores,
    lost,       \* entries a restore took from the folder
    lostEffects,\* effects a restore took every trace of
    \* Instance state, kept in the instance's local root.
    role,       \* "writer" or "reader"
    wid,        \* the directory this instance writes
    head,       \* the last entry this instance wrote
    seg,        \* the segment it appends to, or None
    segs,       \* segments since its last snapshot that it may delete
    dying,      \* files it is deleting now
    lastSnap,   \* its last snapshot, or None
    nextName,   \* its segment name counter, when names are not unique
    inflight,   \* its effect in flight
    \* Reader state: its cached view, also in the local root, and `vis`, its
    \* copy of the folder.
    acc,        \* entries whose effects the reader has merged
    known,      \* entry hashes the reader can chain from
    pairs,      \* entries whose predecessor the reader has seen
    vis         \* vis[r][f]: how much of file f reader r sees

writer == <<ents, files, nW, nEff, clones, losses, role, wid, head, seg, segs,
            dying, lastSnap, nextName, inflight>>
reader == <<acc, known, pairs, vis>>
folder == <<backup, restores, lost, lostEffects>>
vars == <<writer, reader, folder, chaos>>

NoEffect == [eff |-> 0, rec |-> None, logged |-> FALSE]
Files == 1..Len(files)
All == 1..Len(ents)
Range(s) == {s[k] : k \in 1..Len(s)}

RECURSIVE Chain(_)
Chain(e) == IF e = None THEN {} ELSE {e} \cup Chain(ents[e].prev)

FileLen(f) == IF files[f].kind = "seg" THEN Len(files[f].ents) ELSE 1

\* Entries a folder holds, in a segment or folded in a snapshot.
Holds(fs) ==
    UNION {IF fs[f].kind = "seg" THEN Range(fs[f].ents) ELSE fs[f].folds :
               f \in {g \in 1..Len(fs) : ~fs[g].deleted}}

\* Entries written and not taken by a restore.
Kept == All \ lost

-----------------------------------------------------------------------------
(* Reading. A reader names no file: it takes every file it sees in a       *)
(* writer's directory and places entries by chain alone.                   *)

\* Where a restore shortened a segment, a longer copy shows only what the
\* restore left; the reader merged the rest when it read it.
Shown(v, f) == IF v[f] < Len(files[f].ents) THEN v[f] ELSE Len(files[f].ents)
Present(v) ==
    UNION {{files[f].ents[k] : k \in 1..Shown(v, f)} : f \in {g \in Files : files[g].kind = "seg"}}
Snaps(v) == {f \in Files : files[f].kind = "snap" /\ v[f] = 1}
Folds(v) == UNION {files[s].folds : s \in Snaps(v)}
\* What a snapshot lets a reader chain from: its anchor, or every hash it folds.
SnapRoots(v) ==
    IF FoldHashes THEN Folds(v) ELSE {files[s].anchor : s \in Snaps(v)}

\* An entry is placed when its predecessor is placed or it starts a chain.
\* Nothing else is placed: an entry after a gap is held back.
RECURSIVE Place(_, _)
Place(K, P) ==
    LET next == K \cup {x \in P : ents[x].prev = None \/ ents[x].prev \in K}
    IN IF next = K THEN K ELSE Place(next, P)

SeenPairs(v, p0) == p0 \cup Present(v) \cup (IF FoldHashes THEN Folds(v) ELSE {})
Placed(v, k0) == Place(k0 \cup SnapRoots(v), Present(v))

\* A reader reads after every change it sees, and its cached view only adds.
ReadWith(r, v, a0, k0, p0) ==
    LET K == Placed(v, k0) IN
    /\ acc' = [acc EXCEPT ![r] = a0 \cup Folds(v) \cup K]
    /\ known' = [known EXCEPT ![r] = K]
    /\ pairs' = [pairs EXCEPT ![r] = SeenPairs(v, p0)]

\* The view a reader without a cached view computes from what it sees.
Fresh(v) == Folds(v) \cup Placed(v, {})

Held(r) == Present(vis[r]) \ known[r]
ForksIn(E) ==
    {<<ents[x].dir, ents[x].prev>> :
        x \in {y \in E : \E z \in E : z # y /\ ents[z].dir = ents[y].dir
                                          /\ ents[z].prev = ents[y].prev}}
Forks(r) == ForksIn(pairs[r])

Closed(r) == {ents[x].closes : x \in acc[r]} \ {0}
\* Pending records the reader sees whose effect no merged entry closes.
Reported(r) ==
    {files[f].eff : f \in {g \in Files : files[g].kind = "pend" /\ vis[r][g] = 1}}
        \ (Closed(r) \cup {inflight[r].eff})

\* Every file in the folder is wholly visible to r.
Delivered(r) == \A f \in Files : ~files[f].deleted => vis[r][f] = FileLen(f)

-----------------------------------------------------------------------------
(* Writers. Instance i touches only files in directory wid[i].             *)

\* The entry this writer last wrote is no longer in the folder.
HeadLost(i) == head[i] # None /\ head[i] \notin Holds(files)

\* Before each write, a writer confirms that its head is in the folder;
\* before a deletion, also that the snapshot superseding the file is.
Writable(i) == role[i] = "writer" /\ (CheckFirst => ~HeadLost(i))

NewFile(kind, d, name, es, anchor, fo, eff) ==
    [kind |-> kind, dir |-> d, name |-> name, ents |-> es, anchor |-> anchor,
     folds |-> fo, eff |-> eff, deleted |-> FALSE]

\* Appending to no open segment starts a new one; a fresh open never
\* appends to a segment it did not start.
AppendEntry(i, closes) ==
    LET e == Len(ents) + 1
        new == seg[i] = None
        f == IF new THEN Len(files) + 1 ELSE seg[i]
    IN /\ Writable(i)
       /\ Len(ents) < MaxEntries
       /\ new => Len(files) < MaxFiles
       /\ ents' = Append(ents, [dir |-> wid[i], prev |-> head[i], closes |-> closes])
       /\ files' = IF new
                   THEN Append(files, NewFile("seg", wid[i],
                            IF UniqueNames THEN f ELSE nextName[i], <<e>>, None, {}, 0))
                   ELSE [files EXCEPT ![f].ents = Append(@, e)]
       /\ nextName' = IF new /\ ~UniqueNames
                      THEN [nextName EXCEPT ![i] = @ + 1] ELSE nextName
       /\ seg' = [seg EXCEPT ![i] = f]
       /\ segs' = IF new THEN [segs EXCEPT ![i] = @ \cup {f}] ELSE segs
       /\ head' = [head EXCEPT ![i] = e]

Write(i) ==
    /\ AppendEntry(i, 0)
    /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying, lastSnap,
                   inflight, reader, folder, chaos>>

\* The owner folds its own chain into a snapshot, closes its segment, and
\* deletes the segments and snapshot the new one supersedes.
Compact(i) ==
    LET s == Len(files) + 1
        old == IF lastSnap[i] = None THEN {} ELSE {lastSnap[i]}
    IN /\ Writable(i)
       /\ segs[i] # {}
       /\ Len(files) < MaxFiles
       /\ files' = Append(files, NewFile("snap", wid[i], s, <<>>, head[i], Chain(head[i]), 0))
       /\ lastSnap' = [lastSnap EXCEPT ![i] = s]
       /\ seg' = [seg EXCEPT ![i] = None]
       /\ segs' = [segs EXCEPT ![i] = {}]
       /\ dying' = [dying EXCEPT ![i] = @ \cup segs[i] \cup old]
       /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, nextName,
                      inflight, reader, folder, chaos>>

\* A deletion names a file. Sync applies it to whichever file holds that
\* name, which for a segment name two histories chose may be the other's.
DeleteOne(i) ==
    /\ Writable(i)
    /\ \E f \in dying[i] :
        LET namesake == {g \in Files : /\ files[g].kind = files[f].kind
                                       /\ files[g].dir = files[f].dir
                                       /\ files[g].name = files[f].name
                                       /\ ~files[g].deleted}
        IN /\ CheckFirst => ~files[lastSnap[i]].deleted
           /\ dying' = [dying EXCEPT ![i] = @ \ {f}]
           /\ IF namesake = {} THEN UNCHANGED files
              ELSE \E g \in namesake : files' = [files EXCEPT ![g].deleted = TRUE]
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg, segs,
                   lastSnap, nextName, inflight, reader, folder, chaos>>

\* A multi-step effect: write its pending record, log the entry that
\* closes it, then delete the record.
Begin(i) ==
    LET f == Len(files) + 1 IN
    /\ Writable(i)
    /\ inflight[i] = NoEffect
    /\ nEff < MaxEffects
    /\ Len(files) < MaxFiles
    /\ files' = Append(files, NewFile("pend", wid[i], f, <<>>, None, {}, nEff + 1))
    /\ inflight' = [inflight EXCEPT ![i] = [eff |-> nEff + 1, rec |-> f, logged |-> FALSE]]
    /\ nEff' = nEff + 1
    /\ UNCHANGED <<ents, nW, clones, losses, role, wid, head, seg, segs,
                   dying, lastSnap, nextName, reader, folder, chaos>>

Close(i) ==
    /\ inflight[i].eff # 0
    /\ ~inflight[i].logged
    /\ AppendEntry(i, inflight[i].eff)
    /\ inflight' = [inflight EXCEPT ![i].logged = TRUE]
    /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying, lastSnap,
                   reader, folder, chaos>>

Unpend(i) ==
    /\ Writable(i)
    /\ inflight[i].logged
    /\ files' = [files EXCEPT ![inflight[i].rec].deleted = TRUE]
    /\ inflight' = [inflight EXCEPT ![i] = NoEffect]
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg, segs,
                   dying, lastSnap, nextName, reader, folder, chaos>>

\* With the user's consent, a writer closes an effect it reports by logging
\* the closing entry in its own directory. The record stays: only its owner
\* deletes it. Consent is the choice to take this step.
Settle(i) ==
    \E e \in Reported(i) :
        /\ AppendEntry(i, e)
        /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying,
                       lastSnap, inflight, reader, folder, chaos>>

\* A copied local root (a restored backup, a cloned disk, copied app data)
\* opens on another machine as the same writer with the same history.
Clone(i, j) ==
    /\ role[i] = "writer"
    /\ role[j] = "reader"
    /\ clones < MaxClones
    /\ clones' = clones + 1
    /\ role' = [role EXCEPT ![j] = "writer"]
    /\ wid' = [wid EXCEPT ![j] = wid[i]]
    /\ head' = [head EXCEPT ![j] = head[i]]
    /\ seg' = [seg EXCEPT ![j] = None]
    /\ segs' = [segs EXCEPT ![j] = IF SealedOnly THEN segs[i] \ {seg[i]} ELSE segs[i]]
    /\ dying' = [dying EXCEPT ![j] = dying[i]]
    /\ lastSnap' = [lastSnap EXCEPT ![j] = lastSnap[i]]
    /\ nextName' = [nextName EXCEPT ![j] = nextName[i]]
    /\ inflight' = [inflight EXCEPT ![j] = inflight[i]]
    /\ ReadWith(j, vis[j], acc[i], known[i], pairs[i])
    /\ UNCHANGED <<ents, files, nW, nEff, losses, vis, folder, chaos>>

\* A crash or quit. The next open starts a new segment; the one left open
\* may still be appended to by a process this one cannot see.
Restart(i) ==
    /\ role[i] = "writer"
    /\ seg[i] # None
    /\ seg' = [seg EXCEPT ![i] = None]
    /\ segs' = IF SealedOnly THEN [segs EXCEPT ![i] = @ \ {seg[i]}] ELSE segs
    /\ UNCHANGED <<ents, files, nW, nEff, clones, losses, role, wid, head,
                   dying, lastSnap, nextName, inflight, reader, folder, chaos>>

\* Instance i continues under a new writer id and never writes or compacts
\* its old directory again.
Renew(i) ==
    /\ nW < MaxWriters
    /\ nW' = nW + 1
    /\ wid' = [wid EXCEPT ![i] = nW + 1]
    /\ head' = [head EXCEPT ![i] = None]
    /\ seg' = [seg EXCEPT ![i] = None]
    /\ segs' = [segs EXCEPT ![i] = {}]
    /\ dying' = [dying EXCEPT ![i] = {}]
    /\ lastSnap' = [lastSnap EXCEPT ![i] = None]
    /\ nextName' = [nextName EXCEPT ![i] = 0]
    /\ inflight' = [inflight EXCEPT ![i] = NoEffect]

\* Losing the local root loses the cached view and the effect in flight;
\* the next open is a new writer that reads the folder from scratch.
LoseRoot(i) ==
    /\ role[i] = "writer"
    /\ losses < MaxLosses
    /\ losses' = losses + 1
    /\ Renew(i)
    /\ ReadWith(i, vis[i], {}, {}, {})
    /\ UNCHANGED <<ents, files, nEff, clones, role, vis, folder, chaos>>

\* An instance that sees its own directory forked takes a new writer id.
LeaveFork(i) ==
    /\ role[i] = "writer"
    /\ inflight[i] = NoEffect
    /\ \E fk \in Forks(i) : fk[1] = wid[i]
    /\ Renew(i)
    /\ UNCHANGED <<ents, files, nEff, clones, losses, role, reader, folder, chaos>>

\* An instance whose head is no longer in the folder takes a new writer id
\* rather than append after it.
LeaveRestored(i) ==
    /\ CheckFirst
    /\ role[i] = "writer"
    /\ HeadLost(i)
    /\ Renew(i)
    /\ UNCHANGED <<ents, files, nEff, clones, losses, role, reader, folder, chaos>>

-----------------------------------------------------------------------------
(* Restores. The folder, not a local root, goes back to an earlier state:  *)
(* every file is as it was then, and files created since are gone. The     *)
(* restore is applied to the folder the writers check, by a local backup   *)
(* tool or by a sync client that applies it to this device before the next *)
(* write. A restore on another device that a sync client merges into this  *)
(* one is a client keeping one of two versions, which is not modeled.      *)

Backup ==
    /\ backup = {}
    /\ restores < MaxRestores
    /\ backup' = {files}
    /\ UNCHANGED <<writer, reader, restores, lost, lostEffects, chaos>>

Restore ==
    /\ restores < MaxRestores
    /\ \E b \in backup :
        LET fs == [f \in Files |-> IF f <= Len(b) THEN b[f]
                                  ELSE [files[f] EXCEPT !.deleted = TRUE]]
            traced == {files[f].eff : f \in {g \in Files : fs[g].kind = "pend" /\ ~fs[g].deleted}}
                          \cup {ents[e].closes : e \in Holds(fs)}
        IN /\ files' = fs
           /\ lost' = lost \cup (All \ Holds(fs))
           /\ lostEffects' = lostEffects \cup ((1..nEff) \ traced)
    /\ restores' = restores + 1
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg, segs,
                   dying, lastSnap, nextName, inflight, reader, backup, chaos>>

-----------------------------------------------------------------------------
(* Sync. Each file reaches each reader independently. A file may show any  *)
(* prefix of what was appended, grow to all of it, arrive after its own    *)
(* deletion, reappear after it, and stay. Hiding or shortening a file,     *)
(* other than by delivering its deletion or a restore, spends the chaos    *)
(* budget.                                                                 *)

Show(r, f, k) ==
    /\ vis' = [vis EXCEPT ![r][f] = k]
    /\ ReadWith(r, vis'[r], acc[r], known[r], pairs[r])
    /\ UNCHANGED <<writer, folder>>

Deliver(r) ==
    \E f \in Files : \E k \in 0..FileLen(f) :
        /\ k # vis[r][f]
        /\ IF k > vis[r][f] \/ k = FileLen(f) \/ (k = 0 /\ files[f].deleted)
           THEN UNCHANGED chaos
           ELSE chaos < MaxChaos /\ chaos' = chaos + 1
        /\ Show(r, f, k)

DeliverAll(r) ==
    \E f \in Files :
        /\ ~files[f].deleted
        /\ vis[r][f] # FileLen(f)
        /\ Show(r, f, FileLen(f))
        /\ UNCHANGED chaos

-----------------------------------------------------------------------------

Init ==
    /\ ents = <<>>
    /\ files = <<>>
    /\ nW = 1
    /\ nEff = 0
    /\ chaos = 0
    /\ clones = 0
    /\ losses = 0
    /\ backup = {}
    /\ restores = 0
    /\ lost = {}
    /\ lostEffects = {}
    /\ role = [i \in Instances |-> IF i = First THEN "writer" ELSE "reader"]
    /\ wid = [i \in Instances |-> IF i = First THEN 1 ELSE 0]
    /\ head = [i \in Instances |-> None]
    /\ seg = [i \in Instances |-> None]
    /\ segs = [i \in Instances |-> {}]
    /\ dying = [i \in Instances |-> {}]
    /\ lastSnap = [i \in Instances |-> None]
    /\ nextName = [i \in Instances |-> 0]
    /\ inflight = [i \in Instances |-> NoEffect]
    /\ acc = [i \in Instances |-> {}]
    /\ known = [i \in Instances |-> {}]
    /\ pairs = [i \in Instances |-> {}]
    /\ vis = [i \in Instances |-> [f \in 1..MaxFiles |-> 0]]

Next ==
    \/ \E i \in Instances :
        \/ Write(i)
        \/ Compact(i)
        \/ DeleteOne(i)
        \/ Begin(i)
        \/ Close(i)
        \/ Unpend(i)
        \/ Settle(i)
        \/ Restart(i)
        \/ LoseRoot(i)
        \/ LeaveFork(i)
        \/ LeaveRestored(i)
        \/ \E j \in Instances : Clone(i, j)
    \/ Backup
    \/ Restore
    \/ \E r \in Instances : Deliver(r)

\* Writers may stop at any time; sync eventually delivers every file.
Spec == Init /\ [][Next]_vars /\ \A r \in Instances : WF_vars(DeliverAll(r))

-----------------------------------------------------------------------------
(* Safety.                                                                 *)

\* A reader never accepts an entry before its predecessor.
ChainOrder ==
    \A r \in Instances : \A e \in acc[r] :
        ents[e].prev = None \/ ents[e].prev \in acc[r]

\* Every entry a reader sees is merged or held back as a gap and reported;
\* a file under any name, including a sync conflicted copy, is read.
NothingIgnored ==
    \A r \in Instances : Present(vis[r]) \subseteq acc[r] \cup Held(r)

\* A writer creates, appends to and deletes files only in its own directory.
OwnDirectory ==
    \A i \in Instances : role[i] = "writer" =>
        \A f \in segs[i] \cup dying[i]
                 \cup ({seg[i], lastSnap[i], inflight[i].rec} \ {None}) :
            files[f].dir = wid[i]

\* The folder keeps every entry written, in a segment or folded in a
\* snapshot, unless a restore took it.
Retained == Kept \subseteq Holds(files)

\* Every effect begun has a pending record or a closing entry in the folder,
\* unless a restore took both.
EffectAccounted ==
    \A x \in (1..nEff) \ lostEffects :
        \/ \E f \in Files : files[f].kind = "pend" /\ files[f].eff = x /\ ~files[f].deleted
        \/ \E e \in Kept : ents[e].closes = x

\* Once every file has reached a reader, it accepts every entry the folder
\* keeps, whatever the delivery order, holds back only what a restore took,
\* and reports every fork among the kept entries. A reader without a cached
\* view computes the same.
Converged(r) ==
    /\ Kept \subseteq acc[r]
    /\ Held(r) \subseteq lost
    /\ ForksIn(Kept) \subseteq Forks(r)
DeliveredConverges ==
    \A r \in Instances : Delivered(r) => Converged(r) /\ Kept \subseteq Fresh(vis[r])

\* Once every file has reached a reader, every effect begun is closed,
\* reported, or the reader's own effect in flight, unless a restore took it.
Settled(r) ==
    \A x \in (1..nEff) \ lostEffects :
        x \in Closed(r) \cup Reported(r) \cup {inflight[r].eff}
DeliveredSettles == \A r \in Instances : Delivered(r) => Settled(r)

\* A reader's view never loses an entry or a fork, compaction, delivery order
\* and restores notwithstanding, unless its local root is lost or replaced by
\* a copy. A fork is reported when it first appears, so it is reported once.
LocalRootKept(r) == ~((losses' > losses \/ clones' > clones) /\ wid'[r] # wid[r])
Monotone ==
    [][\A r \in Instances : LocalRootKept(r) => acc[r] \subseteq acc'[r]]_vars
ForksKept ==
    [][\A r \in Instances : LocalRootKept(r) => Forks(r) \subseteq (Forks(r))']_vars

\* False: without a cached view, delivery order can hide entries a reader
\* has shown, which is why readers keep one.
FreshMonotone ==
    [][\A r \in Instances : Fresh(vis[r]) \subseteq Fresh(vis'[r])]_vars

-----------------------------------------------------------------------------
(* Liveness.                                                               *)

\* Once writes stop, every reader converges and settles or reports every
\* effect, including those of a writer whose local root was lost.
Convergence == \A r \in Instances : <>[](Converged(r) /\ Settled(r))

=============================================================================
