//! The session's steps that Undo takes back, in one list: the changes the library logged,
//! which the library undoes, and the edits to what an editor holds of a document, which
//! are undone in memory.
//!
//! An edit step applies only where its document holds exactly what the step recorded. A
//! working copy is all of a document's content, so nothing else needs checking, and a
//! step whose document moved on is let go when Undo reaches it.

use std::collections::VecDeque;

use eframe::egui;
use toshokan::{EntryHash, Identity};

use crate::store::Waiting;
use crate::workspace::Working;

/// How many steps are kept, and how many bytes of working copies they may hold, counting
/// both ends of each step. Past either, the oldest steps go.
pub const STEPS: usize = 1000;
pub const BYTES: usize = 256 << 20;

/// The documents whose working copies edit steps hold.
pub trait Documents {
    /// Whether the document holds exactly `working`. Where it holds an equal body in
    /// another allocation, `working` takes the document's, so the next comparison is of
    /// pointers.
    fn holds(&self, id: u64, working: &mut Working) -> bool;
    /// Put `working` back, and return whether the document holds it now.
    fn hold(&mut self, id: u64, working: &Working) -> bool;
}

/// What keeps an edit step open to further changes: the gesture that made it goes on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gesture {
    /// Nothing goes on, so a change is a step of its own.
    Over,
    /// A pointer button pressed in an earlier frame is down, or was let go in this one.
    Dragging,
    /// This widget has keyboard focus.
    Focused(egui::Id),
}

/// A document a save wrote from its body: the file the save replaced and the file it
/// left. The save's undo leaves the first and its redo the second, and neither changes
/// what the editor holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Saved {
    pub id: u64,
    pub before: Identity,
    pub after: Identity,
}

/// A change the library logged, which the library undoes.
#[derive(Clone, PartialEq, Debug)]
pub struct Logged {
    label: String,
    /// The entry its undo or redo reverses.
    entry: EntryHash,
    /// Empty unless the change did nothing but save documents from their bodies.
    saved: Vec<Saved>,
}

/// What one gesture changed in what the editor holds of a document.
#[derive(Clone, PartialEq, Debug)]
struct Edited {
    label: String,
    id: u64,
    before: Working,
    after: Working,
}

#[derive(Clone, PartialEq, Debug)]
enum Step {
    Library(Logged),
    Edit(Edited),
}

impl Step {
    fn size(&self) -> usize {
        match self {
            Step::Library(_) => 0,
            Step::Edit(step) => step.before.size() + step.after.size(),
        }
    }

    fn label(&self) -> &str {
        match self {
            Step::Library(step) => &step.label,
            Step::Edit(step) => &step.label,
        }
    }

    /// Whether Undo may leave this step and take an edit from beneath it.
    fn is_save(&self) -> bool {
        matches!(self, Step::Library(step) if !step.saved.is_empty())
    }
}

/// A library step out of its list while the library is asked to undo or redo it.
#[derive(PartialEq, Debug)]
pub struct Taken {
    step: Logged,
    /// How many steps had been recorded when it was taken.
    recorded: u64,
}

impl Taken {
    pub fn label(&self) -> &str {
        &self.step.label
    }

    pub fn saved(&self) -> &[Saved] {
        &self.step.saved
    }
}

/// What the library was asked, which its answer is followed by.
#[derive(PartialEq, Debug)]
pub enum Asked {
    /// A change, as the Edit menu names it, with the documents it saved from their
    /// bodies where it did nothing else.
    Change {
        label: String,
        saved: Vec<Saved>,
    },
    /// A change no undo takes back.
    Lasting,
    Undo(Taken),
    Redo(Taken),
}

/// How the library answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Answer {
    /// The entry that logged the step, where it was made.
    pub made: Option<EntryHash>,
    pub undoable: bool,
    /// The library let go of the step an undo or redo would have reversed.
    pub forgot: bool,
    /// The entry that logged the step that waited, which this one finished first.
    pub finished: Option<EntryHash>,
    pub waiting: Waiting,
}

/// The step Undo or Redo takes next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Next<'a> {
    Library { label: &'a str },
    Edit { label: &'a str, id: u64 },
}

impl Next<'_> {
    pub fn label(&self) -> &str {
        match self {
            Next::Library { label } | Next::Edit { label, .. } => label,
        }
    }
}

/// What an Undo or Redo did.
#[derive(PartialEq, Debug)]
pub enum Went {
    Nowhere,
    /// It put back what the editor held of this document.
    Edit(u64),
    /// The library is to be asked. One never asked goes back with [`Timeline::put_back`].
    Library(Taken),
}

#[derive(Default)]
pub struct Timeline {
    /// The steps an undo takes back, last last.
    done: VecDeque<Step>,
    /// The steps a redo makes again, last last.
    undone: Vec<Step>,
    /// The gesture that keeps the last step of `done` open. Never [`Gesture::Over`].
    open: Option<Gesture>,
    /// What the library was asked that failed partway and waits for the next write to
    /// finish it.
    waiting: Option<Asked>,
    /// Counts the steps recorded.
    recorded: u64,
    /// The bytes the steps of both lists hold.
    bytes: usize,
}

impl Timeline {
    /// Follow what one gesture made of a document's working copy. A change the open
    /// step's gesture goes on making extends that step.
    pub fn record(
        &mut self,
        id: u64,
        label: String,
        before: Working,
        after: Working,
        gesture: Gesture,
    ) {
        if before == after {
            return;
        }
        if self.open == Some(gesture) {
            if let Some(Step::Edit(open)) = self.done.back_mut() {
                if open.id == id && open.after == before {
                    self.bytes = self.bytes - open.after.size() + after.size();
                    open.after = after;
                    return;
                }
            }
        }
        self.close();
        self.end_redo();
        let step = Edited {
            label,
            id,
            before,
            after,
        };
        if self.keep(Step::Edit(step)) && gesture != Gesture::Over {
            self.open = Some(gesture);
        }
    }

    /// The frame ended with `gesture` going on. A step another gesture opened closes,
    /// and the steps Undo and Redo would take next are let go where their documents no
    /// longer hold what they recorded.
    pub fn settle(&mut self, gesture: Gesture, documents: &impl Documents) {
        if self.open.is_some_and(|open| open != gesture) {
            self.close();
        }
        while let Some(at) = self.undoes() {
            let Step::Edit(step) = &mut self.done[at] else {
                break;
            };
            if documents.holds(step.id, &mut step.after) {
                break;
            }
            self.let_go(at);
        }
        while let Some(Step::Edit(step)) = self.undone.last_mut() {
            if documents.holds(step.id, &mut step.before) {
                break;
            }
            self.bytes -= self.undone.pop().map_or(0, |step| step.size());
        }
    }

    /// The step Undo takes: the last, or an edit beneath the saves that end the list,
    /// which a save and the edits before it leave independent of each other.
    fn undoes(&self) -> Option<usize> {
        let last = self.done.len().checked_sub(1)?;
        let beneath = self.done.iter().rposition(|step| !step.is_save());
        match beneath {
            Some(at) if matches!(self.done[at], Step::Edit(_)) => Some(at),
            Some(_) | None => Some(last),
        }
    }

    pub fn next_undo(&self) -> Option<Next<'_>> {
        self.undoes().map(|at| next(&self.done[at]))
    }

    pub fn next_redo(&self) -> Option<Next<'_>> {
        self.undone.last().map(next)
    }

    /// Take back the step [`Timeline::next_undo`] names. An edit step whose document
    /// holds something else is let go, and the step behind it is tried.
    pub fn undo(&mut self, documents: &mut impl Documents) -> Went {
        self.close();
        while let Some(at) = self.undoes() {
            let Some(step) = self.done.remove(at) else {
                break;
            };
            let size = step.size();
            self.bytes -= size;
            match step {
                Step::Library(step) => return Went::Library(self.taken(step)),
                Step::Edit(mut step) => {
                    if documents.holds(step.id, &mut step.after)
                        && documents.hold(step.id, &step.before)
                    {
                        let id = step.id;
                        self.bytes += size;
                        self.undone.push(Step::Edit(step));
                        return Went::Edit(id);
                    }
                }
            }
        }
        Went::Nowhere
    }

    /// Make again the step [`Timeline::next_redo`] names.
    pub fn redo(&mut self, documents: &mut impl Documents) -> Went {
        self.close();
        while let Some(step) = self.undone.pop() {
            let size = step.size();
            self.bytes -= size;
            match step {
                Step::Library(step) => return Went::Library(self.taken(step)),
                Step::Edit(mut step) => {
                    if documents.holds(step.id, &mut step.before)
                        && documents.hold(step.id, &step.after)
                    {
                        let id = step.id;
                        self.bytes += size;
                        self.done.push_back(Step::Edit(step));
                        return Went::Edit(id);
                    }
                }
            }
        }
        Went::Nowhere
    }

    fn taken(&self, step: Logged) -> Taken {
        Taken {
            step,
            recorded: self.recorded,
        }
    }

    /// Put a step the library was not asked about, or refused to change, back at the
    /// end of the list it left.
    pub fn put_back(&mut self, asked: Asked) {
        match asked {
            Asked::Undo(taken) => self.done.push_back(Step::Library(taken.step)),
            Asked::Redo(taken) => self.undone.push(Step::Library(taken.step)),
            Asked::Change { .. } | Asked::Lasting => {}
        }
    }

    /// Follow the library's answer to `asked`. Returns what can no longer be done with
    /// the step a refused undo or redo would have reversed, where the library let go of
    /// it.
    pub fn answered(&mut self, asked: Option<Asked>, answer: Answer) -> Option<String> {
        let earlier = match (answer.finished, self.waiting.take()) {
            (Some(entry), Some(earlier)) => {
                self.took(earlier, entry);
                None
            }
            (_, earlier) => earlier,
        };
        match answer.waiting {
            Waiting::Earlier => self.waiting = earlier,
            Waiting::Nothing | Waiting::This => earlier.into_iter().for_each(|e| self.put_back(e)),
        }
        let asked = asked?;
        if answer.forgot {
            return forgotten(asked);
        }
        match answer.made {
            Some(entry) if answer.undoable || asked == Asked::Lasting => self.took(asked, entry),
            _ if answer.waiting == Waiting::This => self.waiting = Some(asked),
            _ => self.put_back(asked),
        }
        None
    }

    /// Follow a step the library made, which `entry` logged.
    fn took(&mut self, asked: Asked, entry: EntryHash) {
        match asked {
            Asked::Change { label, saved } => {
                self.close();
                self.end_redo();
                self.keep(Step::Library(Logged {
                    label,
                    entry,
                    saved,
                }));
            }
            // Logging any step ends the library's redo, as a change does.
            Asked::Lasting => self.end_redo(),
            // A step recorded since the undo was asked ended redo, this one included.
            Asked::Undo(taken) if taken.recorded == self.recorded => {
                self.undone.push(Step::Library(Logged {
                    entry,
                    ..taken.step
                }));
            }
            Asked::Undo(_) => {}
            Asked::Redo(taken) => self.done.push_back(Step::Library(Logged {
                entry,
                ..taken.step
            })),
        }
    }

    /// Let go of the steps whose undo or redo needs what `entries` displaced, and of
    /// every step behind them. Returns the latest change Undo no longer reaches.
    pub fn trim(&mut self, entries: &[EntryHash]) -> Option<String> {
        let needs =
            |step: &Step| matches!(step, Step::Library(step) if entries.contains(&step.entry));
        if let Some(at) = self.undone.iter().rposition(needs) {
            self.undone.drain(..=at);
        }
        let at = self.done.iter().rposition(needs);
        let gone = at.and_then(|at| self.done.drain(..=at).next_back());
        self.count();
        gone.map(|step| step.label().to_string())
    }

    /// Let go of every step the library logged and of every step behind one, as the
    /// library's own history ends.
    pub fn forget_library(&mut self) {
        let logged = |step: &Step| matches!(step, Step::Library(_));
        if let Some(at) = self.undone.iter().rposition(logged) {
            self.undone.drain(..=at);
        }
        if let Some(at) = self.done.iter().rposition(logged) {
            self.done.drain(..=at);
        }
        self.waiting = None;
        self.count();
    }

    /// Close the open step, so the next change is a step of its own. One that left its
    /// document as it found it goes.
    pub fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        if let Some(Step::Edit(step)) = self.done.back() {
            if step.before == step.after {
                self.let_go(self.done.len() - 1);
            }
        }
    }

    fn let_go(&mut self, at: usize) {
        self.bytes -= self.done.remove(at).map_or(0, |step| step.size());
    }

    /// A new step ends redo. The library keeps what it would have redone until its next
    /// change, and is never asked for it.
    fn end_redo(&mut self) {
        self.recorded += 1;
        self.bytes -= self.undone.drain(..).map(|step| step.size()).sum::<usize>();
    }

    /// Keep a new step within the limits, letting the oldest go, and return whether it
    /// was kept. One too large to keep cuts the timeline, since Undo skips no change.
    fn keep(&mut self, step: Step) -> bool {
        let size = step.size();
        if size > BYTES {
            self.done.clear();
            self.bytes = 0;
            return false;
        }
        self.done.push_back(step);
        self.bytes += size;
        while self.done.len() > STEPS || self.bytes > BYTES {
            self.let_go(0);
        }
        true
    }

    fn count(&mut self) {
        let steps = self.done.iter().chain(&self.undone);
        self.bytes = steps.map(Step::size).sum();
    }
}

fn next(step: &Step) -> Next<'_> {
    match step {
        Step::Library(step) => Next::Library { label: &step.label },
        Step::Edit(step) => Next::Edit {
            label: &step.label,
            id: step.id,
        },
    }
}

fn forgotten(asked: Asked) -> Option<String> {
    match asked {
        Asked::Undo(taken) => Some(format!("{} cannot be undone", taken.step.label)),
        Asked::Redo(taken) => Some(format!("{} cannot be made again", taken.step.label)),
        Asked::Change { .. } | Asked::Lasting => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default, Clone, PartialEq, Debug)]
    struct Held(BTreeMap<u64, Working>);

    impl Documents for Held {
        fn holds(&self, id: u64, working: &mut Working) -> bool {
            self.0.get(&id) == Some(working)
        }

        fn hold(&mut self, id: u64, working: &Working) -> bool {
            self.0.insert(id, working.clone());
            true
        }
    }

    fn body(byte: u8) -> Working {
        Working::Body(vec![byte].into())
    }

    fn entry(n: u128) -> EntryHash {
        EntryHash::from_u128(n)
    }

    fn made(n: u128) -> Answer {
        Answer {
            made: Some(entry(n)),
            undoable: true,
            forgot: false,
            finished: None,
            waiting: Waiting::Nothing,
        }
    }

    fn refused() -> Answer {
        Answer {
            made: None,
            undoable: false,
            ..made(0)
        }
    }

    fn change(label: &str) -> Asked {
        Asked::Change {
            label: label.to_string(),
            saved: Vec::new(),
        }
    }

    fn save(id: u64) -> Asked {
        let saved = Saved {
            id,
            before: Identity::from_u128(1),
            after: Identity::from_u128(2),
        };
        Asked::Change {
            label: format!("Save “{id}”"),
            saved: vec![saved],
        }
    }

    /// One session: the documents, and the timeline that follows the edits made of them.
    #[derive(Default)]
    struct Session {
        held: Held,
        timeline: Timeline,
        logged: u128,
    }

    impl Session {
        fn edit(&mut self, id: u64, byte: u8, gesture: Gesture) {
            let before = self.held.0.get(&id).cloned().unwrap_or_else(|| body(0));
            self.held.0.insert(id, body(byte));
            let label = format!("Edit “{id}”");
            self.timeline.record(id, label, before, body(byte), gesture);
            self.timeline.settle(gesture, &self.held);
        }

        fn click(&mut self, id: u64, byte: u8) {
            self.edit(id, byte, Gesture::Over);
        }

        fn log(&mut self, asked: Asked) {
            self.logged += 1;
            self.timeline.answered(Some(asked), made(self.logged));
        }

        /// Undo, with the library answering at once that it made the step.
        fn undo(&mut self) -> Option<String> {
            match self.timeline.undo(&mut self.held) {
                Went::Nowhere => None,
                Went::Edit(id) => Some(format!("Edit “{id}”")),
                Went::Library(taken) => {
                    let label = taken.label().to_string();
                    self.logged += 1;
                    let answer = made(self.logged);
                    self.timeline.answered(Some(Asked::Undo(taken)), answer);
                    Some(label)
                }
            }
        }

        fn redo(&mut self) -> Option<String> {
            match self.timeline.redo(&mut self.held) {
                Went::Nowhere => None,
                Went::Edit(id) => Some(format!("Edit “{id}”")),
                Went::Library(taken) => {
                    let label = taken.label().to_string();
                    self.logged += 1;
                    let answer = made(self.logged);
                    self.timeline.answered(Some(Asked::Redo(taken)), answer);
                    Some(label)
                }
            }
        }

        fn holds(&self, id: u64) -> Working {
            self.held.0[&id].clone()
        }

        fn undo_reads(&self) -> Option<&str> {
            self.timeline.next_undo().map(|next| match next {
                Next::Library { label } | Next::Edit { label, .. } => label,
            })
        }

        fn redo_reads(&self) -> Option<&str> {
            self.timeline.next_redo().map(|next| match next {
                Next::Library { label } | Next::Edit { label, .. } => label,
            })
        }
    }

    #[test]
    fn undo_puts_back_what_an_edit_replaced_and_redo_makes_it_again() {
        let mut session = Session::default();
        session.click(1, 3);
        session.click(1, 8);

        assert_eq!(session.undo().as_deref(), Some("Edit “1”"));
        assert_eq!(session.holds(1), body(3));
        assert_eq!(session.undo().as_deref(), Some("Edit “1”"));
        assert_eq!(session.holds(1), body(0));
        assert_eq!(session.undo(), None, "nothing is left to undo");

        session.redo();
        session.redo();
        assert_eq!(session.holds(1), body(8));
        assert_eq!(session.redo(), None, "nothing is left to redo");
    }

    #[test]
    fn a_drag_is_one_step_and_each_click_is_its_own() {
        let mut session = Session::default();
        for byte in 1..=5 {
            session.edit(1, byte, Gesture::Dragging);
        }
        session.timeline.settle(Gesture::Over, &session.held);
        session.click(1, 6);
        session.click(1, 7);

        session.undo();
        assert_eq!(session.holds(1), body(6));
        session.undo();
        assert_eq!(
            session.holds(1),
            body(5),
            "the second click is undone alone"
        );
        session.undo();
        assert_eq!(session.holds(1), body(0), "the drag is undone whole");
    }

    #[test]
    fn typing_in_one_box_is_one_step_until_focus_leaves_it() {
        let (name, notes) = (egui::Id::new("name"), egui::Id::new("notes"));
        let mut session = Session::default();
        session.edit(1, 1, Gesture::Focused(name));
        session.edit(1, 2, Gesture::Focused(name));
        session.edit(1, 3, Gesture::Focused(notes));
        session.edit(1, 4, Gesture::Focused(notes));

        session.undo();
        assert_eq!(session.holds(1), body(2), "the second box is undone whole");
        session.undo();
        assert_eq!(session.holds(1), body(0));
    }

    #[test]
    fn a_drag_that_ends_where_it_began_leaves_no_step() {
        let mut session = Session::default();
        session.click(1, 4);
        session.edit(1, 9, Gesture::Dragging);
        session.edit(1, 4, Gesture::Dragging);
        session.timeline.settle(Gesture::Over, &session.held);

        session.undo();
        assert_eq!(session.holds(1), body(0), "undo takes the click back");
    }

    #[test]
    fn a_gesture_on_another_document_is_a_step_of_its_own() {
        let mut session = Session::default();
        session.edit(1, 1, Gesture::Dragging);
        session.edit(2, 1, Gesture::Dragging);

        session.undo();
        assert_eq!((session.holds(1), session.holds(2)), (body(1), body(0)));
    }

    #[test]
    fn undo_takes_an_edit_from_beneath_a_save_and_leaves_the_save() {
        let mut session = Session::default();
        session.click(1, 3);
        session.log(save(1));
        assert_eq!(session.undo_reads(), Some("Edit “1”"));

        assert_eq!(session.undo().as_deref(), Some("Edit “1”"));
        assert_eq!(session.holds(1), body(0));
        assert_eq!(session.undo_reads(), Some("Save “1”"), "the save is next");
        assert_eq!(session.redo_reads(), Some("Edit “1”"));

        session.redo();
        assert_eq!(session.holds(1), body(3));
        assert_eq!(session.undo_reads(), Some("Edit “1”"), "it is redone above");
    }

    #[test]
    fn undo_stops_at_a_save_with_a_change_to_files_behind_it() {
        let mut session = Session::default();
        session.click(1, 3);
        session.log(change("Rename “1”"));
        session.log(save(2));

        assert_eq!(session.undo().as_deref(), Some("Save “2”"));
        assert_eq!(session.undo().as_deref(), Some("Rename “1”"));
        assert_eq!(session.undo().as_deref(), Some("Edit “1”"));
    }

    #[test]
    fn an_edit_step_is_let_go_where_its_document_holds_something_else() {
        let mut session = Session::default();
        session.click(1, 3);
        session.click(2, 5);
        session.click(1, 4);
        // The document follows its file, which no step records.
        session.held.0.insert(1, body(9));

        assert_eq!(session.undo().as_deref(), Some("Edit “2”"));
        assert_eq!(session.holds(1), body(9), "no step touched it");
        assert_eq!(session.undo(), None, "its earlier step went as well");
    }

    #[test]
    fn a_frame_lets_go_of_the_step_undo_would_take_once_its_document_moved_on() {
        let mut session = Session::default();
        session.click(1, 3);
        session.held.0.insert(1, body(9));
        assert_eq!(session.undo_reads(), Some("Edit “1”"));

        session.timeline.settle(Gesture::Over, &session.held);

        assert_eq!(session.undo_reads(), None, "the menu names no dead step");
    }

    #[test]
    fn a_step_is_good_again_once_its_document_holds_what_it_recorded() {
        let mut session = Session::default();
        session.click(1, 3);
        session.click(2, 1);
        session.held.0.insert(1, body(9));
        session.held.0.insert(1, body(3));

        session.undo();
        assert_eq!(session.undo().as_deref(), Some("Edit “1”"));
        assert_eq!(session.holds(1), body(0));
    }

    #[test]
    fn a_new_step_ends_redo() {
        let mut session = Session::default();
        session.click(1, 3);
        session.log(change("Rename “1”"));
        session.undo();
        session.undo();
        assert_eq!(session.redo_reads(), Some("Edit “1”"));

        session.click(2, 1);

        assert_eq!(session.redo_reads(), None);
        assert_eq!(session.redo(), None);
    }

    #[test]
    fn a_library_step_undone_while_an_edit_was_made_is_not_redone() {
        let mut session = Session::default();
        session.log(change("Rename “1”"));
        let Went::Library(taken) = session.timeline.undo(&mut session.held) else {
            panic!("the rename is undone by the library");
        };
        session.click(1, 3);

        session.timeline.answered(Some(Asked::Undo(taken)), made(9));

        assert_eq!(session.redo_reads(), None, "the edit ended redo");
        assert_eq!(session.undo_reads(), Some("Edit “1”"));
    }

    #[test]
    fn a_library_step_redone_while_an_edit_was_made_is_undone_after_it() {
        let mut session = Session::default();
        session.log(change("Rename “1”"));
        session.undo();
        let Went::Library(taken) = session.timeline.redo(&mut session.held) else {
            panic!("the rename is redone by the library");
        };
        session.click(1, 3);

        session.timeline.answered(Some(Asked::Redo(taken)), made(9));

        assert_eq!(session.undo().as_deref(), Some("Rename “1”"));
        assert_eq!(session.undo().as_deref(), Some("Edit “1”"));
    }

    #[test]
    fn a_refused_undo_leaves_its_step_to_be_undone() {
        let mut session = Session::default();
        session.log(change("Rename “1”"));
        let Went::Library(taken) = session.timeline.undo(&mut session.held) else {
            panic!("the rename is undone by the library");
        };
        assert_eq!(
            session.undo_reads(),
            None,
            "it is out while the library is asked"
        );

        let said = session
            .timeline
            .answered(Some(Asked::Undo(taken)), refused());

        assert_eq!(said, None);
        assert_eq!(session.undo_reads(), Some("Rename “1”"));
    }

    #[test]
    fn an_undo_the_library_let_go_of_is_named_and_gone() {
        let mut session = Session::default();
        session.log(change("Rename “1”"));
        let Went::Library(taken) = session.timeline.undo(&mut session.held) else {
            panic!("the rename is undone by the library");
        };
        let forgot = Answer {
            forgot: true,
            ..refused()
        };

        let said = session.timeline.answered(Some(Asked::Undo(taken)), forgot);

        assert_eq!(said.as_deref(), Some("Rename “1” cannot be undone"));
        assert_eq!((session.undo_reads(), session.redo_reads()), (None, None));
    }

    #[test]
    fn a_trimmed_step_and_every_step_behind_it_leave() {
        let mut session = Session::default();
        session.click(1, 3);
        session.log(change("Rename “1”"));
        session.click(1, 4);
        session.log(change("Delete “2”"));
        session.undo();
        assert_eq!(
            session.timeline.trim(&[entry(7)]),
            None,
            "nothing needed it"
        );

        let gone = session.timeline.trim(&[entry(1)]);

        assert_eq!(gone.as_deref(), Some("Rename “1”"));
        assert_eq!(
            session.redo_reads(),
            Some("Delete “2”"),
            "the redo needs none"
        );
        assert_eq!(session.undo().as_deref(), Some("Edit “1”"));
        assert_eq!(session.holds(1), body(3));
        assert_eq!(
            session.undo(),
            None,
            "the edit behind the rename went with it"
        );
    }

    #[test]
    fn the_end_of_the_librarys_history_leaves_the_edits_made_since() {
        let mut session = Session::default();
        session.click(1, 3);
        session.log(change("Rename “1”"));
        session.click(1, 4);

        session.timeline.forget_library();

        assert_eq!(session.undo().as_deref(), Some("Edit “1”"));
        assert_eq!(session.undo(), None);
    }

    #[test]
    fn an_undo_that_failed_partway_is_followed_once_the_next_write_finishes_it() {
        let mut session = Session::default();
        session.log(change("Save “a”"));
        session.log(change("Save “b”"));
        let Went::Library(taken) = session.timeline.undo(&mut session.held) else {
            panic!("the save is undone by the library");
        };
        let partway = Answer {
            waiting: Waiting::This,
            ..refused()
        };
        session.timeline.answered(Some(Asked::Undo(taken)), partway);
        assert_eq!(session.undo_reads(), Some("Save “a”"));

        let finishing = Answer {
            finished: Some(entry(8)),
            ..refused()
        };
        session.timeline.answered(None, finishing);

        assert_eq!(session.redo_reads(), Some("Save “b”"));
        assert_eq!(session.undo_reads(), Some("Save “a”"));
    }

    #[test]
    fn an_undo_a_later_write_finished_is_redone_by_the_entry_that_finished_it() {
        let mut session = Session::default();
        session.log(change("Save “a”"));
        let Went::Library(taken) = session.timeline.undo(&mut session.held) else {
            panic!("the save is undone by the library");
        };
        let partway = Answer {
            waiting: Waiting::This,
            ..refused()
        };
        session.timeline.answered(Some(Asked::Undo(taken)), partway);
        let finishing = Answer {
            finished: Some(entry(8)),
            ..refused()
        };
        session.timeline.answered(None, finishing);

        assert_eq!(session.timeline.trim(&[entry(1)]), None);
        assert_eq!(session.redo_reads(), Some("Save “a”"));
        session.timeline.trim(&[entry(8)]);
        assert_eq!(session.redo_reads(), None);
    }

    #[test]
    fn a_change_that_failed_partway_is_a_step_only_once_it_is_finished() {
        let mut session = Session::default();
        let partway = Answer {
            waiting: Waiting::This,
            ..refused()
        };
        session.timeline.answered(Some(save(1)), partway);
        let untried = Answer {
            waiting: Waiting::Earlier,
            ..refused()
        };
        session.timeline.answered(None, untried);
        assert_eq!(session.undo_reads(), None, "it still waits");

        let finishing = Answer {
            finished: Some(entry(8)),
            ..refused()
        };
        session.timeline.answered(None, finishing);

        assert_eq!(session.undo_reads(), Some("Save “1”"));
    }

    #[test]
    fn a_change_that_failed_partway_and_no_longer_waits_is_no_step() {
        let mut session = Session::default();
        let partway = Answer {
            waiting: Waiting::This,
            ..refused()
        };
        session.timeline.answered(Some(save(1)), partway);

        session.timeline.answered(None, refused());
        let finishing = Answer {
            finished: Some(entry(8)),
            ..refused()
        };
        session.timeline.answered(None, finishing);

        assert_eq!(session.undo_reads(), None);
    }

    #[test]
    fn the_steps_kept_are_bounded_by_their_count() {
        let mut session = Session::default();
        for n in 0..STEPS + 10 {
            session.click(1, (n % 250) as u8 + 1);
        }

        let mut undone = 0;
        while session.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, STEPS);
    }

    #[test]
    fn the_steps_kept_are_bounded_by_their_bytes() {
        let large = |byte: u8| Working::Body(vec![byte; BYTES / 5].into());
        let mut timeline = Timeline::default();
        for byte in 0..4 {
            let (before, after) = (large(byte), large(byte + 1));
            timeline.record(1, "Edit".to_string(), before, after, Gesture::Over);
        }

        assert!(timeline.bytes <= BYTES);
        assert_eq!(
            timeline.done.len(),
            2,
            "each step holds two fifths of the limit"
        );
    }

    #[test]
    fn a_step_too_large_to_keep_ends_what_undo_reaches() {
        let large = |byte: u8| Working::Body(vec![byte; BYTES / 2 + 1].into());
        let mut session = Session::default();
        session.click(1, 3);

        let (before, after) = (large(1), large(2));
        session
            .timeline
            .record(1, "Edit".to_string(), before, after, Gesture::Dragging);

        assert_eq!(session.undo_reads(), None);
        assert_eq!(session.timeline.bytes, 0);
        assert_eq!(session.timeline.open, None);
    }

    /// The library as the timeline must keep in step with it: what it would undo and
    /// redo, and the file each document was saved as.
    #[derive(Default, Clone, PartialEq, Debug)]
    struct Library {
        done: Vec<(String, Option<(u64, Working, Working)>)>,
        undone: Vec<(String, Option<(u64, Working, Working)>)>,
        files: BTreeMap<u64, Working>,
    }

    impl Library {
        fn commit(&mut self, label: String, saved: Option<(u64, Working)>) {
            let saved = saved.map(|(id, now)| {
                let was = self
                    .files
                    .insert(id, now.clone())
                    .unwrap_or_else(|| body(0));
                (id, was, now)
            });
            self.done.push((label, saved));
            self.undone.clear();
        }

        fn undo(&mut self, label: &str) {
            let step = self.done.pop().expect("the library has a step to undo");
            assert_eq!(step.0, label, "the library undoes its own last step");
            if let Some((id, was, _)) = &step.1 {
                self.files.insert(*id, was.clone());
            }
            self.undone.push(step);
        }

        fn redo(&mut self, label: &str) {
            let step = self.undone.pop().expect("the library has a step to redo");
            assert_eq!(step.0, label, "the library redoes what it undid last");
            if let Some((id, _, now)) = &step.1 {
                self.files.insert(*id, now.clone());
            }
            self.done.push(step);
        }
    }

    struct Walk {
        session: Session,
        library: Library,
        seed: u64,
    }

    impl Walk {
        fn roll(&mut self, sides: u64) -> u64 {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            self.seed % sides
        }

        fn state(&self) -> (Held, BTreeMap<u64, Working>) {
            (self.session.held.clone(), self.library.files.clone())
        }

        fn undo(&mut self) -> bool {
            match self.session.timeline.undo(&mut self.session.held) {
                Went::Nowhere => false,
                Went::Edit(_) => true,
                Went::Library(taken) => {
                    self.library.undo(taken.label());
                    self.session.logged += 1;
                    let answer = made(self.session.logged);
                    self.session
                        .timeline
                        .answered(Some(Asked::Undo(taken)), answer);
                    true
                }
            }
        }

        fn redo(&mut self) -> bool {
            match self.session.timeline.redo(&mut self.session.held) {
                Went::Nowhere => false,
                Went::Edit(_) => true,
                Went::Library(taken) => {
                    self.library.redo(taken.label());
                    self.session.logged += 1;
                    let answer = made(self.session.logged);
                    self.session
                        .timeline
                        .answered(Some(Asked::Redo(taken)), answer);
                    true
                }
            }
        }

        fn step(&mut self, n: u64) {
            let id = 1 + self.roll(3);
            match self.roll(8) {
                0..=2 => self.session.click(id, n as u8 + 1),
                3 => {
                    let label = format!("Save “{id}” {n}");
                    let now = self.session.held.0.get(&id).cloned();
                    let now = now.unwrap_or_else(|| body(0));
                    self.library.commit(label.clone(), Some((id, now)));
                    let saved = Saved {
                        id,
                        before: Identity::from_u128(1),
                        after: Identity::from_u128(2),
                    };
                    self.session.log(Asked::Change {
                        label,
                        saved: vec![saved],
                    });
                }
                4 => {
                    let label = format!("Rename {n}");
                    self.library.commit(label.clone(), None);
                    self.session.log(change(&label));
                }
                5 | 6 => {
                    let before = self.state();
                    if self.undo() {
                        let undone = self.state();
                        assert!(self.redo(), "what was undone is redone");
                        assert_eq!(self.state(), before, "redo puts back what undo took");
                        assert!(self.undo());
                        assert_eq!(self.state(), undone);
                    }
                }
                _ => {
                    self.redo();
                }
            }
        }
    }

    #[test]
    fn any_walk_asks_the_library_for_its_own_last_step_and_unwinds_to_the_start() {
        for seed in 1..=200 {
            let mut walk = Walk {
                session: Session::default(),
                library: Library::default(),
                seed,
            };
            for id in 1..=3 {
                walk.session.held.0.insert(id, body(0));
                walk.library.files.insert(id, body(0));
            }
            let start = walk.state();
            for n in 0..60 {
                walk.step(n);
            }
            while walk.undo() {}
            assert_eq!(walk.state(), start, "seed {seed} unwinds to where it began");
        }
    }
}
