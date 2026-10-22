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
(* also holds its number in its chain, 0 for the first.                    *)
(*                                                                         *)
(* A file is a segment (a growing sequence of entries, which the process   *)
(* writing it seals when it closes it), a snapshot (anchored at its last   *)
(* entry, it reflects the chain it folds but lets a reader chain only from *)
(* its last `Window` entries and the lines it certified) or a pending      *)
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
    MaxRestores,    \* bound on restores of the folder from its backup
    Window,         \* how many of a chain's last entries a snapshot lets a reader chain from
    Certify,        \* a snapshot certifies the lines it folds of its directory's files
    Numbered,       \* a reader places an entry only within its window's reach
    Classify,       \* a reader shows an entry whose predecessor it forgot
    Grace,          \* a reader may drop what no file it sees holds
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
    spent,      \* records of its effects closed since, not yet deleted
    \* Reader state: its cached view, also in the local root, and `vis`, its
    \* copy of the folder.
    acc,        \* entries whose effects the reader has merged (a ghost)
    known,      \* entries it remembers: those in flight
    snaps,      \* the latest snapshot it accepted of each directory
    reported,   \* forks it has reported
    shown,      \* entries it shows as written on a basis it forgot
    gaps,       \* entries it holds back after a gap
    dup,        \* entries its last read placed though an accepted snapshot folds them
    vis         \* vis[r][f]: how much of file f reader r sees

writer == <<ents, files, nW, nEff, clones, losses, role, wid, head, seg, dying,
            lastSnap, nextName, inflight, spent>>
reader == <<acc, known, snaps, reported, shown, gaps, dup, vis>>
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

N(e) == ents[e].n

\* What a snapshot lets a reader chain from: its anchor, its window and the
\* entries of the files it certified. What it folds is a ghost no reader can
\* list.
Know(s) == files[s].win \cup files[s].certs \cup {files[s].anchor}
Roots(S) == UNION {Know(s) : s \in S}
Folds(S) == UNION {files[s].folds : s \in S}

\* Snapshots are ranked by the number of their anchor, then by its hash. The
\* main snapshot of each directory among S is the one ranked highest.
Rank(s) == N(files[s].anchor) * (MaxEntries + 1) + files[s].anchor
Mains(S) ==
    [d \in {files[s].dir : s \in S} |->
        CHOOSE s \in S : files[s].dir = d /\ \A t \in S : files[t].dir = d => Rank(t) <= Rank(s)]
\* A reader accepts each directory's main snapshot, and every snapshot whose
\* anchor the main one lets it chain from or K holds.
Accepted(S, K) ==
    LET m == Mains(S) IN
    {s \in S : s = m[files[s].dir] \/ files[s].anchor \in Know(m[files[s].dir]) \cup K}

\* An entry is within reach unless an accepted snapshot of its directory
\* may fold it out of sight: its number is at or below the bottom of the
\* main snapshot's window. `B` maps each directory with one to the number
\* of that snapshot's anchor.
Bases(A) == LET m == Mains(A) IN [d \in DOMAIN m |-> N(files[m[d]].anchor)]
Reach(B, x) ==
    \/ ~Numbered
    \/ ents[x].dir \notin DOMAIN B
    \/ N(x) + Window > B[ents[x].dir]

\* An entry is placed when it starts a chain or its predecessor is placed,
\* and it is within reach. Nothing else is placed.
RECURSIVE Place(_, _, _)
Place(B, K, P) ==
    LET next == K \cup {x \in P : /\ ents[x].prev = None \/ ents[x].prev \in K
                                  /\ Reach(B, x)}
    IN IF next = K THEN K ELSE Place(B, next, P)

RECURSIVE Spread(_, _)
Spread(F, H) ==
    LET next == F \cup {x \in H : ents[x].prev \in F}
    IN IF next = F THEN F ELSE Spread(next, H)

ForksIn(E) ==
    {<<ents[x].dir, ents[x].prev>> :
        x \in {y \in E : \E z \in E : z # y /\ ents[z].dir = ents[y].dir
                                          /\ ents[z].prev = ents[y].prev}}

\* An entry a reader holds back is written on a forgotten basis when it is
\* out of reach, or its predecessor is held by nothing and at or below the
\* bottom of the window, or so is an entry before it.
Forgot(held, K, B) ==
    LET lostBasis ==
            {x \in held : \/ ents[x].prev = None
                          \/ ents[x].prev \in K
                          \/ /\ ents[x].dir \in DOMAIN B
                             /\ N(x) - 1 + Window <= B[ents[x].dir]}
    IN IF Classify THEN Spread(lostBasis, held) ELSE {}
Unplaced(v, A, folded, K) ==
    IF Classify THEN Folds(Snaps(v) \ A) \ (folded \cup K) ELSE {}
Outcome(P, a0, k0, A, K, folded, held, forgot, unplaced) ==
    [acc |-> (a0 \ (forgot \cup unplaced)) \cup folded \cup K,
     known |-> {x \in K : x \notin folded \cup P},
     snaps |-> LET m == Mains(A) IN {m[d] : d \in DOMAIN m},
     forks |-> ForksIn(K \cup P \cup UNION {files[s].win : s \in A}),
     shown |-> forgot \cup unplaced,
     gaps |-> held \ forgot,
     dup |-> (K \ (k0 \cup Roots(A))) \cap folded]

\* What a reader makes of what it sees, `v`, and its cached view: what it
\* merged (`a0`), the entries it remembers (`k0`) and the snapshots it
\* accepted (`s0`). It places the entries of the files it sees afresh from
\* what the snapshots let it chain from and what it remembers, and
\* remembers only the entries in flight: placed, folded by no snapshot it
\* accepted, and held by no file it sees. What it now shows as written on a
\* forgotten basis it no longer merges.
Read(v, a0, k0, s0) ==
    LET P == Present(v)
        S == Snaps(v) \cup s0
        A1 == Accepted(S, k0)
        K1 == Place(Bases(A1), k0 \cup Roots(A1), P)
        A == Accepted(S, K1)
        B == Bases(A)
        K == Place(B, K1 \cup Roots(A), P)
        folded == Folds(A)
        held == P \ K
    IN Outcome(P, a0, k0, A, K, folded, held, Forgot(held, K, B), Unplaced(v, A, folded, K))

\* A reader reads after every change it sees, as `Read` says. Each step is
\* bound once, so that TLC evaluates it once when it checks fairness.
ReadWith(r, v, a0, k0, s0, f0) ==
    \E P \in {Present(v)} : \E S \in {Snaps(v) \cup s0} :
    \E A1 \in {Accepted(S, k0)} :
    \E K1 \in {Place(Bases(A1), k0 \cup Roots(A1), P)} :
    \E A \in {Accepted(S, K1)} : \E B \in {Bases(A)} :
    \E K \in {Place(B, K1 \cup Roots(A), P)} :
    \E folded \in {Folds(A)} : \E held \in {P \ K} :
    \E forgot \in {Forgot(held, K, B)} : \E unplaced \in {Unplaced(v, A, folded, K)} :
    \E new \in {Outcome(P, a0, k0, A, K, folded, held, forgot, unplaced)} :
        /\ acc' = [acc EXCEPT ![r] = new.acc]
        /\ known' = [known EXCEPT ![r] = new.known]
        /\ snaps' = [snaps EXCEPT ![r] = new.snaps]
        /\ reported' = [reported EXCEPT ![r] = f0 \cup new.forks]
        /\ shown' = [shown EXCEPT ![r] = new.shown]
        /\ gaps' = [gaps EXCEPT ![r] = new.gaps]
        /\ dup' = [dup EXCEPT ![r] = new.dup]

\* What a reader without a cached view makes of what it sees.
Fresh(v) == Read(v, {}, {}, {})

\* Entries the files a reader sees hold, in a segment or folded in a snapshot.
VisHolds(v) == Present(v) \cup Folds(Snaps(v))

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

NewFile(kind, d, name, es, anchor, fo, win, certs, eff) ==
    [kind |-> kind, dir |-> d, name |-> name, ents |-> es, anchor |-> anchor,
     folds |-> fo, win |-> win, certs |-> certs, eff |-> eff, sealed |-> FALSE,
     deleted |-> FALSE]

\* A segment of directory d that its writer may delete from folder fs: it
\* ends with a seal marker, and a snapshot of d in fs certifies all it holds.
MayDelete(fs, d, f) ==
    /\ SealedOnly => fs[f].sealed
    /\ FoldedOnly =>
           \E s \in 1..Len(fs) : /\ fs[s].kind = "snap"
                                /\ fs[s].dir = d
                                /\ ~fs[s].deleted
                                /\ Range(fs[f].ents) \subseteq
                                       fs[s].win \cup fs[s].certs \cup {fs[s].anchor}

\* Appending to no open segment starts a new one; a fresh open never
\* appends to a segment it did not start.
AppendEntry(i, closes) ==
    LET e == Len(ents) + 1
        new == seg[i] = None
        f == IF new THEN Len(files) + 1 ELSE seg[i]
    IN /\ Writable(i)
       /\ Len(ents) < MaxEntries
       /\ new => Len(files) < MaxFiles
       /\ ents' = Append(ents, [dir |-> wid[i], prev |-> head[i], closes |-> closes,
                                 n |-> IF head[i] = None THEN 0 ELSE N(head[i]) + 1])
       /\ files' = IF new
                   THEN Append(files, NewFile("seg", wid[i],
                            IF UniqueNames THEN f ELSE nextName[i], <<e>>, None, {}, {}, {}, 0))
                   ELSE [files EXCEPT ![f].ents = Append(@, e)]
       /\ nextName' = IF new /\ ~UniqueNames
                      THEN [nextName EXCEPT ![i] = @ + 1] ELSE nextName
       /\ seg' = [seg EXCEPT ![i] = f]
       /\ head' = [head EXCEPT ![i] = e]

Write(i) ==
    /\ AppendEntry(i, 0)
    /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying, lastSnap,
                   inflight, spent, reader, folder, chaos>>

\* The owner seals its segment, folds its own chain into a snapshot, and
\* deletes the snapshot the new one supersedes and the segments of its
\* directory it may delete. The snapshot keeps the chain's last `Window`
\* entries, and certifies the lines of the files of its directory that it
\* folds.
Compact(i) ==
    LET sealed == IF seg[i] = None THEN files
                  ELSE [files EXCEPT ![seg[i]].sealed = TRUE]
        s == Len(files) + 1
        chain == Chain(head[i])
        here == {f \in Files : sealed[f].dir = wid[i] /\ ~sealed[f].deleted}
        certs ==
            IF ~Certify THEN {}
            ELSE UNION {Range(sealed[f].ents) \cap chain :
                            f \in {g \in here : sealed[g].kind = "seg"}}
                 \cup {sealed[f].anchor :
                            f \in {g \in here : sealed[g].kind = "snap" /\ sealed[g].anchor \in chain}}
        win == {e \in chain : N(e) + Window > N(head[i])}
        fs == Append(sealed, NewFile("snap", wid[i], s, <<>>, head[i], chain, win, certs, 0))
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
                      inflight, spent, reader, folder, chaos>>

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
                   lastSnap, nextName, inflight, spent, reader, folder, chaos>>

\* A multi-step effect: write its pending record, then log the entry that
\* closes it. The record is then spent, and its writer deletes it at any
\* later time, after beginning other effects or never.
Spent(i) == spent[i] \cup IF inflight[i].logged THEN {inflight[i].rec} ELSE {}

\* No effect of this writer is open.
Idle(i) == inflight[i] = NoEffect \/ inflight[i].logged

Begin(i) ==
    LET f == Len(files) + 1 IN
    /\ Writable(i)
    /\ Idle(i)
    /\ nEff < MaxEffects
    /\ Len(files) < MaxFiles
    /\ files' = Append(files, NewFile("pend", wid[i], f, <<>>, None, {}, {}, {}, nEff + 1))
    /\ inflight' = [inflight EXCEPT ![i] = [eff |-> nEff + 1, rec |-> f, logged |-> FALSE]]
    /\ spent' = [spent EXCEPT ![i] = Spent(i)]
    /\ nEff' = nEff + 1
    /\ UNCHANGED <<ents, nW, clones, losses, role, wid, head, seg, dying,
                   lastSnap, nextName, reader, folder, chaos>>

Close(i) ==
    /\ inflight[i].eff # 0
    /\ ~inflight[i].logged
    /\ AppendEntry(i, inflight[i].eff)
    /\ inflight' = [inflight EXCEPT ![i].logged = TRUE]
    /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying, lastSnap,
                   spent, reader, folder, chaos>>

Unpend(i) ==
    /\ Writable(i)
    /\ \E f \in Spent(i) :
        /\ files' = [files EXCEPT ![f].deleted = TRUE]
        /\ spent' = [spent EXCEPT ![i] = @ \ {f}]
        /\ inflight' = IF f = inflight[i].rec
                       THEN [inflight EXCEPT ![i] = NoEffect] ELSE inflight
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg, dying,
                   lastSnap, nextName, reader, folder, chaos>>

\* With the user's consent, a writer closes an effect it reports by logging
\* the closing entry in its own directory. The record stays: only its owner
\* deletes it. Consent is the choice to take this step.
Settle(i) ==
    \E e \in Reported(i) :
        /\ AppendEntry(i, e)
        /\ UNCHANGED <<nW, nEff, clones, losses, role, wid, dying,
                       lastSnap, inflight, spent, reader, folder, chaos>>

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
    /\ dying' = [dying EXCEPT ![j] = dying[i]]
    /\ lastSnap' = [lastSnap EXCEPT ![j] = lastSnap[i]]
    /\ nextName' = [nextName EXCEPT ![j] = nextName[i]]
    /\ inflight' = [inflight EXCEPT ![j] = inflight[i]]
    /\ spent' = [spent EXCEPT ![j] = spent[i]]
    /\ ReadWith(j, vis[j], acc[i], known[i], snaps[i], reported[i])
    /\ UNCHANGED <<ents, files, nW, nEff, losses, vis, folder, chaos>>

\* A process that quits seals its segment with a durable marker. The next
\* open starts a new segment.
Quit(i) ==
    /\ Writable(i)
    /\ seg[i] # None
    /\ files' = [files EXCEPT ![seg[i]].sealed = TRUE]
    /\ seg' = [seg EXCEPT ![i] = None]
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, dying,
                   lastSnap, nextName, inflight, spent, reader, folder, chaos>>

\* A crash leaves its segment without a seal marker.
Crash(i) ==
    /\ role[i] = "writer"
    /\ seg[i] # None
    /\ seg' = [seg EXCEPT ![i] = None]
    /\ UNCHANGED <<ents, files, nW, nEff, clones, losses, role, wid, head,
                   dying, lastSnap, nextName, inflight, spent, reader, folder,
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
    /\ spent' = [spent EXCEPT ![i] = {}]

\* Losing the local root loses the cached view and the effect in flight;
\* the next open is a new writer that reads the folder from scratch.
LoseRoot(i) ==
    /\ role[i] = "writer"
    /\ losses < MaxLosses
    /\ losses' = losses + 1
    /\ Renew(i)
    /\ ReadWith(i, vis[i], {}, {}, {}, {})
    /\ UNCHANGED <<ents, files, nEff, clones, role, vis, folder, chaos>>

\* An instance that sees its own directory forked, or holding an entry it
\* shows as written on a forgotten basis, takes a new writer id.
LeaveFork(i) ==
    /\ role[i] = "writer"
    /\ Idle(i)
    /\ \/ \E fk \in reported[i] : fk[1] = wid[i]
       \/ \E x \in shown[i] : ents[x].dir = wid[i]
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
    /\ UNCHANGED <<ents, nW, nEff, clones, losses, role, wid, head, seg, dying,
                   lastSnap, nextName, inflight, spent, reader, backup, chaos>>

-----------------------------------------------------------------------------
(* Sync. Each file reaches each reader independently. A file may show any  *)
(* prefix of what was appended, grow to all of it, arrive after its own    *)
(* deletion, reappear after it, and stay. Hiding or shortening a file,     *)
(* other than by delivering its deletion or a restore, spends the chaos    *)
(* budget.                                                                 *)

Show(r, f, k) ==
    /\ vis' = [vis EXCEPT ![r][f] = k]
    /\ ReadWith(r, vis'[r], acc[r], known[r], snaps[r], reported[r])
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

\* A reader whose view reflects an entry no file it sees holds, past the
\* grace period, derives its view again from the files it sees. The model
\* allows this at any such time.
Forget(r) ==
    /\ Grace
    /\ ~(acc[r] \subseteq VisHolds(vis[r]))
    /\ ReadWith(r, vis[r], {}, {}, {}, reported[r])
    /\ UNCHANGED <<writer, folder, vis, chaos>>

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
    /\ dying = [i \in Instances |-> {}]
    /\ lastSnap = [i \in Instances |-> None]
    /\ nextName = [i \in Instances |-> 0]
    /\ inflight = [i \in Instances |-> NoEffect]
    /\ spent = [i \in Instances |-> {}]
    /\ acc = [i \in Instances |-> {}]
    /\ known = [i \in Instances |-> {}]
    /\ snaps = [i \in Instances |-> {}]
    /\ reported = [i \in Instances |-> {}]
    /\ shown = [i \in Instances |-> {}]
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
        \/ \E j \in Instances : Clone(i, j)
    \/ Backup
    \/ Restore
    \/ \E r \in Instances : Deliver(r) \/ Forget(r)

\* Writers may stop at any time; sync eventually delivers every file.
Spec == Init /\ [][Next]_vars /\ \A r \in Instances : WF_vars(DeliverAll(r))

-----------------------------------------------------------------------------
(* Safety.                                                                 *)

\* A reader never accepts an entry before its predecessor.
ChainOrder ==
    \A r \in Instances : \A e \in acc[r] :
        ents[e].prev = None \/ ents[e].prev \in acc[r]

\* Every entry a reader sees is merged, held back as a gap and reported, or
\* shown as written on a forgotten basis; a file under any name, including
\* a sync conflicted copy, is read.
NothingIgnored ==
    \A r \in Instances : Present(vis[r]) \subseteq acc[r] \cup gaps[r] \cup shown[r]

\* A reader never places an entry from a file when a snapshot it accepted
\* already folds it: merging it again could bring back what it replaced.
NoReapply == \A r \in Instances : dup[r] = {}

\* An entry is shown as written on a forgotten basis only when its chain
\* forked at or before it, when the reader remembers a snapshot a restore
\* took, or when every file that shows it to the reader has been deleted:
\* every segment it sees holding it, and, unless it merged the entry, every
\* snapshot it sees folding it.
Forked(x) ==
    \E a \in Chain(x) : \E b \in All :
        b # a /\ ents[b].dir = ents[a].dir /\ ents[b].prev = ents[a].prev
InSegment(v, f, x) ==
    files[f].kind = "seg" /\ x \in {files[f].ents[k] : k \in 1..Shown(v, f)}
NoSpuriousConflict ==
    \A r \in Instances : \A x \in shown[r] :
        \/ Forked(x)
        \/ Folds(snaps[r]) \cap lost # {}
        \/ /\ \A f \in Files : InSegment(vis[r], f, x) => files[f].deleted
           /\ x \notin acc[r] =>
                  \A f \in Snaps(vis[r]) : x \in files[f].folds => files[f].deleted

\* A writer creates, appends to and deletes files only in its own directory.
OwnDirectory ==
    \A i \in Instances : role[i] = "writer" =>
        \A f \in dying[i] \cup spent[i]
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

\* Once every file has reached a reader, it merges or shows every entry the
\* folder keeps, whatever the delivery order, holds back only what a restore
\* took, and reports every fork among the kept entries it merged. A reader
\* without a cached view merges or shows the same.
Converged(r) ==
    /\ Kept \subseteq acc[r] \cup shown[r]
    /\ gaps[r] \subseteq lost
    /\ ForksIn(Kept \cap acc[r]) \subseteq reported[r]
DeliveredConverges ==
    \A r \in Instances : Delivered(r) =>
        /\ Converged(r)
        /\ Kept \subseteq Fresh(vis[r]).acc \cup Fresh(vis[r]).shown

\* Once every file has reached a reader, every effect begun is closed,
\* reported, or the reader's own effect in flight, unless a restore took it.
Settled(r) ==
    \A x \in (1..nEff) \ lostEffects :
        x \in Closed(r) \cup Reported(r) \cup {inflight[r].eff}
DeliveredSettles == \A r \in Instances : Delivered(r) => Settled(r)

\* A reader stops merging an entry only when no file it sees holds it, or
\* when it now holds it back or shows it as written on a forgotten basis,
\* unless its local root is lost or replaced by a copy. A fork is reported
\* when it first appears, so it is reported once.
LocalRootKept(r) == ~((losses' > losses \/ clones' > clones) /\ wid'[r] # wid[r])
Monotone ==
    [][\A r \in Instances : LocalRootKept(r) =>
           acc[r] \ acc'[r] \subseteq (All \ VisHolds(vis'[r])) \cup shown'[r] \cup gaps'[r]]_vars
ForksKept ==
    [][\A r \in Instances : LocalRootKept(r) => reported[r] \subseteq reported'[r]]_vars

\* False: without a cached view, delivery order can hide entries a reader
\* has shown, which is why readers keep one.
FreshMonotone ==
    [][\A r \in Instances : Fresh(vis[r]).acc \subseteq Fresh(vis'[r]).acc]_vars

-----------------------------------------------------------------------------
(* Liveness.                                                               *)

\* Once writes stop, every reader converges and settles or reports every
\* effect, including those of a writer whose local root was lost.
Convergence == \A r \in Instances : <>[](Converged(r) /\ Settled(r))

=============================================================================
