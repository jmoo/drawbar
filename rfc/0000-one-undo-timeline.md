# RFC-0000: One undo timeline for files and edits

- Start date: 2026-11-22
- Authors: jmoo
- RFC PR: [jmoo/drawbar#0000](https://github.com/jmoo/drawbar/pull/0000)
- Tracking issue: none yet

## Summary

Undo and Redo reach the edits made in a document as well as the changes made to
files and tags. drawbar keeps one timeline for the session, with two kinds of
step. A library step is an entry toshokan logged, undone by toshokan as today. An
edit step holds what the editor held of a document before and after one gesture,
and is undone in memory. An edit changes what the editor holds and no file. A
save of a sound held whole changes the file and nothing the editor holds.
Because the two are independent, no step carries state for another, and Undo can
take back an edit made before a save and leave the save alone. toshokan does not
change.

## Motivation

Undo covers files and tags and stops at the document. The guide says so: "An
edit you have not saved is taken back with **Revert** instead." Revert discards
every edit since the last save. A user who turns one knob too far can turn it
back by hand or lose the session's other edits, and a knob that was at an
unremembered value leaves only the second.

⌘Z after an edit does something else today. It reaches the library's history, so
after a save it undoes the save.

What exists:

- toshokan undoes an intent by committing a compensating intent. Its history is
  per instance and ends when the instance closes. It knows files and facts, and
  nothing of an edit a user has not saved.
- Unsaved edits live in the workspace and the editors, and are kept as drafts in
  the install's own state every few seconds. They reach the file only on Save.
- A text field being typed in has egui's own undo while it has focus.

The take editor shipped briefly with its own edit history, merged with the
library's by a shared clock, and review took it out
([jmoo/drawbar#321](https://github.com/jmoo/drawbar/pull/321)) in favor of a
design for the whole app. This is that design.

It has to hold these:

1. It composes with toshokan and does not change it.
2. It works for every document type, including sample instruments and piano
   libraries edited as a plan over a file too large to hold.
3. It adds no per-frame work, no I/O per edit, and no copy of a document body.
4. It is the same code in the browser and on the desktop.
5. It grows with nothing: its memory is bounded, and it writes nothing.
6. The design rules out a wrong undo, without care at each call site.

## Guide-level explanation

**Edit ▸ Undo** takes back your last edit to a sound or your last change to
files and tags, whichever came later. A save is passed over: Undo leaves it and
takes back the edit before it. The menu names what Undo will take back:

| You do | Undo then reads |
|---|---|
| Pull a drawbar down in `c3.ne5p` | Undo Edit “c3.ne5p” |
| Rename it to `gospel.ne5p` | Undo Rename “c3.ne5p” |
| Turn a knob in `b3.ne5p` | Undo Edit “b3.ne5p” |
| Save `b3.ne5p` | Undo Edit “b3.ne5p” |
| Revert `gospel.ne5p` | Undo Revert “gospel.ne5p” |

Redo makes again what Undo took back, last first. Doing anything new ends Redo,
as it does today.

One gesture is one step. Dragging a knob from 3 to 8 is one step, however many
values it passed through. One visit to a text box is one step. An action on
several sounds at once is one step.

Saving a program, set list, note or other sound held whole does not end or
shorten what Undo can reach. ⌘Z after a save takes back the last edit, as it
would have before the save. The file stays as you saved it, and the sound reads
unsaved until you save again or redo.

**Undo Save** puts a file back as it was before a save. It shows when the next
thing to undo is the save itself: what lies behind it is a change to files, or
nothing. In the table above, after the knob is undone, Undo reads **Undo Save
“b3.ne5p”**, then **Undo Rename “c3.ne5p”**.

A sample instrument or a piano library edited in its file is different. There
the old file is the only thing that holds what a save replaced. **Undo Save**
puts the file back, and the edits made before that save are not taken back one
by one.

Undo brings the document it changes to the front.

While you type in a text box, Undo and Redo belong to that box, as they do now.
When you leave the box, what you typed becomes one step.

Undo reaches back to when the window opened the library, and no further than a
fixed number of steps. When a sound's file is changed from outside drawbar, the
edits made to that sound before the change can no longer be undone. The activity
log says so when Undo reaches one.

Sending a sound to the instrument is not a step, and undoing an edit sends
nothing.

## Reference-level explanation

### The design in one paragraph

A document's state has two halves with two owners. The library owns its file.
The editor owns its *working copy*: the body it holds, or the edit it holds over
the file. drawbar keeps one list of steps for the session. A library step names
the entry toshokan logged, and toshokan undoes it. An edit step names a document
and holds its working copy before and after, and undoing it puts the earlier one
back, but only if the document still holds the later one. A save of a body
moves the file and leaves the working copy, so a save and an edit need to know
nothing of each other. The list lives in memory, is bounded by a count and a
size, and ends with the session.

### Three rules

Everything below follows from three rules.

1. **An edit changes the working copy and no file.** This holds today.
2. **A save changes the file and no working copy.** It writes the file from the
   working copy and leaves the editor holding what it held. Its undo and its
   redo do the same in reverse: the file and the baseline move, and the document
   keeps its working copy.
3. **Any other change to a document's file is followed, and ends the document's
   edit steps.** The document takes the file as it does today, and the steps
   recorded before are let go. This covers a change from outside drawbar, this
   session writing one sound over another, the undo and redo of that, and a
   document whose body the workspace no longer holds.

Rule 2 is the one that changes behavior. Today a saved document adopts whatever
its file becomes, including what this session's own undo of a save put back.

With rule 2, a save and the edits around it are independent. Undoing a save
moves the file and the baseline. The document shows what it showed and reads
unsaved. No working copy is carried through the save, nothing waits for a file
to be read back, and the order in which the library answers does not matter.

Rule 2 is met per document type, and a type that does not meet it takes rule 3
for its saves. Bodies meet it with the changes under
[What rule 2 needs](#what-rule-2-needs). A document edited in its file does not
meet it today, because its save lets go of the edit, and it starts under rule 3:
its save ends its edit steps, and undoing the save takes the document back with
the file. Edits made since the last save are undone one by one for every type.

### Where the state belongs

| State | Kind | Home |
|---|---|---|
| Files, tags, the log | Shared truth | toshokan, unchanged |
| A document's working copy | Local truth | the workspace and editors, kept to `unsaved/` as today |
| The timeline | Local truth, memory only | drawbar, one per window |
| Unsaved mark, menu labels, whether Undo is enabled | Derived | computed from the above |

The timeline is never written. Losing it loses the ability to undo and nothing a
user sees, which is what closing the window does to toshokan's history today. The
unsaved mark stays a comparison of the working copy against the baseline. It is
never counted, so no undo can leave it wrong.

Five tests of the design:

- *Each fact in one place.* A working copy lives in the editor's half only. One
  fact is mirrored: the order of library steps is in toshokan's history and in
  the timeline. The mirror exists today as `History`, and the invariant below is
  what a test holds it to.
- *A derived structure can be deleted.* Dropping the timeline changes nothing
  shown after a reread.
- *One writer per file.* The timeline writes no file.
- *One mechanism per concern.* One list answers ⌘Z. A text field's own undo is
  the exception, bounded by focus.
- *The spec fits on a page.* The three rules, the guard and the invariant are
  the spec.

### The working copy

```rust
/// What the editor holds of a document.
#[derive(Clone, PartialEq)]
pub enum Working {
    /// The whole body.
    Body(Bytes),
    /// An edit over the file the document rests in, or none.
    Edit(Option<Edit>),
}
```

`Bytes` is the workspace's `Arc<[u8]>` and `Edit` is `rewrite::Edit`: a sample
instrument's sets or a piano library's plan. Both exist today.

| Document | Working copy | Its saves |
|---|---|---|
| Program, set list, project, note | `Body` | Rule 2 |
| Sample instrument held whole, WAV take | `Body` | Rule 2 |
| Sample instrument resting in its file | `Edit`, its sets | Rule 3 to start |
| Piano library | `Edit`, its plan | Rule 3 to start |

[Large documents](#large-documents) moves the second row to `Edit`.

`Document` answers `working(id) -> Working` and accepts `hold(id, Working)`,
beside the workspace it already drives. Each is one exhaustive match on the
document's shape, so a new kind of document cannot compile without stating what
it holds. `hold` goes through the mutators an edit goes through, and the editors
then take their state from the workspace, as they do when a kept edit is
restored at open. `working` is read before an editor lets go of anything: a
revert notes what the document held, then reverts.

Anything else an editor keeps about a document's content is derived from the
working copy and the file: a WAV take's gain figures, the bytes a piano
library's plan was laid out into, decoded audio, a field view, a half-typed box,
a picked row. After `hold` it is derived again as opening the document derives
it, or checked against the content. Where the user is looking, such as a zoom or
a scroll, is kept.

### Steps

```rust
enum Step {
    /// A change the library logged, undone by the library. `saved` names the
    /// documents it saved under rule 2, and is empty for anything else.
    Library { label: String, entry: toshokan::EntryHash, saved: Vec<u64> },
    /// What one gesture changed in the editor's half.
    Edit { label: String, changes: Vec<Change> },
}

struct Change {
    id: u64,
    /// The document's epoch when the change was made.
    epoch: u64,
    before: Working,
    after: Working,
}

struct Timeline {
    done: Vec<Step>,
    undone: Vec<Step>,
    /// The library step an undo or redo was sent for and has not answered.
    in_flight: Option<Step>,
    /// The step that failed partway and waits for the next write, as today.
    waiting: Option<Step>,
}
```

`Timeline` takes over from the mirror's `History` and adds one kind of step. It
belongs to the app, and the store reports to it. A view of a slot has no file,
and a library may be read-only or closed, and edits to documents are undone all
the same. Switching libraries drops the old library's steps.

A library step is recorded when the library answers, where `History` records it
today. The handling of a step that failed partway, of a step the next write
finishes, and of an intent no undo takes back stays as it is.

Each document has an *epoch*, a counter raised whenever rule 3 applies. An edit
step records the epoch it was made in.

| Action | Step |
|---|---|
| An edit, one gesture | `Edit`, one change per document touched |
| Revert, or taking the file's side of a conflict | `Edit`: the working copy becomes the file's |
| Save of a body | `Library`, with the document in `saved` |
| Save of a document edited in its file | `Library`; rule 3 |
| Writing over a sound | `Library`; rule 3 |
| Rename, move, tag, copy, new, delete | `Library` |

### Recording

Each call that changes a working copy states its cause:

```rust
enum Cause {
    /// The user changed it. This is a step.
    User,
    /// Rule 3: it follows its file, or a draft kept from an earlier session.
    /// This raises the epoch.
    Follow,
    /// The timeline put it back.
    Timeline,
}
```

The cause is a parameter of the mutators that change a working copy: the
workspace's `respell`, `make_edit`, `let_go_edit` and `drop_edits`, and the
piano editor's plan commit. It has no default, so a new caller cannot compile
without choosing. Laying a plan out into bytes changes no working copy and takes
no cause. Where a frame runs a call says nothing about why it ran: store
events, device answers, finished jobs and user acts interleave, and some of the
store's changes arrive as acts. The caller knows the cause, so the caller states
it.

A `User` change notes the working copy the document held before it, once per
document per frame. At the end of the frame each noted document whose working
copy now differs joins the open step, or opens one.

A step stays open while the gesture that opened it goes on: a pointer button is
still down, or the widget that had keyboard focus when it opened still has it.
A run of arrow-key nudges on one control is one step. The step closes at the end
of the first frame where neither holds, and before any library step, Undo or
Redo. A change to a document already in the open step replaces its `after`. On
closing, a change whose `before` equals its `after` is dropped, and a step left
with none is dropped. A new step empties `undone`.

No timer takes part. A drag that pauses is still one step, and two quick clicks
are two.

Recording clones an `Arc` or an `Edit`. It does no I/O, sends the store nothing,
and copies no body. A frame with no edit does nothing. Closing a step compares
each change's two ends, which share an allocation when nothing changed.

A result that arrives later than the act that asked for it, such as a detection
over a take, is a `User` change when it lands. It is its own step.

### Undo and redo

Undo takes the last step of `done`, or an edit step beneath the saves that end
`done`, as the next section says.

A library step sends `Cmd::Undo`, as today. A refusal that can never pass lets
the step go and says so. Any other refusal keeps the step and says what is in
the way. Neither changes from today.

An edit step, for each change, last first:

1. If `epoch(id) == change.epoch` and `working(id) == change.after`, call
   `hold(id, change.before)` with cause `Timeline`.
2. Otherwise let that change go, and say in the activity log that it cannot be
   undone.

A step with at least one change put back moves to `undone`. A step with none is
dropped.

Redo is the same with `before` and `after` exchanged and `Cmd::Redo`.

The guard in step 1 is the safety argument for the editor's half. An undo only
replaces a working copy the timeline recorded with another the timeline
recorded, in the epoch it recorded them. It is the editor's counterpart to
toshokan's `ChangedSince` refusal. Every step checks its own ground, so a step
let go cannot leave another wrong. The comparison runs once per key press and
tries pointer equality first.

Undo and Redo stay disabled while a change to the library is in flight or an act
waits for a document to be laid out, as today.

A library step leaves its list when its undo or redo is sent and waits in
`in_flight` until the library answers. An edit made meanwhile is recorded as
usual. When the answer comes, a step that was redone joins `done`. A step that
was undone joins `undone`, unless a step was recorded after it was sent, and
then it is dropped. A refused step goes back to the end of the list it left.

### Saves and edits

By rules 1 and 2, a save and an edit of a body are independent: undoing the save
moves a file, undoing the edit moves a body, and neither reads what the other
wrote. Undoing the edit and leaving the save gives the state that undoing the
save, undoing the edit and redoing the save would. So Undo does not undo a save
to reach an edit behind it.

Undo looks past the saves at the end of `done` to the first step that is not a
save. If that step is an edit, Undo takes it out from beneath them. Otherwise
Undo takes the last step, which is a save when there is one.

Only a library step with documents in `saved` is passed this way. Every other
library step is undone in its turn.

Redo takes the last step of `undone`, of either kind, and puts it at the end of
`done`. An edit redone lands after the save it was taken from beneath, which is
the same state by the same independence.

The command undoes one step and stops. Nothing here loops or waits. An edit
taken from beneath a save runs no commit and touches no file. A save is undone
only by a command whose label says **Undo Save**.

### Keeping toshokan in step

One invariant ties the two histories:

> The library steps of `done`, in order, are the newest intents toshokan's
> history would undo, in order. The library steps of `undone` are the newest it
> would redo.

Edit steps move among them and library steps never pass each other, so toshokan
is always asked for its own top.

toshokan ends redo when a new intent commits. An edit commits nothing, so after
a new edit step empties `undone`, toshokan still holds what it would have
redone. drawbar leaves it there and never asks for it. The next undo lands above
it and the next commit clears it, so it is never the top when drawbar asks. A
step cut for the trash is out of reach in the same way today. Until that commit,
the trash counts those steps' bytes against its limit, which can cut older steps
sooner.

A cut for the trash, a restart or a lost answer removes every step behind it, of
either kind. A library step the timeline drops for `STEPS` or `BYTES` stays in
toshokan's history, also out of reach.

### What rule 2 needs

Three things, all in the workspace and the store's mirror. toshokan is not among
them.

- **The baseline is what the file is known to hold.** Today a save moves the
  baseline when it is asked for, before the library answers, and a refused save
  leaves it moved under a new stamp. With Undo able to bring back equal bytes, the
  document would read saved over a file that does not hold them. A refused save
  puts the baseline back. Refusing a save over a file rewritten at the same
  length and time, as
  [jmoo/drawbar#300](https://github.com/jmoo/drawbar/pull/300) proposes, makes
  refused saves more common and this more pressing.
- **A save's own file is told by identity.** When the file is read back after a
  save, its undo or its redo, rule 2 applies only if the file is the one that
  commit left. Any other file is a change from outside, and rule 3 applies. The
  mirror keeps the identity the commit wrote for each document it is reading
  back. A mark on the whole answer is not enough, because another writer's
  change can arrive with it.
- **A save's own file moves only the baseline.** For a document in `saved`, the
  read-back moves the baseline, leaves the working copy, and asks no conflict
  question.

For a document edited in its file to meet rule 2, its save must stop letting go
of the edit. The edit a save was sent with has to be held apart from the edit
the editor holds now, and an edit that changes nothing over the file has to stay
held and read saved. A piano library's plan has to name the file it was made
over by content. An edit over a file is also not independent of a save of that
file, so Undo would have to undo such a save before an edit behind it. Those are
changes to how the workspace holds edits, and they are why these documents start
under rule 3.

### When things go wrong

| Event | What happens |
|---|---|
| A save is refused | The baseline goes back. No step is recorded. |
| An undo is refused and may pass later | The step stays, and the log says what is in the way. |
| An undo can never pass | The step is let go. |
| A file changes from outside | Rule 3. The document's edit steps are let go as Undo reaches them. |
| The file read back after a save's undo is another file | Rule 3. |
| The workspace no longer holds a document's body | Rule 3. There is no working copy to keep. |
| An edit of a body is made while its save is in flight | It is recorded. The save's step lands after it, and the two are independent. |
| An edit is made while an undo or redo is in flight | It is recorded. A redo in flight joins `done`. An undo in flight is dropped. |
| An asset leaves and returns | It has a new id. The edit steps made before it left are let go. |

### Order of work

This lands after [jmoo/drawbar#300](https://github.com/jmoo/drawbar/pull/300),
if that merges. #300 rewrites the mirror functions rule 2 changes: the one that
takes a file read back under a document, the one that takes a save's answer, and
the one that takes a commit's view. It also supplies what rule 2 builds on. A
commit reports the identity of each file it wrote, and an unsaved document
already keeps its edit, unasked, when this session's own undo changes its file.
Rule 2 extends that to the saved document, which adopts the file today.

#300 adds ways a document's file changes with no edit: two files found to be
one, an unsaved copy going back onto a file, and a file that ends while its
document holds work. Each takes cause `Follow`.

In order:

1. The baseline after a refused save. It is a fix on its own.
2. `working` and `hold`, causes, edit steps, the guard, and the timeline in
   place of `History`. Every document type gets undo of edits back to its last
   save.
3. Rule 2 for bodies, and Undo taking an edit from beneath a save.
4. [Large documents](#large-documents), one row at a time.

### Limits

The timeline holds at most `STEPS` steps and at most `BYTES` bytes, counting
each distinct body once and each edit at its size. Past either, the oldest steps
go. A single step larger than `BYTES` is not kept, and the timeline is cut
there.

Program, set list, project and note bodies are a kilobyte to tens of kilobytes.
A sample instrument's sets grow with each edit until a save, so they count. A
sample instrument held whole and a WAV take are megabytes to tens of megabytes
per step while they are still bodies. They are what `BYTES` is for until
[Large documents](#large-documents) is done, and few of their steps fit.

The trash holds what a save displaced, up to its own limit. When it lets go of a
save's bytes, the timeline is cut at that save, as `History` is today. The cut
now takes the edit steps behind it too.

### Large documents

A document too large to hold whole is held as `Working::Edit`, and never as
`Working::Body`. Its edit is small whatever the file's size, so its undo steps
are small, and so are the drafts kept of it.

Two costs make this a rule. A body in a step is a body in memory, and the
browser has little to spare. A body kept as a draft is written whole at every
keep, because each keep is a new file in `unsaved/`.

What holds bodies today, and what each becomes:

| Document | Today | Becomes |
|---|---|---|
| Sample instrument or piano library resting in its file | `Edit` | Unchanged |
| WAV take | `Body`, megabytes | `Edit`: its gain and marks over the file |
| Piano library held whole | The plan, and the bytes laid out from it | `Edit`: the plan. Bytes are laid out only to save or send |
| Sample instrument held whole, as a view of a slot is | `Body` | `Edit` over the bytes as received, kept as a file in the install's own state |

The last row is the one with no file in the library to rest in. What the
instrument sent is written once to the install's own state, where drafts already
go, and the document rests in that file and is read by range.

None of this is in toshokan's log, and none of it needs toshokan to change. The
install's own state is reached through toshokan's file layer, as `unsaved/` is
today. A save of an edit over a file lays the edit out by range, and a copy of a
file is made without reading it whole. The store has an intent for each.

A document that moves from `Body` to `Edit` also moves from rule 2 to rule 3 for
its saves. It trades undo across a save for memory until edits over files meet
rule 2.

### Text fields

A field being typed in keeps egui's undo, and the menu and the keys stay out of
its way as they do now. The field's changes reach the working copy during the
visit and form one open step, which closes when focus leaves.

### Browser and desktop

The timeline is plain data on the UI thread. It has no platform code and no
storage, and it adds no store command.

### What a test can state

- Undo then Redo of any step, with no step let go between, leaves every file and
  every working copy as it was.
- After every step is undone with none refused or let go, every file and working
  copy is as it was when the first was made.
- A save of a body, its undo and its redo change no working copy.
- Undoing an edit of a body from beneath a save leaves the file as saved and the
  working copy as it was before the edit, and the document reads unsaved. Redo
  puts back both and it reads saved.
- After a refused save, a document reads saved only when its working copy equals
  what the file holds.
- An edit step never changes a document whose epoch or working copy differs from
  what the step recorded.
- The timeline's library steps match toshokan's history after any sequence of
  actions, refusals and cuts.
- An edit sends the store nothing and allocates nothing proportional to the
  body.
- The timeline's memory is bounded by `STEPS` and `BYTES` for any session.

All but the last two are properties over random sequences of edits, saves, renames,
reverts, outside changes and undos, checked against a model that is a list of
whole states. Each document type has a direct test for the rule its saves
follow: under rule 2, save, undo the save, and the document shows what it
showed; under rule 3, it shows the file.

## Drawbacks

- Undo can reach a document that is not in front. Bringing it to the front is a
  jump the user did not ask for by name.
- Undo treats a save two ways. It leaves the save alone to reach an edit of a
  body, and stops at it to reach anything else. The label says which, but a
  user meets the rule without being told.
- A sample instrument or piano library edited in its file loses its edit steps
  at each save until it meets rule 2.
- Writing one sound over another ends the edit steps of the sound written over.
- Edit steps name a document by the id the workspace gave it, which is the
  steadiest name a document has. An asset that leaves and returns, as when a
  delete is undone, gets a new id, and the edit steps made before it left are
  let go. The file ids proposed in
  [jmoo/drawbar#300](https://github.com/jmoo/drawbar/pull/300) would not help:
  one is drawn anew when a file returns, and a view of a slot has none.
- Rule 2 changes how the workspace takes its own saves, and needs the baseline
  fixed after a refused save.
- After an edit ends Redo, toshokan keeps the steps it would have redone, and
  the trash keeps their bytes, until the next commit.
- A long visit to a note is one step. The field's own undo covers the typing
  only while focus stays.
- Sample instruments held whole and WAV takes keep a body per step, so few of
  their steps fit until they are held as edits. Held as edits, they lose their
  edit steps at each save.
- An outside change ends the undo of a document's earlier edits, even where the
  user keeps their own side of the conflict.
- The timeline and toshokan's history are two lists that must agree.

## Rationale and alternatives

| | Design | Verdict |
|---|---|---|
| A | Save on every edit; toshokan's history is the undo | Rejected |
| B | A stack per document; ⌘Z goes to the pane with focus | Rejected |
| C | One timeline; edit steps hold working copies | Proposed |
| D | toshokan learns unsaved edits | Rejected |
| E | Edit steps hold commands and their inverses | Rejected |
| F | A save is a mark, not a step | Rejected |
| G | A save's step carries the working copy it saved | Rejected |
| H | Undoing a save takes the document back with the file | For files edited in place |
| I | Undo always stops at a save, or never does | Rejected |

**A. Save on every edit.** It has one mechanism.
Each save is a commit that stages and syncs the file, writes and syncs a pending
record, syncs directories, appends and syncs the log, and moves a full copy of
the old file to the trash. In a picked folder in the browser an append copies the
segment. Each file-moving commit adds about 70 bytes to the writer's snapshot
that never leave. Every knob step would reach every device the folder syncs to.
A drag needs either a commit per value or a delay before saving, and a delay is
unsaved state again. It removes Save and Revert, which users have. It buys no
persistence, because toshokan's history ends with the session too.

**B. A stack per document.** Text editors do this, and it suits many files of
long text. The argument for it is that the thing in front of the user is what
should undo. Here one document shows at a time, and a session's actions cross
the line constantly: edit, save, rename, send, edit another. Save, Revert,
delete and an edit applied to several sounds belong to both stacks. ⌘Z would
have two meanings decided by focus, in an app that has no focused pane. The take
editor tried a variant, two stacks merged by a clock, and its review found the
seams.

**D. toshokan learns unsaved edits.** The log is shared truth and an unsaved
edit is local truth. Logging edits would sync every gesture and grow the log.
A second, unsynced store inside toshokan would be a new concern in a crate whose
scope is settled.

**E. Commands and inverses.** Each step would keep the values its sets replaced,
read back through a reader per document type that spells a value the way its
setter takes it. Steps are small whatever the body's size, which is its
advantage for large bodies held whole. The cost is a reader per type, a sweep to
prove every reader sets back the same bits, a check at run time for the ones the
sweep missed, and a path that forgets is an edit that cannot be undone, which is
the bug other editors keep fixing. Holding the working copy is exact by
construction and covers every path that exists and every one added later.

Inverses are needed when a save is a mark: the old file is gone and only an
inverse reaches behind it. With a save as a step, the library brings the old
file back. A large body held whole can later be held as its sets, with nothing
else here changing.

**F. A save is a mark.** Text editors do this: undo never touches the file, and
the document reads unsaved when undo stands behind the save. toshokan's history
rules it out, because it is a stack. A save left on it blocks the undo of every
earlier library step until it is undone, and a save taken off it leaves earlier
steps on the same file to undo across bytes they never saw, which needs a new
kind of undo in toshokan and a rule in its protocol. The proposal gets this
option's behavior for bodies from independence, with toshokan as it is: the save
stays on the stack, and is undone when Undo reaches it.

**G. The save's step carries the working copy.** A save would let go of the edit
as today, and its step would put the edit back when undone. The library's answer
and the file's read are separate events, frames apart, and the read may never
come. The put-back has to wait for it, Undo has to stay off until then, and an
edit made between the save and its answer lands on the wrong side of the save's
step. Rule 2 removes each of those by leaving the working copy alone.

**H. The document follows the file.** Undoing a save restores the file, the
saved document adopts it as it does today, and the edit steps behind the save go
stale. It is rule 3 applied to saves, and needs nothing that rule 2 needs. The
cost is that one ⌘Z after a save takes back every edit since the save before.
It is where documents edited in their files start, and the fallback for any type
where rule 2 proves fragile.

**I. Always stop, or never.** If Undo always stops at a save, a user who saves
after each edit presses ⌘Z twice per edit, and every second press changes
nothing in the editor and commits to the library. If Undo never stops and undoes
saves in passing, one ⌘Z puts back the file of every sound saved since the edit,
including sounds the edit never touched, and each of those files syncs to every
device as its older self. For a piano library that is hundreds of megabytes
moved by a key press that named an edit. Both follow from undoing a save that
did not need undoing.

**Nothing.** Revert stays the only way back, and each editor that wants more
grows its own, as the take editor did.

## Prior art

### Earlier work here

The take editor's history kept a step per edit of the open WAV, stamped from a
clock shared with the library's history, and ⌘Z took whichever side was stamped
later. Review of [jmoo/drawbar#321](https://github.com/jmoo/drawbar/pull/321)
found four faults, and each has a counterpart here:

| Fault found | Here |
|---|---|
| A save reset the history after a gain and not after a cut | One rule per document type says what its save does, and a save of a body resets nothing. |
| Undo traveled as an edit in a queue that names no document, and dropped the frame's other edits | Undo is a command on the timeline. Each change names its document. |
| The document's side reported nothing while another tab was in front | The timeline belongs to no document and no tab. |
| After an undo, the pick named a sample that no longer existed | Editor state beside the working copy is derived and rebuilt. |

### Other software

How shipping software relates edit undo to saving and to kept history:

| Software | Unit and scope | Across save | Kept history | Gesture grouping | Changed underneath |
|---|---|---|---|---|---|
| macOS documents | Event-loop group, per document | Kept; unsaved state follows undo | Versions, separate | Explicit groups | File coordination |
| VS Code | Edit, per file; a rename refactor spans files | Kept; kept across close if content matches | Timeline, separate | Typing runs | Content mismatch drops the stack |
| Vim | Change, a tree per buffer | Kept; writes are marks in the tree | The undo file is the history | One insert | Hash mismatch ignores the undo file |
| Photoshop | State, per document | Kept until close | None in the file | Stroke or command | n/a |
| Blender | Operator, one global stack | Kept until close | Autosave, separate | Operator | n/a |
| Reaper | Action, per project, memory cap | Kept; optionally saved beside the project | Optional | Options per kind of action | n/a |
| Figma | Own operations, per user | Saving is continuous | Version history, separate | Gesture | Own operations only |
| Yjs | Transaction, per origin | n/a | None | 500 ms window | Other origins untracked |
| Jujutsu | Operation, per repository | n/a | The operation log is the undo | Command | Operations merge |
| JetBrains IDEs | Command, per file, plus global commands | Kept | Local History, days | Typing | "Cannot undo" |

What this RFC takes from them:

- **Two mechanisms are the norm.** A session undo stack beside a coarser kept
  history is what nearly every row has. toshokan's log and trash are the kept
  history here. This RFC adds the session half and does not merge the two.
- **Save is not a reset, and not a stop.** Apple's documents and VS Code keep
  undo across a save and derive "unsaved" from where undo stands. Where a save is
  reported to clear history, users complain. None of them makes ⌘Z stop at a
  save, and none changes the saved file on undo. For a body, neither does this.
- **History over the wrong base is the classic corruption.** Vim and VS Code
  compare content and drop the history on a mismatch. The guard is that rule,
  applied per step, and the epoch is its stricter form for a file changed from
  outside.
- **Whole-state steps are right for small state.** Blender's undo became slow
  where state was large, and its fix was to share unchanged blocks. Bodies here
  are small and shared by `Arc`. Inverse commands earn their cost where several
  people edit at once, which is not this app.
- **Gestures beat timers.** Yjs splits a drag that pauses. Krita's time-based
  merging has a long bug history.
- **Coverage gaps are the recurring bug.** Surge XT's changelog adds missed edit
  paths to undo release after release. Apple warns against undo for a subset of
  changes.
- **One global command in many stacks is fragile.** JetBrains records a
  refactoring in every file's stack and refuses it when one file moved on. One
  timeline has no such case.
- **A cap is how memory stays bounded.** Reaper caps undo memory and drops the
  oldest.

Sources:

- [Apple, Document-Based App Programming Guide](https://developer.apple.com/library/archive/documentation/DataManagement/Conceptual/DocBasedAppProgrammingGuideForOSX/StandardBehaviors/StandardBehaviors.html)
- VS Code release notes [1.44](https://code.visualstudio.com/updates/v1_44),
  [1.46](https://code.visualstudio.com/updates/v1_46),
  [1.52](https://code.visualstudio.com/updates/v1_52)
- [Vim, undo.txt](https://vimhelp.org/undo.txt.html)
- [Adobe, History panel](https://helpx.adobe.com/photoshop/desktop/get-started/set-up-toolbars-panels/history-panel-overview.html)
- [Krita, Undo History](https://docs.krita.org/Undo_History)
- [Blender 2.83 release notes](https://wiki.blender.org/release_notes/2.83/more_features/)
- [Reaper, General preferences](http://wiki.cockos.com/wiki/index.php/Preferences_General)
- [Figma, How multiplayer technology works](https://www.figma.com/blog/how-figmas-multiplayer-technology-works/)
- [Liveblocks, Undo and redo in a multiplayer environment](https://liveblocks.io/blog/how-to-build-undo-redo-in-a-multiplayer-environment)
- [Yjs, UndoManager](https://docs.yjs.dev/api/undo-manager)
- [Jujutsu, operation log](https://man.archlinux.org/man/jj-operation-log.1.en)
- [JetBrains, Local History](https://www.jetbrains.com/help/idea/local-history.html)
- [Surge XT changelog](https://surge-synthesizer.github.io/changelog)

## Unresolved questions

- **Documents edited in their files.** When should sample instruments and
  piano libraries move from rule 3 to rule 2, and is it worth it? A save of one
  is rare and heavy, and its undo moves a large file.
- **Delete.** Deleting an asset with an unsaved edit drops the edit. Undoing the
  delete brings the file back and the edit stays gone. Should the edit come
  back?
- **Bringing the document to the front.** Should the first ⌘Z on a document that
  is not showing only show it, and the second undo? Should a step that touched
  several documents show any?
- **`STEPS` and `BYTES`.** 1,000 steps and 256 MiB are placeholders. The browser
  may want less.
- **What uses memory in the browser.** It has not been measured. Decoded audio
  and the limit on files read whole may weigh more than working copies, and
  neither is touched here. A heap profile of a session should order the rows of
  the Large documents table against them.
- **The wheel.** A wheel has no end the app can see, so each tick is a step. Is
  that too many?
- **Notes.** Is one step per visit to a note too coarse? The note editor could
  close the step on its own terms.
- **The label.** "Edit" names no field. The editor knows the field at the point
  of the change and could pass its name.

Implementation updates the guide's Undo section in
`docs/src/drawbar/this-computer.md` in the same change.

## Future possibilities

- A history panel listing the timeline's steps.
- Field names in labels: "Undo Drawbar 3".
- No confirmation before a detection replaces a take's marks. It asks today
  because nothing can take it back.
- Undo beside a line of the activity log. The timeline undoes in order, so
  undoing one line out of order needs its own rule.
- A timeline that survives a restart. It needs toshokan's history to survive one
  first.
