//! "This computer", kept across restarts.
//!
//! eframe hands over one string store — localStorage in a browser tab, a ron file on the
//! desktop — so an asset is written as a line: its id, where it came from, what it is
//! called, the bytes it was last saved as in base64, and, where it holds something else,
//! those bytes too. It is read back through the same decode-and-verify any file gets,
//! because bytes off a store deserve no more trust than bytes off a disk.

use crate::base64;
use crate::log::Log;
use crate::queue::Queue;
use crate::workspace::{Origin, Saved, Workspace};
use nord_usb::{Location, ObjectClass};

const KEY: &str = "drawbar.this_computer";
const VERSION: &str = "drawbar 2";

/// How many fields a line holds, which is the whole of what a version decides.
#[derive(Clone, Copy)]
enum Shape {
    /// `drawbar 1`: id, origin, name, bytes. The bytes are both what the asset holds
    /// and what it was last saved as, because that version knew of no other.
    Four,
    /// `drawbar 2`: those, and — where an edit is unsaved — the bytes it holds instead.
    Five,
}

impl Shape {
    /// The shape a version line asks for. A version this build does not know has none,
    /// and its store is left alone rather than half-read.
    fn of(version: &str) -> Option<Shape> {
        match version {
            "drawbar 1" => Some(Shape::Four),
            VERSION => Some(Shape::Five),
            _ => None,
        }
    }
}

/// The largest asset worth keeping.
///
/// ⚠️ A browser gives an origin about 5 MiB for everything it stores, and base64 costs a
/// third on top. A sample runs to megabytes on its own, so one would fill the store and
/// take every program with it.
const MAX_ENTITY: usize = 1024 * 1024;

/// What the whole store may take.
///
/// ⚠️ Writing past the quota is refused by the browser and reported nowhere the app can
/// see — `Storage::set_string` cannot fail as far as its caller knows. So the budget is
/// kept here, below the quota, and what does not fit is said out loud rather than lost
/// quietly.
const BUDGET: usize = 3 * 1024 * 1024;

/// Write the list. Called by eframe periodically and on the way out.
///
/// ⚠️ A view of a slot is written only when it holds changes. An untouched view is the
/// instrument's own copy looked at in place, and persisting it would hand the operator a
/// local copy they never asked to keep; an edited one is the **only** copy there is, and
/// quitting with its tab open must not be how it goes.
///
/// ⚠️ On wasm every call base64-encodes the whole list on the only thread. Callers
/// must rate-limit writes because dragging mutates the list every frame.
///
/// What is written comes back kept — see [`load`].
pub fn save(
    storage: &mut dyn eframe::Storage,
    workspace: &Workspace,
    queue: &Queue,
    log: &mut Log,
) {
    let mut out = format!("{VERSION}\n{}\n", workspace.next_id());
    let mut skipped = 0;
    let mut dropped = 0;
    for entity in workspace.entities() {
        if !entity.kept && !crate::workspace::precious(entity, queue) {
            continue;
        }
        if entity.bytes.len().max(entity.saved.bytes.len()) > MAX_ENTITY {
            skipped += 1;
            continue;
        }
        // The baseline is what the asset is; the tail is what it holds instead, and only
        // an unsaved asset has one.
        let unsaved = match entity.is_unsaved() {
            true => format!("\t{}", base64::encode(&entity.bytes)),
            false => String::new(),
        };
        let line = format!(
            "{}\t{}\t{}\t{}{unsaved}\n",
            entity.id,
            origin(&entity.origin),
            escape(&entity.name),
            base64::encode(&entity.saved.bytes),
        );
        if out.len() + line.len() > BUDGET {
            dropped += 1;
            continue;
        }
        out.push_str(&line);
    }
    storage.set_string(KEY, out);

    match (skipped, dropped) {
        (0, 0) => {}
        (skipped, 0) => log.say(plural(skipped, "too big to keep between sessions")),
        (0, dropped) => log.trouble(plural(dropped, "left out — there is no room to keep them")),
        (skipped, dropped) => log.trouble(plural(
            skipped + dropped,
            "not kept between sessions — too big, or no room left",
        )),
    }
}

fn plural(n: usize, tail: &str) -> String {
    match n {
        1 => format!("1 sound is {tail}."),
        n => format!("{n} sounds are {tail}."),
    }
}

/// Read the list back, decoding and re-checking every asset on the way in.
///
/// Everything restored is on this computer. A view is a document with a tab over it and
/// a slot under it, and at startup there is neither — so an edited view [`save`] kept
/// comes back as the local asset it had already become in all but name.
pub fn load(storage: &dyn eframe::Storage, workspace: &mut Workspace, log: &mut Log) {
    let Some(text) = storage.get_string(KEY) else {
        return;
    };
    let mut lines = text.lines();
    let Some(shape) = lines.next().and_then(Shape::of) else {
        // A store this build cannot read is left alone rather than half-read: the next
        // save replaces it.
        log.warn("the saved list is in a format this build does not read");
        return;
    };
    let next_id = lines.next().and_then(|line| line.parse().ok());
    let mut restored = Vec::new();
    let mut unreadable = 0;
    for line in lines {
        match entry(line, shape) {
            Some(saved) => restored.push(saved),
            None => unreadable += 1,
        }
    }
    let count = restored.len();
    workspace.restore(restored, next_id, log);
    if unreadable > 0 {
        log.warn(format!("{unreadable} saved line(s) did not read"));
    }
    if count > 0 {
        log.say(match count {
            1 => "1 sound is back from last time.".to_string(),
            n => format!("{n} sounds are back from last time."),
        });
    }
}

fn entry(line: &str, shape: Shape) -> Option<Saved> {
    let mut parts = line.splitn(5, '\t');
    let id = parts.next()?.parse().ok()?;
    let origin = unorigin(parts.next()?)?;
    let name = unescape(parts.next()?);
    let saved = base64::decode(parts.next()?)?;
    let unsaved = match (shape, parts.next()) {
        (_, None) => None,
        (Shape::Five, Some(text)) => Some(base64::decode(text)?),
        (Shape::Four, Some(_)) => return None,
    };
    Some(Saved {
        id,
        name,
        origin,
        saved,
        unsaved,
    })
}

fn origin(origin: &Origin) -> String {
    match origin {
        Origin::File(name) => format!("file:{}", escape(name)),
        Origin::Device { class, at } => {
            format!("device:{}:{}:{}", class.to_raw(), at.bank, at.slot)
        }
        Origin::Fresh => "fresh".into(),
        Origin::Rescued { at } => format!("rescued:{}:{}", at.bank, at.slot),
    }
}

fn unorigin(text: &str) -> Option<Origin> {
    let (head, rest) = text.split_once(':').unwrap_or((text, ""));
    Some(match head {
        "file" => Origin::File(unescape(rest)),
        "fresh" => Origin::Fresh,
        "device" => {
            let mut parts = rest.split(':');
            let class = ObjectClass::from_raw(parts.next()?.parse().ok()?);
            Origin::Device {
                class,
                at: location(parts.next()?, parts.next()?)?,
            }
        }
        "rescued" => {
            let mut parts = rest.split(':');
            Origin::Rescued {
                at: location(parts.next()?, parts.next()?)?,
            }
        }
        _ => return None,
    })
}

fn location(bank: &str, slot: &str) -> Option<Location> {
    Some(Location {
        bank: bank.parse().ok()?,
        slot: slot.parse().ok()?,
    })
}

/// Tabs separate the fields and newlines separate the lines, so a name holding either
/// would be a name that ate the rest of the store.
pub(crate) fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

pub(crate) fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some('t')) => {
                chars.next();
                out.push('\t');
            }
            ('\\', Some('n')) => {
                chars.next();
                out.push('\n');
            }
            ('\\', Some('\\')) => {
                chars.next();
                out.push('\\');
            }
            _ => out.push(c),
        }
    }
    out
}

/// A store in a map, for the tests that need one to write into and read back.
#[cfg(test)]
#[derive(Default)]
pub struct Fake(std::collections::HashMap<String, String>);

#[cfg(test)]
impl eframe::Storage for Fake {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }

    fn flush(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every origin survives the round trip, so a restored asset still knows where it
    /// came from and can still be sent back there.
    #[test]
    fn an_origin_round_trips() {
        let at = Location { bank: 6, slot: 3 };
        for before in [
            Origin::Fresh,
            Origin::File("Africa Split.ne5p".into()),
            Origin::Device {
                class: ObjectClass::Program,
                at,
            },
            Origin::Rescued { at },
        ] {
            let text = origin(&before);
            let after = unorigin(&text).expect(&text);
            assert_eq!(origin(&after), text);
        }
    }

    /// A name holding a separator would otherwise swallow the rest of the line.
    #[test]
    fn a_name_holding_a_separator_survives() {
        for name in ["plain", "with\ttab", "with\nnewline", "back\\slash", "\\t"] {
            assert_eq!(unescape(&escape(name)), name);
        }
    }

    fn workspace() -> (Workspace, Log) {
        (
            Workspace::new(eframe::egui::Context::default()),
            Log::default(),
        )
    }

    /// What was on this computer is on it again, with its name, where it came from and
    /// its bytes — and it is re-checked on the way in.
    #[test]
    fn the_list_comes_back_as_it_was() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        let id = before.create(Fresh::Program, &mut log).unwrap();
        before.rename(id, "Africa-Split.ne5p".into());
        before.create(Fresh::Settings, &mut log).unwrap();

        let mut store = Fake::default();
        save(&mut store, &before, &Queue::default(), &mut log);

        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        assert_eq!(after.entities().len(), 2);
        let restored = after.get(id).expect("kept its id");
        assert_eq!(restored.name, "Africa-Split.ne5p");
        assert_eq!(restored.bytes, before.get(id).unwrap().bytes);
        assert!(matches!(restored.verify, crate::workspace::VerifyState::Ok));
        assert_eq!(after.export_name(id).as_deref(), Some("Africa-Split.ne5p"));
    }

    /// ⚠️ A view is the only copy of what it holds, so quitting with an edited one open
    /// must not be how it goes. An untouched view is the instrument's own bytes and is
    /// not written; an edited one is, and comes back as an asset on this computer,
    /// because at startup there is no tab to view it from.
    #[test]
    fn an_edited_view_survives_a_session_and_an_untouched_one_does_not() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        let bytes = {
            let id = before.create(Fresh::Program, &mut log).unwrap();
            let bytes = before.get(id).unwrap().bytes.clone();
            before.remove(id, &mut log);
            bytes
        };
        let at = |slot| Location { bank: 6, slot };
        let view = |workspace: &mut Workspace, slot, log: &mut Log| {
            workspace.view(
                format!("view-{slot}.ne5p"),
                Origin::Device {
                    class: ObjectClass::Program,
                    at: at(slot),
                },
                bytes.clone(),
                log,
            )
        };
        let edited = view(&mut before, 0, &mut log);
        let owed = view(&mut before, 1, &mut log);
        let untouched = view(&mut before, 2, &mut log);
        let held = before.get(edited).unwrap().bytes.clone();
        before.replace_bytes(edited, [held, vec![0]].concat(), &mut log);
        let mut queue = Queue::default();
        crate::queue::enqueue(
            &before,
            &mut crate::device::Device::new(before.ctx().clone()),
            &mut queue,
            &mut log,
            owed,
            ObjectClass::Program,
            at(1),
        );

        let mut store = Fake::default();
        save(&mut store, &before, &queue, &mut log);
        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        assert!(after.get(untouched).is_none(), "the slot still has it");
        let restored: Vec<u64> = after.listed().map(|e| e.id).collect();
        assert_eq!(restored, vec![edited, owed]);
        assert_eq!(after.get(edited).unwrap().name, "view-0.ne5p");
        // Restored is kept: there is no tab at startup, so nothing views anything.
        assert!(!after.is_view(edited) && !after.is_view(owed));
        // And the slot it came off is still recorded, so it can still go back.
        assert_eq!(
            after.get(owed).unwrap().origin.slot(),
            Some((ObjectClass::Program, at(1)))
        );
    }

    /// An asset holding an edit nothing has saved comes back holding it, and still
    /// knowing what it was saved as — so the revert offered in one session is the same
    /// revert in the next.
    #[test]
    fn an_unsaved_edit_and_the_baseline_under_it_both_survive() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        let id = before.create(Fresh::Program, &mut log).unwrap();
        let saved = before.get(id).unwrap().bytes.clone();
        let (_, edited) =
            crate::fields::apply(&saved, &[("center_panel.gain".into(), "96".into())]).unwrap();
        before.replace_bytes(id, edited.clone(), &mut log);

        let mut store = Fake::default();
        save(&mut store, &before, &Queue::default(), &mut log);
        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        let restored = after.get(id).expect("kept its id");
        assert_eq!(restored.bytes, edited, "it holds the edit");
        assert_eq!(restored.saved.bytes, saved, "and what it was saved as");
        assert!(restored.is_unsaved());
        after.revert(id, &mut log);
        assert_eq!(after.get(id).unwrap().bytes, saved);
    }

    /// A saved asset writes one set of bytes, not two.
    #[test]
    fn a_saved_asset_has_no_unsaved_tail() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        before.create(Fresh::Program, &mut log).unwrap();
        let mut store = Fake::default();
        save(&mut store, &before, &Queue::default(), &mut log);
        let text = eframe::Storage::get_string(&store, KEY).expect("something was written");
        let line = text.lines().nth(2).expect("the one asset's line");
        assert_eq!(line.split('\t').count(), 4);
    }

    /// Something too big to keep is left out and said out loud, rather than filling the
    /// store and taking everything else with it.
    #[test]
    fn an_oversized_asset_is_left_out_and_reported() {
        use crate::workspace::Origin;

        let (mut before, mut log) = workspace();
        before.ingest(
            "huge.nsmp".into(),
            Origin::Fresh,
            vec![0; MAX_ENTITY + 1],
            &mut log,
        );
        let mut store = Fake::default();
        save(&mut store, &before, &Queue::default(), &mut log);
        assert!(log.status().1.contains("too big"), "{}", log.status().1);

        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);
        assert!(after.entities().is_empty());
    }

    /// A store written by another build is left alone rather than half-read.
    #[test]
    fn a_store_from_another_build_is_not_guessed_at() {
        let mut store = Fake::default();
        eframe::Storage::set_string(&mut store, KEY, "drawbar 99\n1\n".to_string());
        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);
        assert!(after.entities().is_empty());
    }

    /// A line that is not a line is dropped, not guessed at.
    #[test]
    fn a_damaged_line_is_refused() {
        let read = |line| entry(line, Shape::Five);
        assert!(read("").is_none());
        assert!(read("7\tfresh\tname").is_none(), "no bytes");
        assert!(
            read("7\tfresh\tname\tZm9v\t!!!").is_none(),
            "the tail is not base64"
        );
        assert!(
            read("7\tfresh\tname\tZm9v\tYmFy").is_some(),
            "saved, and a tail"
        );
        assert!(read("seven\tfresh\tname\tZm9v").is_none(), "no id");
        assert!(read("7\tnonesuch\tname\tZm9v").is_none(), "no such origin");
        assert!(read("7\tfresh\tname\t!!!").is_none(), "not base64");
        assert!(read("7\tfresh\tname\tZm9v").is_some());
        // A version-1 line is four fields, and a fifth is a line this is not.
        assert!(entry("7\tfresh\tname\tZm9v", Shape::Four).is_some());
        assert!(entry("7\tfresh\tname\tZm9v\tYmFy", Shape::Four).is_none());
    }

    /// A list the version before this one wrote is read rather than thrown away: its
    /// four fields are an asset holding exactly what it was last saved as.
    #[test]
    fn a_version_1_store_comes_back_kept_and_saved() {
        use crate::workspace::Fresh;

        let (mut before, mut log) = workspace();
        let id = before.create(Fresh::Program, &mut log).unwrap();
        let bytes = before.get(id).unwrap().bytes.clone();

        let mut store = Fake::default();
        eframe::Storage::set_string(
            &mut store,
            KEY,
            format!(
                "drawbar 1\n8\n7\tfile:Africa Split.ne5p\tAfrica Split.ne5p\t{}\n",
                base64::encode(&bytes)
            ),
        );

        let (mut after, mut log) = workspace();
        load(&store, &mut after, &mut log);

        let restored = after.get(7).expect("the line's own id");
        assert_eq!(restored.name, "Africa Split.ne5p");
        assert_eq!(restored.bytes, bytes);
        assert_eq!(restored.saved.bytes, bytes, "there was no other copy");
        assert!(!restored.is_unsaved());
        assert!(restored.kept);
        assert!(matches!(restored.verify, crate::workspace::VerifyState::Ok));
    }
}
