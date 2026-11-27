//! Seeded walks of edits, saves, renames, reverts, outside changes and undos over one
//! library, each checked after every action against a model that keeps whole states.

use super::*;
use crate::document::{Document, Editors};
use crate::timeline::{Gesture, Next};

const DOCUMENTS: usize = 3;
const SEEDS: std::ops::RangeInclusive<u64> = 1..=12;
const STEPS: usize = 60;
const GAINS: [&str; 6] = ["10", "30", "50", "70", "90", "110"];

/// What one document is, on disk and in the editor.
#[derive(Clone, PartialEq, Debug)]
struct Whole {
    path: String,
    file: Vec<u8>,
    working: Vec<u8>,
}

type State = Vec<Whole>;

/// Where a document's file is and what it holds.
type Placed = (String, Vec<u8>);

/// A document's file before and after a library step, and how many times the file had
/// changed outside drawbar when the step last moved between Undo and Redo.
#[derive(Clone, Debug)]
struct Filed {
    doc: usize,
    before: Placed,
    after: Placed,
    outside: usize,
}

#[derive(Clone, Debug)]
enum Step {
    Edit {
        label: String,
        doc: usize,
        before: Vec<u8>,
        after: Vec<u8>,
    },
    Library {
        label: String,
        save: bool,
        files: Vec<Filed>,
    },
}

/// What the model expects Undo or Redo to name.
#[derive(Clone, PartialEq, Debug)]
enum Named {
    Library(String),
    Edit(String, usize),
}

/// What became of a step Undo or Redo took.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Answer {
    Taken,
    /// The library refused it and let it go.
    Forgotten,
}

/// The steps Undo and Redo take, over the state they take them in.
struct Model {
    state: State,
    done: Vec<Step>,
    undone: Vec<Step>,
    /// How many times each document's file changed outside drawbar.
    outside: Vec<usize>,
    /// How many steps were let go, by the timeline or the library.
    let_go: usize,
}

impl Model {
    /// Record a new step. A save ends the redo of every step undone before a library
    /// step, and leaves the edits undone since it to be made again; any other step ends
    /// all redo.
    fn new_step(&mut self, mut step: Step) {
        self.stamp(&mut step);
        let library = |step: &Step| matches!(step, Step::Library { .. });
        match step {
            Step::Library { save: true, .. } => {
                if let Some(at) = self.undone.iter().rposition(library) {
                    self.undone.drain(..=at);
                }
            }
            _ => self.undone.clear(),
        }
        self.done.push(step);
    }

    /// Whether `step` is an edit whose document no longer holds what Undo, or Redo where
    /// `again`, needs.
    fn dead(&self, step: &Step, again: bool) -> bool {
        match step {
            Step::Edit {
                doc, before, after, ..
            } => !self.holds(*doc, if again { before } else { after }),
            Step::Library { .. } => false,
        }
    }

    fn stamp(&self, step: &mut Step) {
        if let Step::Library { files, .. } = step {
            for filed in files {
                filed.outside = self.outside[filed.doc];
            }
        }
    }

    /// Whether a file the library step names changed outside drawbar since it last moved,
    /// which leaves the library to decide whether it can still be taken.
    fn moved_on(&self, step: &Step) -> bool {
        let Step::Library { files, .. } = step else {
            return false;
        };
        files
            .iter()
            .any(|filed| filed.outside != self.outside[filed.doc])
    }

    /// The step Undo takes: the last, or an edit beneath the saves that end the list.
    fn undoes(&self) -> Option<usize> {
        let last = self.done.len().checked_sub(1)?;
        let is_save = |step: &Step| matches!(step, Step::Library { save: true, .. });
        match self.done.iter().rposition(|step| !is_save(step)) {
            Some(at) if matches!(self.done[at], Step::Edit { .. }) => Some(at),
            _ => Some(last),
        }
    }

    fn holds(&self, doc: usize, working: &[u8]) -> bool {
        self.state[doc].working == working
    }

    fn placed(&self, doc: usize) -> Placed {
        let whole = &self.state[doc];
        (whole.path.clone(), whole.file.clone())
    }

    /// As a frame ends: the next steps whose documents moved on are let go.
    fn settle(&mut self) {
        while let Some(at) = self.undoes() {
            match &self.done[at] {
                Step::Edit { doc, after, .. } if !self.holds(*doc, after) => {
                    self.done.remove(at);
                    self.let_go += 1;
                }
                _ => break,
            }
        }
        while let Some(Step::Edit { doc, before, .. }) = self.undone.last() {
            if self.holds(*doc, before) {
                break;
            }
            self.undone.pop();
            self.let_go += 1;
        }
    }

    fn name(step: &Step) -> Named {
        match step {
            Step::Edit { label, doc, .. } => Named::Edit(label.clone(), *doc),
            Step::Library { label, .. } => Named::Library(label.clone()),
        }
    }

    fn next_undo(&self) -> Option<Named> {
        self.undoes().map(|at| Model::name(&self.done[at]))
    }

    fn next_redo(&self) -> Option<Named> {
        self.undone.last().map(Model::name)
    }

    /// Put the documents where `step` leaves them, undone, or made `again`.
    fn take(&mut self, step: &Step, again: bool) {
        match step {
            Step::Edit {
                doc, before, after, ..
            } => self.state[*doc].working = if again { after } else { before }.clone(),
            Step::Library { files, .. } => {
                for filed in files {
                    let (path, file) = if again { &filed.after } else { &filed.before };
                    self.state[filed.doc].path = path.clone();
                    self.state[filed.doc].file = file.clone();
                }
            }
        }
    }

    /// Follow what became of the step at `at` of the list Undo, or Redo where `again`,
    /// takes from.
    fn answered(&mut self, again: bool, at: usize, answer: Answer) {
        let mut step = match again {
            false => self.done.remove(at),
            true => self.undone.remove(at),
        };
        match answer {
            Answer::Taken => {
                self.take(&step, again);
                self.stamp(&mut step);
                match again {
                    false => self.undone.push(step),
                    true => self.done.push(step),
                }
            }
            Answer::Forgotten => self.let_go += 1,
        }
    }
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Edit(usize),
    Save(usize),
    SaveAll,
    Rename(usize),
    Revert(usize),
    Outside(usize),
    RefusedSave(usize),
    Undo,
    Redo,
}

struct Walk {
    root: Temp,
    session: Session,
    document: Document,
    ids: Vec<u64>,
    bodies: Vec<Vec<u8>>,
    model: Model,
    rng: u64,
    seed: u64,
    at: usize,
    action: Action,
    /// Every file is given one modification time after each action, as a disk whose
    /// clock is coarse leaves files of one length looking alike.
    coarse: bool,
    /// The library refused the last step Undo or Redo took.
    refusal: bool,
    /// The document whose edit the last action undid.
    undid: Option<usize>,
    /// Undoing every step returns to the start: no file changed outside drawbar, and no
    /// save was refused.
    unwinds: bool,
    /// The state the walk starts from.
    start: State,
}

impl Walk {
    fn start(seed: u64) -> Walk {
        let root = Temp::new();
        let program = Fresh::Program.bytes().unwrap();
        let bodies: Vec<Vec<u8>> = GAINS.iter().map(|gain| with_gain(&program, gain)).collect();
        for (n, body) in bodies.iter().enumerate() {
            assert!(
                !bodies[..n].contains(body),
                "gain {} gives a body of its own",
                GAINS[n]
            );
        }
        // The last document is a file the library lists, and the rest are files drawbar
        // made and saved, whose steps the timeline then lets go of.
        let listed = "listed.ne5p";
        fs::write(root.at(listed), &bodies[DOCUMENTS - 1]).unwrap();
        let mut session = Session::open(&root);
        let mut ids: Vec<u64> = (1..DOCUMENTS).map(|_| session.create()).collect();
        session.sync();
        for (id, body) in ids.iter().zip(&bodies) {
            session.save(*id, body);
        }
        session.sync();
        session.timeline.forget_library();
        session.bench.workspace.take_notes();
        ids.push(session.named(listed));
        let state: State = ids
            .iter()
            .zip(&bodies)
            .map(|(id, body)| Whole {
                path: session.path(*id).expect("a file"),
                file: body.clone(),
                working: body.clone(),
            })
            .collect();
        let model = Model {
            state: state.clone(),
            done: Vec::new(),
            undone: Vec::new(),
            outside: vec![0; DOCUMENTS],
            let_go: 0,
        };
        let walk = Walk {
            root,
            session,
            document: Document::default(),
            ids,
            bodies,
            model,
            rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            seed,
            at: 0,
            action: Action::Undo,
            coarse: seed.is_multiple_of(2),
            refusal: false,
            undid: None,
            unwinds: true,
            start: state,
        };
        walk.check("the library opens");
        walk
    }

    fn roll(&mut self, sides: usize) -> usize {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % sides as u64) as usize
    }

    fn whence(&self) -> String {
        format!("seed {}, step {}, {:?}", self.seed, self.at, self.action)
    }

    /// A body other than each of `not`.
    fn other(&mut self, not: &[&[u8]]) -> Vec<u8> {
        loop {
            let at = self.roll(self.bodies.len());
            if !not.contains(&self.bodies[at].as_slice()) {
                return self.bodies[at].clone();
            }
        }
    }

    fn actual(&self) -> State {
        (0..DOCUMENTS)
            .map(|doc| {
                let id = self.ids[doc];
                let path = self.session.path(id).expect("a file");
                Whole {
                    file: fs::read(self.root.at(&path)).unwrap(),
                    working: self.session.bytes(id),
                    path,
                }
            })
            .collect()
    }

    fn name(&self, doc: usize) -> String {
        let entity = self.session.bench.workspace.get(self.ids[doc]);
        entity.expect("held").name.clone()
    }

    fn doc(&self, id: u64) -> usize {
        let at = self.ids.iter().position(|held| *held == id);
        at.expect("a walked document")
    }

    /// The end of a frame: each edit a user made is a step, and the steps the next Undo
    /// and Redo would take are let go where their documents moved on.
    fn frame(&mut self) {
        for note in self.session.bench.workspace.take_notes() {
            let workspace = &self.session.bench.workspace;
            let after = self.document.working(workspace, note.id).expect("held");
            let verb = if note.revert { "Revert" } else { "Edit" };
            let label = format!("{verb} “{}”", self.name(self.doc(note.id)));
            let timeline = &mut self.session.timeline;
            timeline.record(note.id, label, note.before, after, Gesture::Over);
        }
        let Bench { workspace, log, .. } = &mut self.session.bench;
        let editors = Editors {
            document: &mut self.document,
            workspace,
            log,
        };
        self.session.timeline.settle(Gesture::Over, &editors);
        self.model.settle();
    }

    /// Send everything asked, and wait until the library has answered it and nothing is
    /// in flight.
    fn pump(&mut self) {
        for _ in 0..50 {
            // ⚠️ The library's thread counts a command as run after it sent the command's
            // last answer, so the count is read before the answers are folded in, and no
            // answer is waited for once they all are.
            let ran = !self.session.store.writing();
            self.session.decided();
            let store = &self.session.store;
            let waits = store.changing() || store.rechecking();
            let answered = !waits && self.session.settled();
            match (answered, ran) {
                (true, true) => return,
                (true, false) => self.session.ran(),
                (false, _) => assert!(
                    self.session.next(),
                    "{}: the library answered",
                    self.whence()
                ),
            }
        }
        panic!("{}: the library is still deciding", self.whence());
    }

    fn edit(&mut self, doc: usize) {
        let whole = self.model.state[doc].clone();
        let body = self.other(&[&whole.working]);
        let label = format!("Edit “{}”", self.name(doc));
        self.session.put(self.ids[doc], &body);
        self.model.new_step(Step::Edit {
            label,
            doc,
            before: whole.working,
            after: body.clone(),
        });
        self.model.state[doc].working = body;
        self.frame();
    }

    /// Save each of `docs` in one change.
    fn save(&mut self, docs: &[usize]) {
        for doc in docs {
            self.session.bench.workspace.mark_saved(self.ids[*doc]);
        }
        self.pump();
        let mut files = Vec::new();
        for &doc in docs {
            let before = self.model.placed(doc);
            let whole = &mut self.model.state[doc];
            if whole.working != whole.file {
                whole.file = whole.working.clone();
                let after = self.model.placed(doc);
                let outside = 0;
                files.push(Filed {
                    doc,
                    before,
                    after,
                    outside,
                });
            }
        }
        let label = match files.as_slice() {
            [] => return self.frame(),
            [filed] => format!("Save “{}”", leaf(&filed.after.0)),
            more => format!("Save {} files", more.len()),
        };
        let save = true;
        self.model.new_step(Step::Library { label, save, files });
        self.frame();
    }

    fn rename(&mut self, doc: usize) {
        let before = self.model.placed(doc);
        let to = format!("{}{}.ne5p", char::from(b'a' + doc as u8), self.at);
        let place = LibPath::root().join(&to);
        self.session.bench.workspace.place(self.ids[doc], place);
        self.pump();
        self.model.state[doc].path = to;
        let after = self.model.placed(doc);
        let label = format!("Rename “{}”", leaf(&before.0));
        let outside = 0;
        let files = vec![Filed {
            doc,
            before,
            after,
            outside,
        }];
        let save = false;
        self.model.new_step(Step::Library { label, save, files });
        self.frame();
    }

    fn revert(&mut self, doc: usize) {
        let id = self.ids[doc];
        self.session
            .bench
            .act(vec![crate::browser::Act::Revert(id)]);
        self.pump();
        let whole = self.model.state[doc].clone();
        if whole.working != whole.file {
            self.model.new_step(Step::Edit {
                label: format!("Revert “{}”", self.name(doc)),
                doc,
                before: whole.working,
                after: whole.file.clone(),
            });
            self.model.state[doc].working = whole.file;
        }
        self.frame();
    }

    /// Another program writes the file. A document with nothing unsaved shows it, and one
    /// holding an edit asks, and keeps the edit or takes theirs as a revert does. Undo or
    /// Redo may follow in the same frame.
    fn outside(&mut self, doc: usize) {
        self.unwinds = false;
        let whole = self.model.state[doc].clone();
        let theirs = self.other(&[&whole.file, &whole.working]);
        write_later(&self.root, &whole.path, &theirs);
        self.session.refocus();
        self.pump();
        self.model.outside[doc] += 1;
        self.model.state[doc].file = theirs.clone();
        if whole.working == whole.file {
            self.model.state[doc].working = theirs;
        } else {
            let asking = self.session.bench.browser.asking();
            assert!(
                asking.is_some(),
                "{}: a file changed outside under an unsaved edit asks whose to keep",
                self.whence()
            );
            let answer = ["Keep mine", "Take theirs"][self.roll(2)];
            let acts = self.session.bench.browser.answer(answer);
            self.session.bench.act(acts);
            self.pump();
            if answer == "Take theirs" {
                self.model.new_step(Step::Edit {
                    label: format!("Revert “{}”", self.name(doc)),
                    doc,
                    before: whole.working,
                    after: theirs.clone(),
                });
                self.model.state[doc].working = theirs;
                return self.frame();
            }
        }
        // Undo or Redo may come in the frame the change was read in, before it ends.
        match self.roll(3) {
            0 => self.undo(),
            1 => self.redo(),
            _ => self.frame(),
        }
    }

    fn refused_save(&mut self, doc: usize) {
        self.unwinds = false;
        self.session.store.fault(a_refused_save());
        self.session.bench.workspace.mark_saved(self.ids[doc]);
        self.pump();
        self.frame();
    }

    fn named(&self, next: Option<Next<'_>>) -> Option<Named> {
        next.map(|next| match next {
            Next::Library { label } => Named::Library(label.to_string()),
            Next::Edit { label, id } => Named::Edit(label.to_string(), self.doc(id)),
        })
    }

    fn undo(&mut self) {
        let shown = self.named(self.session.timeline.next_undo());
        assert_eq!(
            shown,
            self.model.next_undo(),
            "{}: Undo names the step the model takes back",
            self.whence()
        );
        let before = self.actual();
        let Some(at) = self.model.undoes() else {
            self.press(false, None);
            return self.frame();
        };
        let step = self.model.done[at].clone();
        if self.model.dead(&step, false) {
            return self.let_go(false, at);
        }
        let beneath = at + 1 != self.model.done.len();
        if let Step::Edit { doc, .. } = step {
            self.undid = Some(doc);
        }
        let answer = self.press(false, Some(&step));
        self.model.answered(false, at, answer);
        self.frame();
        let claim = match (&step, beneath) {
            (Step::Edit { .. }, true) => "undoing an edit from beneath a save",
            (Step::Edit { .. }, false) => "undoing an edit",
            (Step::Library { save: true, .. }, _) => "undoing a save",
            (Step::Library { save: false, .. }, _) => "undoing a rename",
        };
        self.expect(claim, &before, &step);
        if answer == Answer::Taken && self.roll(2) == 0 {
            let undone = self.actual();
            assert_eq!(
                self.named(self.session.timeline.next_redo()),
                self.model.next_redo(),
                "{}: Redo names the step just undone",
                self.whence()
            );
            let at = self.model.undone.len() - 1;
            let again = self.press(true, Some(&step));
            self.model.answered(true, at, again);
            self.frame();
            self.compare("Redo right after Undo puts back what Undo took", &before);
            let at = self.model.undoes().expect("the step just made again");
            let back = self.press(false, Some(&step));
            self.model.answered(false, at, back);
            self.frame();
            self.compare("Undo again takes the same step back", &undone);
        }
    }

    fn redo(&mut self) {
        let shown = self.named(self.session.timeline.next_redo());
        assert_eq!(
            shown,
            self.model.next_redo(),
            "{}: Redo names the step the model makes again",
            self.whence()
        );
        let before = self.actual();
        let Some(step) = self.model.undone.last().cloned() else {
            self.press(true, None);
            return self.frame();
        };
        let at = self.model.undone.len() - 1;
        if self.model.dead(&step, true) {
            return self.let_go(true, at);
        }
        let answer = self.press(true, Some(&step));
        self.model.answered(true, at, answer);
        self.frame();
        let claim = match &step {
            Step::Edit { .. } => "redoing an edit",
            Step::Library { save: true, .. } => "redoing a save",
            Step::Library { save: false, .. } => "redoing a rename",
        };
        self.expect(claim, &before, &step);
    }

    /// Press Undo, or Redo where `again`, on the edit step at `at`, whose document moved
    /// on: the step goes, and nothing else changes.
    fn let_go(&mut self, again: bool, at: usize) {
        let before = self.actual();
        self.press(again, None);
        self.model.answered(again, at, Answer::Forgotten);
        self.frame();
        self.compare(
            "a press on an edit whose document moved on changes nothing",
            &before,
        );
    }

    /// The state after `step` was undone or redone from `before`: a library step changes
    /// no working copy, and an edit step changes no file, and neither changes a document
    /// it does not name.
    fn expect(&self, claim: &str, before: &State, step: &Step) {
        let after = self.actual();
        let touched: Vec<usize> = match step {
            Step::Edit { doc, .. } => vec![*doc],
            Step::Library { files, .. } => files.iter().map(|filed| filed.doc).collect(),
        };
        let edit = matches!(step, Step::Edit { .. });
        for doc in 0..DOCUMENTS {
            let (was, now) = (&before[doc], &after[doc]);
            let whence = self.whence();
            assert!(
                edit || was.working == now.working,
                "{whence}: {claim} changed the working copy of document {doc}"
            );
            assert!(
                !edit || (was.file == now.file && was.path == now.path),
                "{whence}: {claim} changed the file of document {doc}"
            );
            assert!(
                touched.contains(&doc) || was == now,
                "{whence}: {claim} of documents {touched:?} changed document {doc}"
            );
        }
        self.compare(claim, &self.model.state);
    }

    /// Press Undo, or Redo where `again`, as the Edit menu does, and wait for all it
    /// asks. The timeline takes `step`, and the answer says what became of it. The frame
    /// goes on.
    fn press(&mut self, again: bool, step: Option<&Step>) -> Answer {
        let whence = self.whence();
        assert!(
            self.session.store.undoes(),
            "{whence}: Undo may ask the library"
        );
        let Bench { workspace, log, .. } = &mut self.session.bench;
        let mut editors = Editors {
            document: &mut self.document,
            workspace,
            log,
        };
        let went = match again {
            false => self.session.timeline.undo(&mut editors),
            true => self.session.timeline.redo(&mut editors),
        };
        let went_to = match &went {
            Went::Nowhere => None,
            Went::Edit(id) => Some(Named::Edit(String::new(), self.doc(*id))),
            Went::Library(taken) => Some(Named::Library(taken.label().to_string())),
        };
        let model_to = step.map(|step| match Model::name(step) {
            Named::Edit(_, doc) => Named::Edit(String::new(), doc),
            named => named,
        });
        assert_eq!(
            went_to, model_to,
            "{whence}: the timeline takes the model's step"
        );
        if let Went::Library(taken) = went {
            let refused = match again {
                false => self.session.store.undo(taken).err(),
                true => self.session.store.redo(taken).err(),
            };
            assert!(refused.is_none(), "{whence}: the library was asked");
        }
        self.pump();
        let Some(Step::Library { label, .. }) = step else {
            return Answer::Taken;
        };
        let verb = if again { "made again" } else { "undone" };
        let answer = match self.session.said(&format!("{label} cannot be {verb}: ")) {
            0 => Answer::Taken,
            _ => Answer::Forgotten,
        };
        let moved_on = step.is_some_and(|step| self.model.moved_on(step));
        assert!(
            moved_on || answer == Answer::Taken,
            "{whence}: the library takes {label}, whose files changed nowhere else"
        );
        self.refusal = answer != Answer::Taken;
        answer
    }

    fn compare(&self, claim: &str, expected: &State) {
        let actual = self.actual();
        let whence = self.whence();
        for doc in 0..DOCUMENTS {
            let (want, got) = (&expected[doc], &actual[doc]);
            assert!(
                want.path == got.path,
                "{whence}: {claim}: document {doc} is at {:?}, not {:?}",
                got.path,
                want.path
            );
            assert!(
                want.file == got.file,
                "{whence}: {claim}: the file of document {doc} holds {}, not {}",
                self.gain(&got.file),
                self.gain(&want.file)
            );
            assert!(
                want.working == got.working,
                "{whence}: {claim}: document {doc} holds {}, not {}",
                self.gain(&got.working),
                self.gain(&want.working)
            );
        }
    }

    /// The gain a body was made with, to name it in a failure.
    fn gain(&self, body: &[u8]) -> String {
        match self.bodies.iter().position(|known| known == body) {
            Some(at) => format!("gain {}", GAINS[at]),
            None => "a body the walk never made".to_string(),
        }
    }

    /// What holds after every action.
    fn check(&self, claim: &str) {
        self.compare(claim, &self.model.state);
        let whence = self.whence();
        for doc in 0..DOCUMENTS {
            let whole = &self.model.state[doc];
            assert_eq!(
                self.session.unsaved(self.ids[doc]),
                whole.working != whole.file,
                "{whence}: document {doc} reads unsaved exactly when it holds what its file does not"
            );
        }
        let store = &self.session.store;
        assert!(
            !store.changing() && !store.deciding(),
            "{whence}: the library has answered"
        );
        let asking = self.session.bench.browser.asking();
        assert_eq!(asking, None, "{whence}: no question waits");
    }

    /// Whether the action explains a warning or an error the log says.
    fn explained(&self, text: &str) -> bool {
        match self.action {
            Action::RefusedSave(_) => {
                text.contains("was not saved, because") || text.ends_with(": refused by the test")
            }
            Action::Outside(_) => text.contains("changed on disk") || self.refusal,
            Action::Undo | Action::Redo => self.refusal,
            _ => false,
        }
    }

    /// Give every file the one time a coarse clock gives files written close together,
    /// and let the library look.
    fn coarsen(&mut self) {
        for whole in &self.model.state {
            touch(&self.root, &whole.path);
        }
        self.session.refocus();
        self.pump();
    }

    fn act(&mut self, action: Action) {
        self.action = action;
        self.refusal = false;
        self.undid = None;
        self.session.bench.log.clear();
        match action {
            Action::Edit(doc) => self.edit(doc),
            Action::Save(doc) => self.save(&[doc]),
            Action::SaveAll => self.save(&[0, 1, 2]),
            Action::Rename(doc) => self.rename(doc),
            Action::Revert(doc) => self.revert(doc),
            Action::Outside(doc) => self.outside(doc),
            Action::RefusedSave(doc) => self.refused_save(doc),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
        }
        if self.coarse {
            self.coarsen();
        }
        self.check("after the action");
        let loud =
            self.session.bench.log.iter().filter(|entry| {
                entry.level != crate::log::Level::Info && !self.explained(&entry.text)
            });
        let loud: Vec<&str> = loud.map(|entry| entry.text.as_str()).collect();
        assert!(loud.is_empty(), "{}: the log says {loud:?}", self.whence());
    }

    /// Half the time, the document of the edit Undo names, so that a change outside
    /// lands under the step Undo would take.
    fn under_undo(&mut self) -> Option<usize> {
        let at = self.model.undoes()?;
        let Step::Edit { doc, .. } = self.model.done[at] else {
            return None;
        };
        (self.roll(2) == 0).then_some(doc)
    }

    fn pick(&mut self, outside: bool) -> Action {
        // A save right after an edit is undone leaves that edit to be redone.
        if let Some(doc) = self.undid.take().filter(|_| self.roll(2) == 0) {
            return [Action::Save(doc), Action::SaveAll][self.roll(2)];
        }
        let doc = self.roll(DOCUMENTS);
        let unsaved = {
            let whole = &self.model.state[doc];
            whole.working != whole.file
        };
        match self.roll(24) {
            0..=5 => Action::Edit(doc),
            6 | 7 => Action::Save(doc),
            8 => Action::SaveAll,
            9 => Action::Rename(doc),
            10 => Action::Revert(doc),
            11..=13 if outside => Action::Outside(self.under_undo().unwrap_or(doc)),
            14 if outside && unsaved => Action::RefusedSave(doc),
            11..=19 => Action::Undo,
            _ => Action::Redo,
        }
    }

    fn walk(seed: u64, steps: usize, outside: bool) {
        let mut walk = Walk::start(seed);
        for at in 1..=steps {
            walk.at = at;
            let action = walk.pick(outside);
            walk.act(action);
        }
        if !walk.unwinds {
            return;
        }
        assert_eq!(walk.model.let_go, 0, "seed {seed}: no step was let go");
        walk.at = steps + 1;
        while walk.model.next_undo().is_some() {
            walk.act(Action::Undo);
        }
        assert_eq!(
            walk.named(walk.session.timeline.next_undo()),
            None,
            "seed {seed}: nothing is left to undo"
        );
        walk.compare("undoing every step returns to the start", &walk.start);
    }
}

#[test]
fn any_walk_of_edits_saves_and_renames_undoes_and_redoes_as_the_model_and_unwinds_to_the_start() {
    for seed in SEEDS {
        Walk::walk(seed, STEPS, false);
    }
}

#[test]
fn any_walk_with_changes_outside_and_refused_saves_undoes_only_what_still_matches() {
    for seed in SEEDS {
        Walk::walk(seed, STEPS, true);
    }
}
