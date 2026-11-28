# RFC-0356: One undo timeline for files and edits

- Start date: 2026-11-22
- Authors: jmoo
- RFC PR: [jmoo/drawbar#356](https://github.com/jmoo/drawbar/pull/356)
- Tracking issue: none yet

## Summary

Undo and Redo reach the edits made in a document as well as the changes made to
files and tags. drawbar keeps one timeline for the session, with two kinds of
step. A library step is an entry toshokan logged, undone by toshokan as today. An
edit step holds what the editor held of one document before and after one
gesture, and is undone in memory. An edit changes what the editor holds and no
file. A save of a sound held whole changes the file and nothing the editor holds.
Because the two are independent, no step carries state for another, and Undo can
take back an edit made before a save and leave the save alone. An edit step
applies only where its document still holds exactly what the step recorded, and
that one check is the whole safety argument. toshokan does not change.

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
3. It adds no I/O per edit and no copy of a document body, and a frame with no
   edit does nothing that grows with the document.
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
values it passed through. One visit to a text box is one step.

Saving a program, set list, note or other sound held whole does not end or
shorten what Undo can reach. ⌘Z after a save takes back the last edit, as it
would have before the save. The file stays as you saved it, and the sound reads
unsaved until you save again or redo.

**Undo Save** puts a file back as it was before a save. It shows when the next
thing to undo is the save itself: what lies behind it is a change to files, or
nothing. In the table above, after the knob is undone, Undo reads **Undo Save
“b3.ne5p”**, then **Undo Rename “c3.ne5p”**. The sound keeps what it shows, and
reads unsaved where that differs from the file.

A sample instrument or a piano library edited in its file is different. There
the old file is the only thing that holds what a save replaced. **Undo Save**
puts the file back, and the edits made before that save are not taken back one
by one.

Undo brings the document it changes to the front.

While you type in a text box, Undo and Redo belong to that box, as they do now.
When you leave the box, what you typed becomes one step.

Undo reaches back to when the window opened the library, and no further than a
fixed number of steps. When a sound takes what its file holds, as after a change
from outside drawbar, the edits made to it before can no longer be undone, and
Undo no longer offers them.

Sending a sound to the instrument is not undone, and undoing an edit sends
nothing.

## Reference-level explanation

### The design in one paragraph

A document's state has two halves with two owners. The library owns its file,
and the baseline is what the file is known to hold. The editor owns its *working
copy*: the body it holds, or the edit it holds over a baseline it names. A
working copy is all of the document's content. drawbar keeps one list of steps
for the session. A library step names the entry toshokan logged, and toshokan
undoes it. An edit step names one document and holds its working copy before and
after, and undoing it puts the earlier one back, but only if the document holds
exactly the later one. A save of a body moves the file and the baseline and
leaves the working copy, so a save and an edit need to know nothing of each
other. The list lives in memory, is bounded by a count and a size, and ends with
the session.

### Three rules

Everything below follows from three rules.

1. **An edit changes the working copy and no file.** This holds today.
2. **A save of a body changes the file and no working copy.** It writes the file
   from the working copy and moves the baseline. Its undo and its redo do the
   same in reverse: the file and the baseline move, and the document keeps what
   it holds.
3. **A step applies only where its document holds what the step recorded.**
   Anything else that replaces a working copy records no step: a document taking
   a file changed from outside, a kept draft restored, one sound written over
   another, a baseline moving under an edit made over it. The steps recorded
   before it no longer match, and are let go.

Rule 2 is the one that changes behavior. Today a saved document adopts whatever
its file becomes, including what this session's own undo of a save put back.

With rule 2, a save and the edits around it are independent. Undoing a save
moves the file and the baseline. The document shows what it showed and reads
unsaved. No working copy is carried through the save, nothing an edit step needs
waits for a file to be read back, and the order in which the library answers
does not matter.

Rule 3 needs no bookkeeping, because a working copy says everything its content
depends on. A body is whole. An edit names its baseline, so an edit over a file
that has since been saved, changed or read again is another value, and a step
that recorded the old one does not match. Nothing counts changes or marks steps
stale: the comparison at the moment of the undo is the only test.

An outside change that a document does not take, because the user keeps their
own side, replaces no body. The steps of a body stay good across it.

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
  the timeline. The mirror existed before as the store's `History`, and the
  invariant below is what a test holds it to.
- *A derived structure can be deleted.* Dropping the timeline changes nothing
  shown after a reread.
- *One writer per file.* The timeline writes no file.
- *One mechanism per concern.* One list answers ⌘Z, and one comparison decides
  whether a step applies. A text field's own undo is the exception, bounded by
  focus.
- *The spec fits on a page.* The three rules and the invariant are the spec.

### The working copy

```rust
/// What the editor holds of a document. It is all of the document's content: a
/// body is whole, and an edit names the baseline it is over.
#[derive(Clone, PartialEq)]
pub enum Working {
    /// The whole body.
    Body(Bytes),
    /// An edit over the saved baseline stamped `over`, or none.
    Edit { over: u64, edit: Option<Edit> },
}
```

`Bytes` is the workspace's `Arc<[u8]>` and `Edit` is `rewrite::Edit`: a sample
instrument's sets or a piano library's plan. Both existed. `over` is the stamp
the workspace gives a saved baseline, which moves whenever the baseline does.

| Document | Working copy | Its saves |
|---|---|---|
| Program, set list, project, note, WAV take | `Body` | Rule 2 |
| Sample instrument held whole | `Body` | Its file is read back in place, so rule 3 |
| Sample instrument resting in its file | `Edit`, its sets | The baseline moves, so rule 3 |
| Piano library resting in its file | `Edit`, its plan | The baseline moves, so rule 3 |
| Piano library held whole | None | No edit steps |

A sample instrument or piano library is read back from its file once saved, and
its editor then shows the file. Its save is an ordinary library step, and the
document follows the file when that step is undone.
[Large documents](#large-documents) moves the second row to `Edit`.

`Document` answers `working(id) -> Working` and accepts `hold(id, Working)`,
beside the workspace it already drives. `hold` records no step, and answers
whether the document holds exactly what it was given. After it, the editors
derive what they show from the content again, as they do when a kept edit is
restored at open: a WAV take's gain figures, the bytes a piano library's plan
was laid out into, decoded audio, a field view, a half-typed box, a picked row.
Where the user is looking, such as a zoom or a scroll, is kept.

A piano library held whole has no working copy and records no edit steps. Its
content is the body it holds under a plan that only its editor holds, laid out
into bytes when something needs them, so no one value says what it holds. It is
a short-lived state: a library just built and not yet reopened from its file, a
restored draft, or a view of a slot. Giving it one value is later work.

### Steps

```rust
enum Step {
    /// A change the library logged, undone by the library. `saved` names the
    /// documents it saved under rule 2, and is empty for anything else.
    Library { label: String, entry: toshokan::EntryHash, saved: Vec<Saved> },
    /// What one gesture changed in what the editor holds of one document.
    Edit { label: String, id: u64, before: Working, after: Working },
}

/// A document a save wrote from its body: what its file held before and after.
struct Saved { id: u64, before: Filed, after: Filed }

/// What a file holds: the baseline the workspace knows it by, and the identity
/// the library knows it by.
struct Filed { identity: toshokan::Identity, baseline: Baseline }

struct Timeline {
    done: VecDeque<Step>,
    undone: Vec<Step>,
    /// The gesture that keeps the last step open, if one does.
    open: Option<Gesture>,
    /// What the library was asked that failed partway and waits for the next
    /// write, as today.
    waiting: Option<Asked>,
}
```

`Timeline` takes over from the store mirror's `History` and adds one kind of
step. It belongs to the app, and the store reports to it. A view of a slot has
no file, and a library may be read-only or closed, and edits to documents are
undone all the same. Switching libraries drops the old library's steps and
every step behind one.

A library step is recorded when the library answers, where `History` recorded
it. The handling of a step that failed partway, of a step the next write
finishes, and of an intent no undo takes back stays as it was.

A library step leaves its list when its undo or redo is sent, and the store
holds it until the library answers. An edit made meanwhile is recorded as usual.
When the answer comes, a step that was redone joins `done`. A step that was
undone joins `undone`, unless a step was recorded after it was sent, and then it
is dropped. A step that failed goes back to the end of the list it left, and a
step the library refused is let go, as the next section says.

| Action | Step |
|---|---|
| An edit, one gesture | `Edit` |
| Revert, or taking the file's side of a conflict | `Edit`: the working copy becomes the file's |
| Save of a body | `Library`, with the document in `saved` |
| Save of a document edited in its file | `Library`; its baseline moves |
| Writing over a sound | `Library`; the sound written over takes the file |
| Rename, move, tag, copy, new, delete | `Library` |

### Recording

Each call that changes a working copy states its cause:

```rust
enum Cause {
    /// The user changed it: a step Undo takes back.
    User,
    /// It takes what a file, a kept draft or an undo gives it: no step.
    Follow,
}
```

The cause is a parameter of the workspace's mutators that change a working copy:
`replace_bytes`, `hold_edit` and `drop_edits`. It has no default, so a new
caller cannot compile without choosing. Bytes laid out from a plan change no
working copy and go in by a call of their own. Where a frame runs a call says
nothing about why it ran: store events, device answers, finished jobs and user
acts interleave. The caller knows the cause, so the caller states it.

A `User` change notes the working copy the document held before it, once per
document per frame. At the end of the frame each noted document whose working
copy now differs is a step, or extends the open one.

A step stays open while the gesture that made it goes on, and the app reads the
gesture off the frame's input:

- a widget has keyboard focus, and the step stays open while that widget keeps
  it. A run of arrow-key nudges on one control is one step, and so is one visit
  to a text box.
- otherwise a pointer button is down, or was let go in this frame. A drag is one
  step, and so is a click.
- otherwise nothing goes on, and the change is a step of its own.

A pointer press ends the step before it, whatever gesture made it. A change in
the frame of the press, or in the frame focus leaves a widget by a key, where
that widget had focus as the frame began, is taken as that widget committing as
it loses focus: it joins the widget's step, which then closes. Where no widget had focus, a change in that frame, such as a
slider jumping to the pointer, begins the step the press goes on with. A click
inside a text box therefore splits the typing in two.

A document is noted once a frame, so a control that changes on the press, pressed
while another widget has focus, cannot be told from that widget committing. Its
first change joins the widget's step and the rest of its drag is a second step.

A revert is always a step of its own. It closes the open step and is never
extended, so Undo always brings back what it reverted.

A change extends the open step only if it is to the same document and starts
from what the step left. The step closes at the end of the first frame with
another gesture, and before any library step, Undo or Redo. A step whose two
ends become equal is dropped at once, so the menu never names it. A new step
empties `undone`, unless it is a save.

No timer takes part. A drag that pauses is still one step, and two quick clicks
are two.

Recording clones an `Arc` or an `Edit`. It does no I/O, sends the store nothing,
and copies no body. A frame with no edit takes no note.

A result that arrives later than the act that asked for it, such as a detection
over a take, is a `User` change when it lands.

### Undo and redo

Undo takes the last step of `done`, or an edit step beneath the saves that end
`done`, as the next section says.

A library step sends `Cmd::Undo`, as today, and the library answers one of
three ways:

- It makes the step.
- It refuses the step before anything is written, over what the folder or its
  own history holds. The file a save wrote holds other bytes or is gone,
  something is where the step would put a file, another drawbar changed the
  same thing since, or the trash no longer holds what the step needs. Waiting
  changes most of these not at all, so the step is let go with a word that says
  why, and Undo names the step before it.
- The step fails, because the folder could not be read or written or the
  library is read-only. That may pass, so the step goes back to the list it
  left, and the log says what is in the way. A redo does not go back where a
  step was recorded while it was asked, since a new step ends Redo.

toshokan draws the same line: a refusal is its own kind of error, apart from a
failed read or write. Today drawbar lets a step go only for the last two
refusals above, and a step refused for any other reason stays first in Undo and
is refused at every press. In one timeline that would shut off every step
behind it, edits included.

A library step is checked when it is asked, since only the library can look at
the folder. A step whose file is put back as the step left it, and seen there
by drawbar's next look at the folder, before Undo is pressed is taken as any
other.

An edit step is one comparison and one call:

1. If the document holds exactly `after`, call `hold(id, before)`.
2. Otherwise let the step go, and do nothing more.

Undo takes the step the menu named, or one made since the menu was drawn, or
none. It never falls through to the step behind, which may be a library change
the menu did not name. A command asked in the same frame after one that ran
follows what that one did, and after one that did nothing it is dropped. The
menu names its step as each frame ends, so a press in the frame after an undo
takes the step before the one that undo took. The Mac's menu bar is brought up
to date at that same point, so its labels and the steps a pick takes are one
reading of the timeline.

Redo is the same with `before` and `after` exchanged and `Cmd::Redo`. It takes
the step the menu named or none, since a step made since ends redo: an undone
save that lands above the named step is not written again in its place.

The comparison in step 1 is the safety argument for the editor's half. An undo
only replaces a working copy the timeline recorded with another the timeline
recorded for the same document, and a working copy is all of the content. It is
the editor's counterpart to toshokan's `ChangedSince` refusal. Every step checks
its own ground, so a step let go cannot leave another wrong. A step whose
document holds the recorded value again is good again, whatever happened
between.

The comparison tries pointer equality first. Where a document holds an equal
body in another allocation, the step takes the document's, so the bytes are
compared once.

At the end of each frame the timeline makes the same comparison for the step
Undo would take and the step Redo would take, and lets go of one that no longer
matches. The menu therefore names only a step that would apply. For a body,
that is a pointer comparison in a frame where nothing changed. For a document
resting in its file, it compares the baseline's stamp and the edit's sets.

Undo and Redo run at the end of the frame they are asked in, after the frame's
edits are recorded, and the offer is checked then. A click and ⌘Z that reach one
frame take back the click, whichever the frame order of keys and widgets would
have run first.

Undo and Redo are disabled while a change to the library is sent and not
answered. An edit step also waits while a save of its document is asked and not
sent, and while a save, export or send that carries its document waits on a
save. A library step also waits, as today, for every change asked for to be
sent.

### Saves and edits

By rules 1 and 2, a save and an edit of a body are independent: undoing the save
moves a file, undoing the edit moves a body, and neither reads what the other
wrote. Undoing the edit and leaving the save gives the state that undoing the
save, undoing the edit and redoing the save would. So Undo does not undo a save
to reach an edit behind it.

Undo looks past the saves at the end of `done` to the first step that is not a
save. If that step is an edit, Undo takes it out from beneath them. Otherwise
Undo takes the last step, which is a save when there is one.

Only a library step with documents in `saved` is passed this way: a commit that
did nothing but save documents from their bodies over their own files, which
existed. A new file, one sound written over another, a save that failed partway
and was finished by a later write, and any save of a sample instrument or piano
library are not such steps. Every other library step is undone in its turn, and
a document follows its file when one that saved it is undone.

Redo takes the last step of `undone`, of either kind, and puts it at the end of
`done`. An edit redone lands after the save it was taken from beneath, which is
the same state by the same independence.

A save is not a new edit, so it does not end the redo of one. A save recorded
while an undone edit waits in `undone` leaves it there: the save changed nothing
the edit step holds. It does end toshokan's redo, as every commit does, so the
library steps of `undone` go, and every step undone before one of them. A change
no undo takes back, such as taking in a rescued sound, is recorded as no step and
ends redo in the same way.

The command undoes one step and stops. Nothing here loops over the library or
waits. An edit taken from beneath a save runs no commit and touches no file. A
save is undone only by a command whose label says **Undo Save**.

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

- **One answer to what a file holds.** A save moves the saved baseline when it
  is asked for, before the library answers. While it is unanswered the workspace
  keeps what the file is known to hold beside it, and one accessor gives that
  value: the kept baseline, or else the saved one. A refused save puts it back.
  A read of the file is compared against it, so a read that finds what the file
  is known to hold changes nothing, and a document holds unsaved work whenever
  it holds something else. That is the one test behind the conflict question,
  keeping a document whose file was deleted, and what a quit would lose.
- **A save step holds the baselines it moved between.** An edit step holds the
  working copy before and after. A save step holds, for each document, the
  baseline its file held before and the one it wrote, each with the identity the
  library knows that file by. Both are in memory when the save is sent, and they
  are allocations the document and the edit steps around the save usually share.
  They keep their stamps, so a baseline that comes back is the same baseline.
- **The undo and redo of a save move the baseline when the library answers.**
  toshokan's undo of a save either restores the displaced file or refuses. When
  it answers that it made the step, the file holds the other baseline of the
  step, and the workspace takes it at once. The working copy stays, and the
  document reads unsaved from that instant by the ordinary comparison. Nothing
  waits for the file to be read again. The read that follows finds what the file
  is known to hold, or finds another writer's change and is followed as any
  change from outside is.

An earlier draft told a save's own file by reading it back and matching its
identity. That left a window between the library's answer and the read in which
the document read saved over a file that no longer held its bytes. A change or a
delete from outside in that window took the document's content without asking.

For a document edited in its file to keep its edit steps across a save, its save
must stop moving the ground its edit stands on. The edit a save was sent with
has to be held apart from the edit the editor holds now, an edit that changes
nothing over the file has to stay held and read saved, and a working copy has to
name its file by content and not by a stamp. An edit over a file is also not
independent of a save of that file, so Undo would have to undo such a save
before an edit behind it. Those are changes to how the workspace holds edits,
and they are why these documents lose their edit steps at a save.

### When things go wrong

| Event | What happens |
|---|---|
| A save is refused | The baseline goes back. No step is recorded. A piano library held whole shows its plan again. |
| A file changes from outside while a save of it is unanswered | The document keeps what it holds, and the user is asked which side to keep. |
| A piano library's plan is reverted or undone while a send or save waits for it to be laid out | The send or save is dropped. |
| The answer to an undo or redo of a save is lost | What the file holds is unknown. Each document the save wrote that holds its body reads unsaved, and its file is read again. That read moves only the baseline: equal bytes read saved again, and other bytes leave the document unsaved over them. |
| An undo or redo of a save stops partway | Each file the save wrote is taken to hold what the step leaves, as the next write or open finishes it. A document holding its body is unsaved where it holds something else, and a Save writes it once the step is finished. While the library says the step waits, through a lost answer and an open that still cannot finish it, each such document is kept as a draft over that file whatever it reads and is not let go of for memory, so a quit loses nothing however the step ends. |
| The next write leaves such a step neither finished nor waiting, because its record never landed or the library gave it up | Each file a step names is read again once the step no longer waits, as after a lost answer. That read moves only the baseline, and one that fails is made again with the next look at the folder. A Save that is that write expected what the step would have left, so it is refused once, and the next Save writes. |
| A save is asked in a library that cannot be written | The document reads saved in memory only, and its file is still known to hold what it held. A change to the file from outside asks. |
| A save of an edit held over its file is asked in a library that cannot be written | The save is refused and the edit stays. What waited on it is dropped, with a word. |
| The library refuses an undo or redo over what the folder or its history holds | The step is let go, and the log says why. Nothing is written, and Undo names the step before it. |
| An undo or redo fails because the folder could not be read or written, or the library is read-only | The step stays, and the log says what is in the way. A redo that a step recorded since has ended is let go. |
| The library's history ends while an undo or redo waits for its answer | The step is let go with the rest of that history, whatever the answer. |
| The answer to a save is lost, or a save fails partway and a later write finishes it | The baseline goes back, and the file is read again once what it holds can be known. That read moves only the baseline, so an edit made or undone since stays, unsaved where the file holds something else. |
| A document takes a file changed from outside | Its working copy is the file's. Its edit steps no longer match. |
| A file changes from outside and the user keeps their side | A body is untouched and its steps stay. An edit over the file is made again over it, and its steps no longer match. |
| A file changes or is deleted from outside after a save of it is undone | The document holds unsaved work, so the user is asked, or the document is kept as missing. |
| The workspace no longer holds a document's body | There is no working copy. Its steps no longer match. |
| An edit of a body is made while its save is in flight | It is recorded. The save's step lands after it, and the two are independent. |
| An edit is made while an undo or redo is in flight | It is recorded. A redo in flight joins `done`. An undo in flight is dropped. |
| An asset leaves and returns | It has a new id. The edit steps made before it left are let go, and a save step that named it follows its file as any library step does. |
| A document was let go of for memory since a save of it | The undo of that save marks what is remembered of it stale, and it is read from its file when next shown. Shown before that save is redone, it holds what it read, which the redo leaves unsaved over the file. |

### Order of work

1. The baseline after a refused save. It is a fix on its own.
2. `working` and `hold`, causes and notes, the timeline in place of `History`,
   edit steps and the gesture rule. Every document type gets undo of edits back
   to its last save.
3. Rule 2 for bodies, and Undo taking an edit from beneath a save.
4. [Large documents](#large-documents), one row at a time.

Steps 1 to 3 are one change. Step 4 is later work.

[jmoo/drawbar#357](https://github.com/jmoo/drawbar/pull/357), which was closed
for this, fixed one case of rule 2 on its own: an unsaved document keeps its
edit, unasked, when this session's own undo or redo changes its file, which it
told by the length and time the step left the file at. The baselines a save's
step holds cover that case and the saved document's without that check.
[jmoo/drawbar#300](https://github.com/jmoo/drawbar/pull/300), which was closed,
would have given a commit that reports the identity of each file it wrote, and
is not needed either.

### Limits

The timeline holds at most `STEPS` steps and at most `BYTES` bytes, counting
both ends of each edit step and both baselines of each save step, so a body two
steps share counts twice. Past
either, the oldest steps go. A single step larger than `BYTES` is not kept, and
the timeline is cut there.

Program, set list, project and note bodies are a kilobyte to tens of kilobytes.
A sample instrument's sets grow with each edit until a save, so they count. A
sample instrument held whole and a WAV take are megabytes to tens of megabytes
per step while they are still bodies. They are what `BYTES` is for until
[Large documents](#large-documents) is done, and few of their steps fit.

The trash holds what a save displaced, up to its own limit. When it lets go of a
save's bytes, the timeline is cut at that save, as `History` was. The cut takes
the edit steps behind it too.

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
[Where unsaved work is kept](#where-unsaved-work-is-kept) says why there, and
how such files are let go.

None of this is in toshokan's log, and none of it needs toshokan to change. The
install's own state is reached through toshokan's file layer, as `unsaved/` is
today. A save of an edit over a file lays the edit out by range, and a copy of a
file is made without reading it whole. The store has an intent for each.

A document that moves from `Body` to `Edit` loses its edit steps at each save,
as every edit over a file does. It trades undo across a save for memory until a
save of an edit over a file leaves the edit's ground alone.

### Where unsaved work is kept

Unsaved edits are kept today so that they outlive a restart. They go in
`unsaved/` in the install's own state for the library, which is never synced and
never in the library folder. In the browser that is the site's private storage.
This RFC keeps that, and puts the bytes of a slot view beside them.

**Why the install's own state.** The library folder is shared truth: it syncs,
and every device sees what is in it. An unsaved edit is local truth.

- Edits are kept every few seconds. In the folder, a sync service would upload
  each keep, and a body is written whole each time.
- Two computers could each keep an edit of one sound. Their files would arrive
  side by side with clocks that do not compare, and choosing between them is a
  merge of unsaved work. A save is where two computers meet.
- Everything in the folder is a file the user named or toshokan's record of a
  change. A kept edit is neither, and every version of drawbar that opens the
  folder would have to know it.
- A read-only library keeps unsaved edits today. In the folder it could not.

The folder would give two things. An unsaved edit would follow the user to
another computer. And where the browser has a folder the user picked, kept work
would be on the user's disk, out of reach of the browser's limit on a site's
storage and of its right to delete it. drawbar asks the browser to keep the
site's storage, and the browser may decline.

**How kept files are let go.** Nothing sweeps `unsaved/` by age or by size. A
kept file lives as long as its edit:

- Each keep is a new file. The instance that made it then removes the files of
  that edit it kept or read before.
- When the edit ends, as by a save or a revert, its files are removed. A removal
  that fails is tried again before each later command and at close.
- A file another window kept is removed by this one only after reading it, when
  this one keeps or ends the same edit.
- At open, every kept edit of a sound in the library is read, and the newest of
  each is the edit. A file a crash left behind stays until its edit is next kept
  or ended.

So `unsaved/` holds about one file per unsaved edit. What bounds it is how many
sounds have unsaved edits.

**What large documents need of it.**

- Opening reads each kept file whole to find the newest of each edit. Kept edits
  become small, so that stays cheap. A slot view's bytes are a file of their own,
  named by the hash a kept slot edit already carries, and are read by range.
- A slot view's bytes are let go when the view closes with no edit over them,
  and with the edit when it ends.
- An edit kept over a slot is read only when that slot is viewed again. One
  never viewed again stays, and its megabytes would stay with it. That needs an
  end it does not have today.

### Text fields

A field being typed in keeps egui's undo, and the keys and the menu bar stay out
of its way as they do now. Any other control that has the keyboard, such as a
knob turned with the arrow keys, leaves them to the timeline. A note's text
reaches the working copy as it is typed, and forms one open step that closes
when focus leaves. Every other box commits when it is left, which is one change
and one step.

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
- A document reads unsaved exactly when its working copy differs from what its
  file holds, after a refused save as after anything else.
- An edit step never changes a document whose working copy differs from what the
  step recorded.
- The timeline's library steps match toshokan's history after any sequence of
  actions, refusals and cuts.
- For every document type, `hold` of what `working` answered changes nothing,
  and `working` after `hold` answers what was held.
- An edit sends the store nothing and allocates nothing proportional to the
  body.
- The timeline's memory is bounded by `STEPS` and `BYTES` for any session.

All but the last three are properties over random sequences of edits, saves,
renames, reverts, outside changes and undos, checked against a model that is a
list of whole states.

## Drawbacks

- Undo can reach a document that is not in front. Bringing it to the front is a
  jump the user did not ask for by name.
- A step whose document moved on is let go without a word. The menu stops
  offering it, and nothing says why.
- A library step is let go the first time the library refuses it, and is not
  offered again if its file is later put back as the step left it. What the
  step displaced stays in the trash, where no Undo reaches it. Some refusals
  are judged from the folder as drawbar last looked at it, so a file that was
  missing at that look and has come back since still costs its step. So does a
  file a sync tool has removed for a moment when Undo is pressed.
- Undo treats a save two ways. It leaves the save alone to reach an edit of a
  body, and stops at it to reach anything else. The label says which, but a
  user meets the rule without being told.
- A sample instrument or piano library edited in its file loses its edit steps
  at each save until it meets rule 2. It also follows its file when a save is
  undone or redone, and an edit that changes nothing over the file is not held.
  So values typed back to what an undone save restores are let go, and redoing
  that save shows the saved values, not the ones typed back.
- Writing one sound over another ends the edit steps of the sound written over.
- A sample instrument or piano library held whole, such as one just made, takes
  its file when a save of it is undone, because its file is read back in place
  and not as a body.
- The same content held another way is another working copy. A body put back to
  rest in its file after a refused save holds what it held, and its steps no
  longer match.
- Edit steps name a document by the id the workspace gave it, which is the
  steadiest name a document has. An asset that leaves and returns, as when a
  delete is undone, gets a new id, and the edit steps made before it left are
  let go. The file ids proposed in
  [jmoo/drawbar#300](https://github.com/jmoo/drawbar/pull/300), which was
  closed, would not have helped: one was drawn anew when a file returned, and a
  view of a slot had none.
- Rule 2 changes how the workspace takes its own saves, and needs the baseline
  fixed after a refused save. The saved baseline still moves when a save is
  asked for and not when it lands, so the workspace keeps what the file holds
  beside it for that window. Moving the baseline only when a save lands would
  need no second value, and is a larger change to how saves work.
- A save step keeps both baselines of each document it saved in memory, where
  toshokan already keeps the displaced file in its trash.
- A piano library held whole has no edit steps. Its plan cannot be edited in the
  moment between asking for a save of it and the library's answer, because a
  refused save gives the earlier plan back and two plans cannot be merged.
- Undo or Redo of an edit to a WAV take decodes the take again. On a five-minute
  stereo take a redo measured about twice the cost of the edit.
- A WAV take's gain is told from its bytes against its saved file, by a sample
  the gain left unclipped. A gain that clips every sample leaves none, so after
  its Undo and Redo the take reads 0 dB over the clipped audio, as it does when
  such an edit is restored at open.
- A set of a sample instrument takes the place of one of the same value just
  before it, and no other. A gesture that sets two values at once keeps every
  set it made, so dragged there and back it leaves a step that changes nothing.
- A read that follows a lost answer, or a step that stopped partway, cannot
  tell drawbar's own bytes from another writer's. The document keeps what it
  holds, unsaved, and is not asked about either.
- A file resting in the library is known by the identity toshokan samples. A
  write from outside that keeps its length and changes only bytes the identity
  does not sample is not seen, and an edit held over the file still matches its
  steps.
- A change from outside to a file drawbar holds is noticed by the file's length
  and the time it was written. On a volume that keeps that time to the second, a
  write of the same length in the second of drawbar's own is not seen. The limit
  is older than this change. The undo of a save, and the read that follows a
  lost answer, now rest on it too, and a program file is always one length.
- The library checks a file before it changes it, and does not look again as
  it moves the file away. A write from another program in between goes to the
  trash unseen, in a save as in an undo (#382). When that happens in the undo of
  a save, redoing the save puts the other program's bytes back. The document
  reads saved by then and follows them, so its edit is lost. The answer to a
  redo does not say what was put.
- After an edit ends Redo, toshokan keeps the steps it would have redone, and
  the trash keeps their bytes, until the next commit.
- A long visit to a note is one step. The field's own undo covers the typing
  only while focus stays.
- Sample instruments held whole and WAV takes keep a body per step, so few of
  their steps fit until they are held as edits. Held as edits, they lose their
  edit steps at each save.
- An outside change that a document takes ends the undo of its earlier edits.
- A step holds one document. An edit made to several sounds at once, which
  nothing makes today, would be a step for each.
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
saved document adopts it as it did before, and the edit steps behind the save no
longer match. It needs nothing that rule 2 needs. The cost is that one ⌘Z after
a save takes back every edit since the save before. It is where documents edited
in their files start, and the fallback for any type where rule 2 proves fragile.

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

The take editor had its own Undo and Redo while
[jmoo/drawbar#321](https://github.com/jmoo/drawbar/pull/321) was in review. It
was taken out before the merge so that a history for the whole app could be
designed on its own. The commits stay in that pull request's history:

| Commit | What it did |
|---|---|
| [`3f96b14f`](https://github.com/jmoo/drawbar/commit/3f96b14fac171de8fc7cec69b97612e2200cdb6b) | Undo for the open WAV |
| [`b5f3c533`](https://github.com/jmoo/drawbar/commit/b5f3c5337b32571683c5d23f20c6ef537a200083) | One Undo and Redo across the library and the open WAV |
| [`c7822762`](https://github.com/jmoo/drawbar/commit/c782276281fc5ff7f753b4cfecbbc4ba21694ceb) | The WAV's history outlives a trip to another tab |
| [`3e446854`](https://github.com/jmoo/drawbar/commit/3e446854113facecf4820c354aab26e5da05dc4a) | The removal |

Its design had four parts:

- **A step held settings, not samples.** A step was the gain, the marks and the
  settings the take had before the edit, with a label such as "gain +3.0 dB".
  Undo made the bytes again from the take's samples at no gain and the step
  (`wav::restored`). A step's size did not grow with the take's length.
- **Two histories and one clock.** The library kept its history and the open
  document its own. Each step on either side was stamped from one counter
  (`timeline::tick`). Undo went to the side stamped later. Redo went to the side
  undone later, unless the other side had changed since.
- **Undo was an edit.** Undo and Redo reached the document as sets with reserved
  paths, through the queue that edits use.
- **The history was editor state.** It lived with the open WAV's state, which
  was parked when another tab came to the front.

This RFC takes the first part. It is the `Edit` that
[Large documents](#large-documents) gives a WAV take, with the file in place of
samples held in memory, and its labels answer the question about labels under
[Unresolved questions](#unresolved-questions).

It does not take the second. Two histories need a rule about each other, which
`timeline::redoes` was, and each further history needs one against all the
rest. In one timeline a new step ends the redo, whichever kind it is.

Review of the pull request found four faults, and each has a counterpart here:

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
  compare content and drop the history on a mismatch. The comparison each edit
  step makes is that rule, applied per step, and an edit over a file carries its
  base in the value compared.
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

- **Documents edited in their files.** Should sample instruments and piano
  libraries keep their edit steps across a save, and is it worth it? A save of
  one is rare and heavy, and its undo moves a large file.
- **Delete.** Deleting an asset with an unsaved edit drops the edit. Undoing the
  delete brings the file back and the edit stays gone. Should the edit come
  back?
- **Bringing the document to the front.** Should the first ⌘Z on a document that
  is not showing only show it, and the second undo?
- **A step let go.** Should the activity log say when Undo stops reaching a
  sound's earlier edits, or is the menu no longer offering them enough?
- **Keep both.** Keeping both sides of a conflict moves the edit to a copy and
  records no step, so the original's earlier edit steps are let go. Should they
  follow the edit to the copy?
- **`STEPS` and `BYTES`.** 1,000 steps and 256 MiB are placeholders. The browser
  may want less.
- **What uses memory in the browser.** It has not been measured. Decoded audio
  and the limit on files read whole may weigh more than working copies, and
  neither is touched here. A heap profile of a session should order the rows of
  the Large documents table against them.
- **Kept slot views.** What lets go of an edit kept over a slot that is never
  viewed again, and of the bytes behind it: an age, a size, or a list the user
  can clear?
- **Notes.** Is one step per visit to a note too coarse? A pointer press already
  closes it, and the note editor could close it on its own terms.
- **One gesture over two documents.** A step extends only while it is the last
  one, so a result that lands on another document in the middle of a drag splits
  the drag in two.
- **The label.** "Edit" names no field. The editor knows the field at the point
  of the change and could pass its name.

Implementation updates the guide's Undo sections in
`docs/src/drawbar/editing.md` and `docs/src/drawbar/organize.md` in the same
change.

## Future possibilities

- A history panel listing the timeline's steps.
- Field names in labels: "Undo Drawbar 3".
- No confirmation before a detection replaces a take's marks. It asks today
  because nothing can take it back.
- Undo beside a line of the activity log. The timeline undoes in order, so
  undoing one line out of order needs its own rule.
- A timeline that survives a restart. It needs toshokan's history to survive one
  first.
