------------------------------ MODULE Portable ------------------------------
(***************************************************************************)
(* The portable-root protocol of toshokan: per-writer hash-chained logs in *)
(* a synced folder, compacted by their owner and merged by every reader.   *)
(*                                                                         *)
(* An entry is a natural number standing for its hash. Its record holds    *)
(* the writer directory it was appended to, its predecessor (None for the  *)
(* first entry of a chain) and the effect it closes, if any. Fact values   *)
(* are not modeled: the merge is a join over entries, so a reader's state  *)
(* is the set of entries whose effects it has merged (`acc`). An entry     *)
(* also holds its number in its chain, 0 for the first, and the entries it *)
(* dismisses, if any.                                                      *)
(*                                                                         *)
(* A file is a segment (a growing sequence of entries, which the process   *)
(* writing it seals when it closes it), a snapshot (anchored at its last   *)
(* entry, it reflects the chain it folds but lets a reader chain only from *)
(* its anchor and recognize only the lines it certified) or a pending      *)
(* record (one effect, in flight or spent: closed by a logged entry).      *)
(* Files are never edited in place: segments only grow, and the owner      *)
(* deletes whole files. Each reader sees each file through `vis`: nothing, *)
(* a readable prefix, or all of it, independently of every other file and  *)
(* every other reader.                                                     *)
(*                                                                         *)
(* The protocol rules are constants, TRUE when in force, so that a config  *)
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
    MaxRestores,    \* bound on restores of the folder, or of one directory, from its backup
    MaxDrops,       \* bound on files a sync client reverts to an earlier version
    ResumeMid,      \* a clone may resume mid-segment, appending to its own copy of it
    Certify,        \* a snapshot certifies the lines it folds of its directory's files
    Carry,          \* a snapshot carries forward the certificates of the one before
    Numbered,       \* a reader tells a gap from a run by the entry's number
    CertForks,      \* a reader finds a fork where a tip certifies an entry's predecessor
    Quarantine,     \* a reader never merges what a dismissal names, nor what follows it
    UniqueNames,    \* no two histories give a segment the same name
    SealedOnly,     \* a writer deletes only segments with a seal marker
    FoldedOnly,     \* a writer deletes only segments one of its snapshots folds
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
    drops,
    lost,       \* entries a restore took from the folder
    lostEffects,\* effects a restore took every trace of
    \* Instance state, kept in the instance's local root.
    role,       \* "writer" or "reader"
    wid,        \* the directory this instance writes
    head,       \* the last entry this instance wrote
    seg,        \* the segment it appends to, or None
    dying,      \* files it is deleting now
    lastSnap,   \* its last snapshot, or None
    nextName,   \* its segment name counter, when names are not unique
    inflight,   \* its effect in flight
    \* Reader state: `vis`, its copy of the folder, and what its last read
    \* derived from that copy alone.
    acc,        \* entries whose effects the reader has merged (a ghost)
    reported,   \* fork points it finds, as <<directory, entry>>
    shown,      \* entries it shows as runs
    dismissed,  \* entries of runs that a merged entry dismisses
    gaps,       \* entries it holds back after a gap
    dup,        \* entries its last read placed though a tip folds them
    vis         \* vis[r][f]: how much of file f reader r sees

writer == <<ents, files, nW, nEff, clones, losses, role, wid, head, seg, dying,
            lastSnap, nextName, inflight>>
reader == <<acc, reported, shown, dismissed, gaps, dup, vis>>
folder == <<backup, restores, drops, lost, lostEffects>>
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
(* writer's directory and places entries by chain alone. It keeps nothing  *)
(* between reads: each read derives everything from the files it sees.     *)

\* Where a restore shortened a segment, a longer copy shows only what the
\* restore left.
Shown(v, f) == IF v[f] < Len(files[f].ents) THEN v[f] ELSE Len(files[f].ents)
Present(v) ==
    UNION {{files[f].ents[k] : k \in 1..Shown(v, f)} : f \in {g \in Files : files[g].kind = "seg"}}
Snaps(v) == {f \in Files : files[f].kind = "snap" /\ v[f] = 1}

N(e) == ents[e].n

\* What a snapshot lets a reader recognize: its anchor and the lines it
\* certified. It chains only from its anchor. What it folds is a ghost: in
\* the code a snapshot names its live writes, not every entry it folds.
Know(s) == files[s].certs \cup {files[s].anchor}
Roots(S) == UNION {Know(s) : s \in S}
Folds(S) == UNION {files[s].folds : s \in S}

\* The tips among snapshots S: those no other snapshot of their directory
\* certifies. Of two with one anchor, the one created first.
Tips(S) ==
    {s \in S : ~\E t \in S : /\ t # s
                             /\ files[t].dir = files[s].dir
                             /\ files[s].anchor \in Know(t)
                             /\ files[t].anchor # files[s].anchor \/ t < s}
TipsIn(T, d) == {t \in T : files[t].dir = d}
Multi(T, d) == Cardinality(TipsIn(T, d)) > 1
\* What every tip of directory d folds: the part of its branches they share.
Common(T, d) == {x \in Folds(TipsIn(T, d)) : \A t \in TipsIn(T, d) : x \in files[t].folds}

\* An entry is placed when it starts a chain in a directory with no tip, or
\* its predecessor is a tip's anchor or placed. Nothing else is placed.
RECURSIVE Place(_, _, _)
Place(T, K, P) ==
    LET next == K \cup {x \in P : \/ ents[x].prev = None /\ TipsIn(T, ents[x].dir) = {}
                                  \/ ents[x].prev \in K}
    IN IF next = K THEN K ELSE Place(T, next, P)

RECURSIVE Spread(_, _)
Spread(F, H) ==
    LET next == F \cup {x \in H : ents[x].prev \in F}
    IN IF next = F THEN F ELSE Spread(next, H)

ForksIn(E) ==
    {<<ents[x].dir, ents[x].prev>> :
        x \in {y \in E : \E z \in E : z # y /\ ents[z].dir = ents[y].dir
                                          /\ ents[z].prev = ents[y].prev}}

\* Whether entry x lies beyond one of the fork points FP: past a fork, no
\* entry of that writer is merged, on either branch.
Beyond(FP, x) ==
    \E fp \in FP : fp[1] = ents[x].dir /\ (fp[2] = None \/ N(x) > N(fp[2]))

\* Where the tips of directory d part: the highest entry every one of them
\* certifies, or None when they share none.
Parting(T, d) ==
    LET shared == {x \in Roots(TipsIn(T, d)) : \A t \in TipsIn(T, d) : x \in Know(t)}
    IN IF shared = {} THEN None
       ELSE CHOOSE p \in shared : \A q \in shared : N(q) <= N(p)

\* The highest number among the anchors of the tips of d.
TipN(T, d) ==
    LET ns == {N(files[t].anchor) : t \in TipsIn(T, d)}
    IN CHOOSE m \in ns : \A k \in ns : k <= m

\* What a reader makes of what it sees. `P` is what it sees in segments, `T`
\* its tips, `K` what it places. Its fork points are two successors of one
\* entry, an entry whose predecessor a tip certified but that no tip folds,
\* and where several tips of one writer part. An entry it holds back is a gap
\* while its predecessor may come after the tip's anchor, and otherwise a run.
\* Every entry beyond a fork point, and what one tip of several folds that
\* another does not, is a run too. A dismissal in a merged entry takes the
\* entries it names out of view for good: they are never merged, and an entry
\* after one is a run.
Outcome(P, T, K) ==
    LET cert == Roots(T)
        single == {t \in T : ~Multi(T, files[t].dir)}
        multi == T \ single
        anchors == {files[t].anchor : t \in T}
        held == P \ (K \cup cert)
        FP == ForksIn(K \cup P)
                 \cup {<<d, Parting(T, d)>> : d \in {files[t].dir : t \in multi}}
                 \cup IF ~CertForks THEN {}
                       ELSE {<<ents[x].dir, ents[x].prev>> :
                                x \in {y \in held : ents[y].prev \in cert \ anchors}}
        lostBasis ==
            IF ~Numbered THEN held
            ELSE {x \in held : \/ ents[x].prev = None
                               \/ ents[x].prev \in K \cup cert
                               \/ /\ TipsIn(T, ents[x].dir) # {}
                                  /\ N(x) <= TipN(T, ents[x].dir) + 1}
        segRuns == Spread(lostBasis, held)
        tipped == Folds(single) \cup UNION {Common(T, files[t].dir) : t \in multi}
        multiRuns == (Folds(multi) \ tipped)
                         \cup {x \in K \ anchors : Multi(T, ents[x].dir)}
        candidates == K \cup Folds(T) \cup held
        forkRuns == {x \in candidates : Beyond(FP, x)}
        dis == UNION {ents[e].dis : e \in (tipped \cup K) \ (multiRuns \cup forkRuns \cup segRuns)}
        after == IF Quarantine THEN Spread(dis \cap candidates, candidates) \ dis ELSE {}
        branches == multiRuns \cup forkRuns \cup after
        runs == segRuns \cup branches
        quarantined == IF Quarantine THEN dis ELSE {}
        merged == (tipped \cup K) \ (branches \cup quarantined)
    IN [acc |-> merged,
        forks |-> FP,
        shown |-> runs \ dis,
        dismissed |-> (runs \cup candidates) \cap dis,
        gaps |-> held \ runs,
        dup |-> (K \ anchors) \cap Folds(T)]

\* What a reader places from its tips' anchors, among the entries it sees
\* that no tip certifies.
Placing(T, P) == Place(T, {files[t].anchor : t \in T}, P \ Roots(T))

\* What a reader makes of the files it sees, `v`.
View(v) ==
    LET P == Present(v)
        T == Tips(Snaps(v))
    IN Outcome(P, T, Placing(T, P))

\* A reader reads after every change it sees, as `View` says. Each step is
\* bound once, so that TLC evaluates it once when it checks fairness.
Read(r, v) ==
    \E P \in {Present(v)} : \E T \in {Tips(Snaps(v))} : \E K \in {Placing(T, P)} :
    \E new \in {Outcome(P, T, K)} :
        /\ acc' = [acc EXCEPT ![r] = new.acc]
        /\ reported' = [reported EXCEPT ![r] = new.forks]
        /\ shown' = [shown EXCEPT ![r] = new.shown]
        /\ dismissed' = [dismissed EXCEPT ![r] = new.dismissed]
        /\ gaps' = [gaps EXCEPT ![r] = new.gaps]
        /\ dup' = [dup EXCEPT ![r] = new.dup]

\* Entries the files a reader sees hold, in a segment or folded in a snapshot.
VisHolds(v) == Present(v) \cup Folds(Snaps(v))

\* Effects an entry the reader holds closes: merged, shown as a run, held back
\* after a gap, or dismissed. One closed on a branch is that branch's, shown
\* with it as an item, and never reported to settle again.
Closed(r) == {ents[x].closes : x \in acc[r] \cup shown[r] \cup gaps[r] \cup dismissed[r]} \ {0}
\* Pending records the reader sees whose effect no entry it holds closes.
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

NewFile(kind, d, name, es, anchor, fo, certs, eff) ==
    [kind |-> kind, dir |-> d, name |-> name, ents |-> es, anchor |-> anchor,
     folds |-> fo, certs |-> certs, eff |-> eff, sealed |-> FALSE, deleted |-> FALSE]

\* A segment of directory d that its writer may delete from folder fs: it
\* ends with a seal marker, and a snapshot of d in fs certifies all it holds.
MayDelete(fs, d, f) ==
    /\ SealedOnly => fs[f].sealed
    /\ FoldedOnly =>
           \E s \in 1..Len(fs) : /\ fs[s].kind = "snap"
                                /\ fs[s].dir = d
                                /\ ~fs[s].deleted
                                /\ Range(fs[f].ents) \subseteq fs[s].certs \cup {fs[s].anchor}

\* Appending to no open segment starts a new one; a fresh open never
\* appends to a segment it did not start.
AppendEntry(i, closes, dis) ==
    LET e == Len(ents) + 1
        new == seg[i] = None
        f == IF new THEN Len(files) + 1 ELSE seg[i]
    IN /\ Writable(i)
       /\ Len(ents) < MaxEntries
       /\ new => Len(files) < MaxFiles
       /\ ents' = Append(ents, [dir |-> wid[i], prev |-> head[i], closes |-> closes,
                                 n |-> IF head[i] = None THEN 0 ELSE N(head[i]) + 1,
                                 dis |-> dis, orphan |-> HeadLost(i)])
       /\ files' = IF new
                   THEN Append(files, NewFile("seg", wid[i],
                            IF UniqueNames THEN f ELSE nextName[i], <<e>>, None, {}, {}, 0))
                   ELSE [files EXCEPT ![f].ents = Append(@, e)]
       /\ nextName' = IF new /\ ~UniqueNames
                      THEN [nextName EXCEPT ![i] = @ + 1] ELSE nextName
       /\ seg' = [seg EXCEPT ![i] = f]
       /\ head' = [head EXCEPT ![i] = e]

Write(i) ==
    /\ AppendEntry(i, 0, {})
    /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying, lastSnap,
                   inflight, reader, folder, chaos>>

\* The owner seals its segment, folds its own chain into a snapshot, and
\* deletes the snapshot the new one supersedes and the segments of its
\* directory it may delete. The snapshot certifies the lines it folds of
\* every segment of its directory, and of every segment its chain was written
\* to since its last snapshot, deleted or not; it names by hash each entry
\* since then that no file holds; and it carries forward the certificates of
\* its last snapshot, which it read or wrote, and of the snapshots of its
\* directory on its chain.
Compact(i) ==
    LET sealed == IF seg[i] = None THEN files
                  ELSE [files EXCEPT ![seg[i]].sealed = TRUE]
        s == Len(files) + 1
        chain == Chain(head[i])
        tail == chain \ IF lastSnap[i] = None THEN {} ELSE files[lastSnap[i]].folds
        here == {f \in Files : sealed[f].dir = wid[i] /\ ~sealed[f].deleted}
        held == {g \in here : sealed[g].kind = "snap" /\ sealed[g].anchor \in chain}
                    \cup IF lastSnap[i] = None THEN {} ELSE {lastSnap[i]}
        carried ==
            IF ~Carry THEN {}
            ELSE UNION {sealed[f].certs \cup {sealed[f].anchor} : f \in held}
        certs ==
            IF ~Certify THEN {}
            ELSE carried
                 \cup UNION {Range(sealed[f].ents) \cap chain :
                               f \in {g \in Files : /\ sealed[g].dir = wid[i]
                                                    /\ sealed[g].kind = "seg"
                                                    /\ g \in here \/ Range(sealed[g].ents) \cap tail # {}}}
                 \cup {sealed[f].anchor :
                            f \in {g \in here : sealed[g].kind = "snap" /\ sealed[g].anchor \in chain}}
                 \cup (tail \ UNION {Range(sealed[f].ents) :
                                        f \in {g \in Files : sealed[g].kind = "seg"}})
        fs == Append(sealed, NewFile("snap", wid[i], s, <<>>, head[i], chain, certs, 0))
        old == IF lastSnap[i] = None THEN {} ELSE {lastSnap[i]}
        superseded == {f \in Files : /\ fs[f].kind = "seg"
                                     /\ fs[f].dir = wid[i]
                                     /\ ~fs[f].deleted
                                     /\ MayDelete(fs, wid[i], f)}
    IN /\ Writable(i)
       /\ head[i] # None
       /\ lastSnap[i] # None => files[lastSnap[i]].anchor # head[i]
       /\ Len(files) < MaxFiles
       /\ files' = fs
       /\ lastSnap' = [lastSnap EXCEPT ![i] = s]
       /\ seg' = [seg EXCEPT ![i] = None]
       /\ dying' = [dying EXCEPT ![i] = @ \cup superseded \cup old]
       /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, nextName,
                      inflight, reader, folder, chaos>>

\* A deletion names a file. Sync applies it to whichever file holds that
\* name, which for a segment name two histories chose may be the other's.
\* A segment the writer may no longer delete is left in place.
DeleteOne(i) ==
    /\ Writable(i)
    /\ \E f \in dying[i] :
        LET namesake == {g \in Files : /\ files[g].kind = files[f].kind
                                       /\ files[g].dir = files[f].dir
                                       /\ files[g].name = files[f].name
                                       /\ ~files[g].deleted}
            kept == files[f].kind = "seg" /\ ~MayDelete(files, wid[i], f)
        IN /\ CheckFirst => ~files[lastSnap[i]].deleted
           /\ dying' = [dying EXCEPT ![i] = @ \ {f}]
           /\ IF namesake = {} \/ kept THEN UNCHANGED files
              ELSE \E g \in namesake : files' = [files EXCEPT ![g].deleted = TRUE]
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg,
                   lastSnap, nextName, inflight, reader, folder, chaos>>

\* No effect of this writer is open.
Idle(i) == inflight[i] = NoEffect \/ inflight[i].logged

\* A multi-step effect: write its pending record, then log the entry that
\* closes it. The record is then spent, and its writer deletes it before it
\* begins another effect.
Begin(i) ==
    LET f == Len(files) + 1 IN
    /\ Writable(i)
    /\ inflight[i] = NoEffect
    /\ nEff < MaxEffects
    /\ Len(files) < MaxFiles
    /\ files' = Append(files, NewFile("pend", wid[i], f, <<>>, None, {}, {}, nEff + 1))
    /\ inflight' = [inflight EXCEPT ![i] = [eff |-> nEff + 1, rec |-> f, logged |-> FALSE]]
    /\ nEff' = nEff + 1
    /\ UNCHANGED <<ents, nW, clones, losses, role, wid, head, seg, dying,
                   lastSnap, nextName, reader, folder, chaos>>

Close(i) ==
    /\ inflight[i].eff # 0
    /\ ~inflight[i].logged
    /\ AppendEntry(i, inflight[i].eff, {})
    /\ inflight' = [inflight EXCEPT ![i].logged = TRUE]
    /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying, lastSnap,
                   reader, folder, chaos>>

Unpend(i) ==
    /\ Writable(i)
    /\ inflight[i].logged
    /\ files' = [files EXCEPT ![inflight[i].rec].deleted = TRUE]
    /\ inflight' = [inflight EXCEPT ![i] = NoEffect]
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg, dying,
                   lastSnap, nextName, reader, folder, chaos>>

\* With the user's consent, a writer closes an effect it reports by logging
\* the closing entry in its own directory. The record stays: only its owner
\* deletes it. Consent is the choice to take this step.
Settle(i) ==
    \E e \in Reported(i) :
        /\ AppendEntry(i, e, {})
        /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying,
                       lastSnap, inflight, reader, folder, chaos>>

\* The user dismisses the runs a writer's reader shows, by an entry naming
\* them. Taking a run's writes instead is an ordinary write.
Dismiss(i) ==
    /\ shown[i] # {}
    /\ AppendEntry(i, 0, shown[i])
    /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying, lastSnap,
                   inflight, reader, folder, chaos>>

\* A copied local root (a restored backup, a cloned disk, copied app data)
\* opens on another machine as the same writer with the same history. A
\* writer's local root exists only once its first entry is durable. A copy
\* resumed mid-segment, as a virtual machine resumed twice, keeps appending to
\* the segment it had open; sync keeps the two versions as a conflicted copy.
Clone(i, j) ==
    /\ role[i] = "writer"
    /\ head[i] # None
    /\ role[j] = "reader"
    /\ clones < MaxClones
    /\ clones' = clones + 1
    /\ role' = [role EXCEPT ![j] = "writer"]
    /\ wid' = [wid EXCEPT ![j] = wid[i]]
    /\ head' = [head EXCEPT ![j] = head[i]]
    /\ \E mid \in IF ResumeMid /\ seg[i] # None /\ Len(files) < MaxFiles
                   THEN BOOLEAN ELSE {FALSE} :
        LET f == Len(files) + 1
        IN /\ seg' = [seg EXCEPT ![j] = IF mid THEN f ELSE None]
           /\ files' = IF mid THEN Append(files, [files[seg[i]] EXCEPT !.name = f])
                      ELSE files
    /\ dying' = [dying EXCEPT ![j] = dying[i]]
    /\ lastSnap' = [lastSnap EXCEPT ![j] = lastSnap[i]]
    /\ nextName' = [nextName EXCEPT ![j] = nextName[i]]
    /\ inflight' = [inflight EXCEPT ![j] = inflight[i]]
    /\ UNCHANGED <<ents, nW, nEff, losses, reader, folder, chaos>>

\* A process that quits seals its segment with a durable marker. The next
\* open starts a new segment.
Quit(i) ==
    /\ Writable(i)
    /\ seg[i] # None
    /\ files' = [files EXCEPT ![seg[i]].sealed = TRUE]
    /\ seg' = [seg EXCEPT ![i] = None]
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, dying,
                   lastSnap, nextName, inflight, reader, folder, chaos>>

\* A crash leaves its segment without a seal marker.
Crash(i) ==
    /\ role[i] = "writer"
    /\ seg[i] # None
    /\ seg' = [seg EXCEPT ![i] = None]
    /\ UNCHANGED <<ents, files, nW, nEff, clones, losses, role, wid, head,
                   dying, lastSnap, nextName, inflight, reader, folder,
                   chaos>>

\* Instance i continues under a new writer id and never writes, compacts or
\* deletes a spent record of its old directory again.
Renew(i) ==
    /\ nW < MaxWriters
    /\ nW' = nW + 1
    /\ wid' = [wid EXCEPT ![i] = nW + 1]
    /\ head' = [head EXCEPT ![i] = None]
    /\ seg' = [seg EXCEPT ![i] = None]
    /\ dying' = [dying EXCEPT ![i] = {}]
    /\ lastSnap' = [lastSnap EXCEPT ![i] = None]
    /\ nextName' = [nextName EXCEPT ![i] = 0]
    /\ inflight' = [inflight EXCEPT ![i] = NoEffect]

\* Losing the local root loses the effect in flight; the next open is a new
\* writer.
LoseRoot(i) ==
    /\ role[i] = "writer"
    /\ losses < MaxLosses
    /\ losses' = losses + 1
    /\ Renew(i)
    /\ UNCHANGED <<ents, files, nEff, clones, role, reader, folder, chaos>>

\* An instance that sees an entry of its own directory with two successors
\* takes a new writer id.
LeaveFork(i) ==
    /\ role[i] = "writer"
    /\ Idle(i)
    /\ \E fk \in reported[i] : fk[1] = wid[i]
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
    /\ UNCHANGED <<writer, reader, restores, drops, lost, lostEffects, chaos>>

\* The folder becomes fs, and what it no longer holds is lost.
Becomes(fs) ==
    LET traced == {files[f].eff : f \in {g \in Files : fs[g].kind = "pend" /\ ~fs[g].deleted}}
                      \cup {ents[e].closes : e \in Holds(fs)}
    IN /\ files' = fs
       /\ lost' = lost \cup (All \ Holds(fs))
       /\ lostEffects' = lostEffects \cup ((1..nEff) \ traced)

\* The folder as backup b holds it, in the directories `ds`.
BackedUp(b, ds) ==
    [f \in Files |-> IF files[f].dir \notin ds THEN files[f]
                     ELSE IF f <= Len(b) THEN b[f]
                     ELSE [files[f] EXCEPT !.deleted = TRUE]]

Restore ==
    /\ restores < MaxRestores
    /\ \E b \in backup : Becomes(BackedUp(b, 0..MaxWriters))
    /\ restores' = restores + 1
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg, dying,
                   lastSnap, nextName, inflight, reader, backup, drops, chaos>>

\* One writer's directory alone goes back to its backup.
RestoreDir ==
    /\ restores < MaxRestores
    /\ \E b \in backup : \E d \in 1..nW : Becomes(BackedUp(b, {d}))
    /\ restores' = restores + 1
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg, dying,
                   lastSnap, nextName, inflight, reader, backup, drops, chaos>>

\* A sync client keeps an earlier version of one segment: a shorter prefix,
\* without a seal marker.
DropVersion ==
    /\ drops < MaxDrops
    /\ \E f \in Files : \E k \in 1..(Len(files[f].ents) - 1) :
        /\ files[f].kind = "seg"
        /\ ~files[f].deleted
        /\ Becomes([files EXCEPT ![f].ents = SubSeq(@, 1, k), ![f].sealed = FALSE])
    /\ drops' = drops + 1
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg, dying,
                   lastSnap, nextName, inflight, reader, backup, restores, chaos>>

-----------------------------------------------------------------------------
(* Sync. Each file reaches each reader independently. A file may show any  *)
(* prefix of what was appended, grow to all of it, arrive after its own    *)
(* deletion, reappear after it, and stay. Hiding or shortening a file,     *)
(* other than by delivering its deletion or a restore, spends the chaos    *)
(* budget.                                                                 *)

Show(r, f, k) ==
    /\ vis' = [vis EXCEPT ![r][f] = k]
    /\ Read(r, vis'[r])
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
    /\ drops = 0
    /\ lost = {}
    /\ lostEffects = {}
    /\ role = [i \in Instances |-> IF i = First THEN "writer" ELSE "reader"]
    /\ wid = [i \in Instances |-> IF i = First THEN 1 ELSE 0]
    /\ head = [i \in Instances |-> None]
    /\ seg = [i \in Instances |-> None]
    /\ dying = [i \in Instances |-> {}]
    /\ lastSnap = [i \in Instances |-> None]
    /\ nextName = [i \in Instances |-> 0]
    /\ inflight = [i \in Instances |-> NoEffect]
    /\ acc = [i \in Instances |-> {}]
    /\ reported = [i \in Instances |-> {}]
    /\ shown = [i \in Instances |-> {}]
    /\ dismissed = [i \in Instances |-> {}]
    /\ gaps = [i \in Instances |-> {}]
    /\ dup = [i \in Instances |-> {}]
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
        \/ Quit(i)
        \/ Crash(i)
        \/ LoseRoot(i)
        \/ LeaveFork(i)
        \/ LeaveRestored(i)
        \/ Dismiss(i)
        \/ \E j \in Instances : Clone(i, j)
    \/ Backup
    \/ Restore
    \/ RestoreDir
    \/ DropVersion
    \/ \E r \in Instances : Deliver(r)

\* Writers may stop at any time; sync eventually delivers every file.
Spec == Init /\ [][Next]_vars /\ \A r \in Instances : WF_vars(DeliverAll(r))

-----------------------------------------------------------------------------
(* Safety.                                                                 *)

\* A reader never accepts an entry before its predecessor.
ChainOrder ==
    \A r \in Instances : \A e \in acc[r] :
        ents[e].prev = None \/ ents[e].prev \in acc[r]

\* Every entry a reader sees is merged, held back as a gap and reported,
\* shown as a run, or dismissed; a file under any name, including a sync
\* conflicted copy, is read.
NothingIgnored ==
    \A r \in Instances :
        Present(vis[r]) \subseteq acc[r] \cup gaps[r] \cup shown[r] \cup dismissed[r]

\* What a reader shows as a run it does not merge.
ShownApart == \A r \in Instances : shown[r] \cap acc[r] = {}

\* A reader never places an entry from a file when a tip already folds it:
\* merging it again could bring back what it replaced.
NoReapply == \A r \in Instances : dup[r] = {}

\* Entry x lies on a branch: its chain forked at or before it.
Forked(x) ==
    \E a \in Chain(x) : \E b \in All :
        b # a /\ ents[b].dir = ents[a].dir /\ ents[b].prev = ents[a].prev

\* An entry is shown as a run only when its chain forked at or before it, or
\* a restore or a dropped version took it.
NoSpuriousConflict ==
    \A r \in Instances : \A x \in shown[r] : Forked(x) \/ x \in lost

\* Once every file has reached a reader, it merges every kept entry that no
\* fork precedes and that follows nothing a restore or a dropped version
\* took.
UnforkedMerged ==
    \A r \in Instances : Delivered(r) =>
        \A x \in Kept : ~Forked(x) /\ Chain(x) \cap lost = {} => x \in acc[r]

\* A writer never appends after an entry the folder no longer holds.
NoOrphan == \A e \in All : ~ents[e].orphan

\* The folder as a reader sees it once every file and every deletion has
\* reached it.
Folder == [f \in 1..MaxFiles |-> IF f \in Files /\ ~files[f].deleted THEN FileLen(f) ELSE 0]

\* The entries reader r's last read accounted for.
Holding(r) == acc[r] \cup shown[r] \cup dismissed[r] \cup gaps[r]

\* The files the reader last read hold what a restore or a dropped version
\* took.
Stale(r) == Holding(r) \cap lost # {}

\* Once every file has reached a reader, it merges, shows, dismisses and holds
\* back what a reader that sees exactly the folder does, and finds the same
\* fork points, whatever deleted files it still sees, unless the files it
\* read hold what a restore or a dropped version took.
Agree ==
    \A r \in Instances : Delivered(r) /\ ~Stale(r) =>
        LET F == View(Folder) IN
        /\ acc[r] = F.acc
        /\ shown[r] = F.shown
        /\ dismissed[r] = F.dismissed
        /\ gaps[r] = F.gaps
        /\ reported[r] = F.forks

\* A reader never merges two branches of one writer.
NoMergedFork == \A r \in Instances : ForksIn(acc[r]) = {}

\* A reader never merges an entry a dismissal it merged names. Once every
\* file has reached it, it neither shows nor merges an entry a kept, merged
\* dismissal names.
DismissalsHold ==
    \A r \in Instances :
        /\ acc[r] \cap UNION {ents[e].dis : e \in acc[r]} = {}
        /\ Delivered(r) =>
               (shown[r] \cup acc[r]) \cap UNION {ents[e].dis : e \in Kept \cap acc[r]} = {}

\* A writer creates, appends to and deletes files only in its own directory.
OwnDirectory ==
    \A i \in Instances : role[i] = "writer" =>
        \A f \in dying[i] \cup ({seg[i], lastSnap[i], inflight[i].rec} \ {None}) :
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

\* Once every file has reached a reader, it merges, shows or has dismissed
\* every entry the folder keeps, whatever the delivery order, holds back only
\* what follows an entry a restore or a dropped version took, and reports
\* every fork among the kept entries it sees.
AfterLost == {x \in All : Chain(x) \cap lost # {}}
Converged(r) ==
    /\ Kept \subseteq acc[r] \cup shown[r] \cup dismissed[r] \cup (gaps[r] \cap AfterLost)
    /\ gaps[r] \subseteq AfterLost
    /\ ForksIn(Kept \cap Present(vis[r])) \subseteq reported[r]
DeliveredConverges == \A r \in Instances : Delivered(r) => Converged(r)

\* Once every file has reached a reader, every effect begun is closed,
\* reported, or the reader's own effect in flight, unless a restore took it.
Settled(r) ==
    \A x \in (1..nEff) \ lostEffects :
        x \in Closed(r) \cup Reported(r) \cup {inflight[r].eff}
DeliveredSettles == \A r \in Instances : Delivered(r) => Settled(r)

\* A step in which no file leaves reader r's view or shrinks in it, and after
\* which every entry its last read accounted for is still in a file it sees.
Arriving(r) ==
    /\ \A f \in 1..MaxFiles : vis'[r][f] >= vis[r][f]
    /\ Holding(r) \subseteq VisHolds(vis'[r])

\* As files arrive, a reader stops merging an entry only when it now shows it
\* or has dismissed it.
Monotone ==
    [][\A r \in Instances : Arriving(r) =>
           acc[r] \subseteq acc'[r] \cup shown'[r] \cup dismissed'[r]]_vars

\* As files arrive, a run stays shown until it is dismissed.
NoFlip ==
    [][\A r \in Instances : Arriving(r) =>
           shown[r] \subseteq shown'[r] \cup dismissed'[r]]_vars

\* A writer takes a new id for a fork only when an entry of its directory has
\* two successors.
OwnFileNoRetire ==
    [][\A i \in Instances :
           (role[i] = "writer" /\ wid[i] # 0 /\ wid'[i] # wid[i] /\ losses' = losses /\ ~HeadLost(i))
               => \E x, y \in All : x # y /\ ents[x].dir = wid[i] /\ ents[y].dir = wid[i]
                                       /\ ents[x].prev = ents[y].prev]_vars

-----------------------------------------------------------------------------
(* Liveness.                                                               *)

\* Once writes stop, every reader converges and settles or reports every
\* effect, including those of a writer whose local root was lost.
Convergence == \A r \in Instances : <>[](Converged(r) /\ Settled(r))

=============================================================================
