//! Open a library folder headless through drawbar's own native backend, make one change
//! or say what it sees, and close it as the app closes it.
//!
//! ```text
//! cargo run -p drawbar --example sync_probe --features sync-probe -- \
//!     --library DIR --local DIR STEP [+ STEP]...
//! ```
//!
//! `--local` is this install's own state for the library, which the app keeps under its
//! data folder. Steps joined by `+` run in one session, so `undo` reaches the steps
//! before it:
//!
//! ```text
//! show
//! save PATH (--bytes-from FILE | --text TEXT)
//! tag PATH NAME
//! untag PATH NAME
//! rename PATH NEW-NAME
//! move PATH DIR          (DIR "" is the library's root)
//! delete PATH
//! mkdir PATH
//! undo
//! settle UNFINISHED-ID (finish | rollback | leave)
//! take-branch KEPT-ID
//! dismiss-branch KEPT-ID
//! ```
//!
//! `show` prints JSON with sorted keys: `state` is what synced machines must agree on,
//! and `diagnostics` the rest. The ids `settle`, `take-branch` and `dismiss-branch` take
//! are the ones `show` prints. What the library answers goes to stderr, one line each.
//! The exit status is 1 where a step was refused or failed, and 2 for a usage error.

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (library, local, steps) = match probe::parse(&args) {
        Ok(parsed) => parsed,
        Err(why) => {
            eprintln!("usage: {why}");
            return std::process::ExitCode::from(2);
        }
    };
    match probe::run(library, local, steps) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("sync_probe: {why}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod probe {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use drawbar::store::shown::Shown;
    use drawbar::store::{
        identity_of_bytes, Backend, Cmd, Event, Intent, KeptApart, Key, Leftover, LibPath, Moving,
        Unwritable, Viewed,
    };
    use serde_json::{json, Value};
    use toshokan::log::Settlement;
    use toshokan::Expect;

    pub enum Step {
        Show,
        Save { path: LibPath, bytes: Vec<u8> },
        Tag { path: LibPath, name: String },
        Untag { path: LibPath, name: String },
        Rename { path: LibPath, to: LibPath },
        Delete { path: LibPath },
        Mkdir { path: LibPath },
        Undo,
        Settle { id: String, how: Settlement },
        Take { id: String },
        Dismiss { id: String },
    }

    pub fn parse(args: &[String]) -> Result<(PathBuf, PathBuf, Vec<Step>), String> {
        let (mut library, mut local, mut rest) = (None, None, args);
        loop {
            match rest {
                [flag, dir, more @ ..] if flag == "--library" => {
                    (library, rest) = (Some(dir), more)
                }
                [flag, dir, more @ ..] if flag == "--local" => (local, rest) = (Some(dir), more),
                _ => break,
            }
        }
        let library = library.ok_or("--library DIR is required")?;
        let local = local.ok_or("--local DIR is required")?;
        let steps = rest
            .split(|word| word == "+")
            .map(step)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((PathBuf::from(library), PathBuf::from(local), steps))
    }

    fn step(words: &[String]) -> Result<Step, String> {
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        Ok(match words[..] {
            ["show"] => Step::Show,
            ["save", path, "--bytes-from", file] => Step::Save {
                path: lib_path(path)?,
                bytes: std::fs::read(file).map_err(|e| format!("{file}: {e}"))?,
            },
            ["save", path, "--text", text] => Step::Save {
                path: lib_path(path)?,
                bytes: text.as_bytes().to_vec(),
            },
            ["tag", path, name] => Step::Tag {
                path: lib_path(path)?,
                name: name.to_string(),
            },
            ["untag", path, name] => Step::Untag {
                path: lib_path(path)?,
                name: name.to_string(),
            },
            ["rename", path, name] if !name.contains('/') => {
                let path = lib_path(path)?;
                let to = path.parent().join(lib_path(name)?.as_str());
                Step::Rename { path, to }
            }
            ["move", path, dir] => {
                let path = lib_path(path)?;
                let dir = LibPath::parse(dir).ok_or(format!("{dir:?} is no library folder"))?;
                let to = dir.join(path.leaf());
                Step::Rename { path, to }
            }
            ["delete", path] => Step::Delete {
                path: lib_path(path)?,
            },
            ["mkdir", path] => Step::Mkdir {
                path: lib_path(path)?,
            },
            ["undo"] => Step::Undo,
            ["settle", id, how] => Step::Settle {
                id: id.to_string(),
                how: match how {
                    "finish" => Settlement::Finished,
                    "rollback" => Settlement::RolledBack,
                    "leave" => Settlement::Dismissed,
                    _ => return Err(format!("settle takes finish, rollback or leave, not {how}")),
                },
            },
            ["take-branch", id] => Step::Take { id: id.to_string() },
            ["dismiss-branch", id] => Step::Dismiss { id: id.to_string() },
            _ => return Err(format!("unknown step {words:?}")),
        })
    }

    fn lib_path(text: &str) -> Result<LibPath, String> {
        LibPath::parse(text)
            .filter(|path| !path.is_root())
            .ok_or_else(|| format!("{text:?} is no path inside a library"))
    }

    pub fn run(library: PathBuf, local: PathBuf, steps: Vec<Step>) -> Result<(), String> {
        if !library.is_dir() {
            return Err(format!("{} is not a folder", library.display()));
        }
        let mut session = Session::open(library, local)?;
        let ran = steps.into_iter().try_for_each(|step| session.run(step));
        session.close();
        ran
    }

    /// The library as this probe has heard of it: what the app would mirror.
    struct Session {
        backend: Backend,
        library: PathBuf,
        local: PathBuf,
        shown: Shown,
        kept: Vec<KeptApart>,
        unfinished: Vec<Leftover>,
        finishing: Vec<LibPath>,
        retired: bool,
        writable: Value,
        opened: Value,
        /// Why the step running now failed, as the first answer that said so.
        failed: Option<String>,
    }

    impl Session {
        /// Open the library and wait for the scan that lists every file, as the app's
        /// first scan does.
        fn open(library: PathBuf, local: PathBuf) -> Result<Session, String> {
            let ctx = eframe::egui::Context::default();
            let mut backend = Backend::start(&ctx, library.clone(), local.clone());
            backend.send(Cmd::Open);
            let opened = match backend.recv() {
                Some(Event::Opened(opened)) => opened,
                Some(other) => return Err(format!("the library answered {other:?} to opening")),
                None => return Err("the library did not answer opening".to_string()),
            };
            let opened = opened.map_err(|why| format!("opening the library: {why}"))?;
            for why in &opened.trouble {
                say("error", format!("opening the library: {why}"));
            }
            let writable = match &opened.writable {
                Ok(()) => json!("writable"),
                Err(Unwritable::ReadOnly(why)) => json!({ "read_only": why }),
                Err(Unwritable::Unfinished(why)) => json!({ "unfinished": why }),
            };
            if let Some(why) = writable.as_object().and_then(|why| why.values().next()) {
                say("trouble", format!("the library opened read-only: {why}"));
            }
            let mut session = Session {
                backend,
                library,
                local,
                shown: Shown::default(),
                kept: Vec::new(),
                unfinished: Vec::new(),
                finishing: Vec::new(),
                retired: false,
                writable,
                opened: json!({
                    "drafts": opened.drafts.len(),
                    "ignored": opened.ignored,
                    "opening_copy": opened.opening_copy,
                }),
                failed: None,
            };
            session.viewed(opened.viewed);
            session.scanned()?;
            for kept in &session.kept {
                let changes = kept.changes.len();
                say(
                    "say",
                    format!("kept apart: {changes} changes ({})", kept_id(kept)),
                );
            }
            for leftover in &session.unfinished {
                let orphan = &leftover.orphan;
                let id = orphan_id(leftover);
                say("say", format!("unfinished: “{}” ({id})", orphan.label));
            }
            Ok(session)
        }

        /// Rescan, and fold in every answer up to the scan's, which runs after every
        /// command sent before it.
        fn scanned(&mut self) -> Result<(), String> {
            self.failed = None;
            self.backend.send(Cmd::Rescan);
            loop {
                let event = match self.backend.recv() {
                    Some(event) => event,
                    None if self.backend.waiting().is_some() => continue,
                    None => return Err("the library stopped answering".to_string()),
                };
                if let Event::Scanned(scanned) = event {
                    let viewed = scanned.map_err(|why| format!("reading the library: {why}"))?;
                    self.viewed(viewed);
                    return self.failed.take().map_or(Ok(()), Err);
                }
                self.fold(event);
            }
        }

        fn fold(&mut self, event: Event) {
            let failed = match event {
                Event::Committed { label, result, .. } => match result {
                    Ok(viewed) => {
                        say("info", format!("{label}: done"));
                        self.viewed(viewed);
                        None
                    }
                    Err(why) => Some(format!("{label}: {why}")),
                },
                Event::Saved { path, result, .. } => {
                    result.err().map(|why| format!("saving {path}: {why}"))
                }
                Event::Moved { from, to, result } => result
                    .err()
                    .map(|why| format!("moving {from} to {to}: {why}")),
                Event::Failed(why) => Some(why),
                Event::Unheard { label } => Some(format!("the answer to “{label}” was lost")),
                Event::ReadOnly(why) => {
                    say("trouble", format!("the library is read-only now: {why}"));
                    None
                }
                Event::Restarted => {
                    say(
                        "say",
                        "drawbar began a new record of this library".to_string(),
                    );
                    None
                }
                Event::Trimmed(entries) => {
                    let trimmed = entries.len();
                    say("say", format!("undo no longer reaches {trimmed} changes"));
                    None
                }
                Event::Unemptied(why) => {
                    say("error", format!("emptying the trash: {why}"));
                    None
                }
                other => {
                    say("warn", format!("unexpected answer: {other:?}"));
                    None
                }
            };
            if let Some(why) = failed {
                say("error", why.clone());
                self.failed.get_or_insert(why);
            }
        }

        fn viewed(&mut self, viewed: Viewed) {
            self.shown.apply(viewed.shown);
            self.kept = viewed.kept;
            if let Some(unfinished) = viewed.unfinished {
                self.unfinished = unfinished;
            }
            self.finishing = viewed.finishing;
            self.retired |= viewed.retired;
        }

        fn run(&mut self, step: Step) -> Result<(), String> {
            let cmd = match step {
                Step::Show => {
                    let mut shown = self.show();
                    shown.sort_all_objects();
                    let text = serde_json::to_string_pretty(&shown).map_err(|e| e.to_string())?;
                    println!("{text}");
                    return Ok(());
                }
                Step::Save { path, bytes } => Cmd::Intent(Intent::Save {
                    id: 0,
                    expect: self.expect(&path)?,
                    path,
                    bytes,
                    came: None,
                }),
                Step::Tag { path, name } => {
                    let key = self.key(&path)?;
                    if key.entity().is_some_and(|e| self.wears(e, &name)) {
                        say("info", format!("{path} already wears “{name}”"));
                        return Ok(());
                    }
                    let make = !self.shown.names.contains(&name);
                    Cmd::Intent(Intent::Tag {
                        files: vec![key],
                        name,
                        make,
                    })
                }
                Step::Untag { path, name } => {
                    let entity = self.key(&path)?.entity();
                    let Some(entity) = entity.filter(|e| self.wears(*e, &name)) else {
                        return Err(format!("{path} does not wear “{name}”"));
                    };
                    Cmd::Intent(Intent::Untag {
                        entities: vec![entity],
                        name,
                    })
                }
                Step::Rename { path, to } if self.on_disk(&path).is_dir() => {
                    Cmd::Intent(Intent::MoveFolder { from: path, to })
                }
                Step::Rename { path, to } => Cmd::Intent(Intent::Move {
                    moves: vec![Moving {
                        expect: self.expect(&path)?,
                        from: path,
                        to,
                    }],
                }),
                Step::Delete { path } if self.on_disk(&path).is_dir() => {
                    Cmd::Intent(Intent::RemoveDir { path })
                }
                Step::Delete { path } => Cmd::Intent(Intent::Delete {
                    files: vec![(path.clone(), self.expect(&path)?)],
                }),
                Step::Mkdir { path } => Cmd::Intent(Intent::MakeDir { path }),
                Step::Undo => Cmd::Undo,
                Step::Settle { id, how } => {
                    let leftover = self.unfinished.iter().find(|left| orphan_id(left) == id);
                    let leftover = leftover.ok_or(format!("no unfinished change {id}"))?;
                    let orphan = leftover.orphan.clone();
                    Cmd::Settle { orphan, how }
                }
                Step::Take { id } => Cmd::Intent(Intent::Take {
                    run: self.run_of(&id)?,
                    changes: Vec::new(),
                }),
                Step::Dismiss { id } => Cmd::Intent(Intent::Dismiss {
                    run: self.run_of(&id)?,
                }),
            };
            self.backend.send(cmd);
            self.scanned()
        }

        /// What a change to the file at `path` expects there: the contents on disk now,
        /// as the app expects the contents it last read, or nothing.
        fn expect(&self, path: &LibPath) -> Result<Expect, String> {
            let bytes = match std::fs::read(self.on_disk(path)) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Expect::Absent),
                Err(e) => return Err(format!("{path}: {e}")),
            };
            let identity = identity_of_bytes(path, &bytes);
            identity
                .map(Expect::Holds)
                .ok_or_else(|| format!("{path} is no library path"))
        }

        fn on_disk(&self, path: &LibPath) -> PathBuf {
            path.components()
                .fold(self.library.clone(), |at, name| at.join(name))
        }

        /// How the library knows the file listed at `path`.
        fn key(&self, path: &LibPath) -> Result<Key, String> {
            self.listed()
                .remove(path)
                .ok_or_else(|| format!("{path} is not listed"))
        }

        fn listed(&self) -> BTreeMap<LibPath, Key> {
            let files = self.shown.files.iter().map(|(key, (path, _))| (key, path));
            let all = files.chain(&self.shown.others);
            all.map(|(key, path)| (path.clone(), key.clone())).collect()
        }

        fn wears(&self, entity: toshokan::EntityId, name: &str) -> bool {
            self.shown
                .worn
                .get(&entity)
                .is_some_and(|worn| worn.contains(name))
        }

        fn run_of(&self, id: &str) -> Result<toshokan::log::Run, String> {
            let kept = self.kept.iter().find(|kept| kept_id(kept) == id);
            kept.map(|kept| kept.run)
                .ok_or(format!("nothing kept apart is {id}"))
        }

        fn show(&self) -> Value {
            let mut files = serde_json::Map::new();
            let mut entities = serde_json::Map::new();
            for (path, key) in self.listed() {
                let mut file = match std::fs::read(self.on_disk(&path)) {
                    Ok(bytes) => json!({
                        "len": bytes.len(),
                        "blake3": blake3::hash(&bytes).to_hex().to_string(),
                    }),
                    Err(e) => json!({ "unread": e.to_string() }),
                };
                if let Some(entity) = key.entity() {
                    let worn = self.shown.worn.get(&entity).cloned().unwrap_or_default();
                    file["tags"] = json!(worn);
                    if let Some(came) = self.shown.came.get(&entity) {
                        file["origin"] = json!(came);
                    }
                    entities.insert(path.to_string(), json!(entity.to_string()));
                }
                files.insert(path.to_string(), file);
            }
            let lost = self.shown.lost.iter().map(|(entity, path)| {
                let worn = self.shown.worn.get(entity).cloned().unwrap_or_default();
                (
                    path.to_string(),
                    json!({ "entity": entity.to_string(), "tags": worn }),
                )
            });
            let kept: Vec<Value> = self.kept.iter().map(kept_json).collect();
            let unfinished: Vec<Value> = self.unfinished.iter().map(unfinished_json).collect();
            json!({
                "state": {
                    "files": files,
                    "tags": self.shown.names,
                    "lost": lost.collect::<serde_json::Map<_, _>>(),
                },
                "diagnostics": {
                    "entities": entities,
                    "kept_apart": kept,
                    "unfinished": unfinished,
                    "finishing": self.finishing,
                    "writable": self.writable,
                    "opened": self.opened,
                    "retired": self.retired,
                    "taken_in": self.shown.taken,
                    "writers": writers(&self.library, &self.local),
                },
            })
        }

        /// Let the library go once every command sent has run, as the app does at a quit.
        fn close(mut self) {
            self.backend.finish();
            while let Some(event) = self.backend.try_recv() {
                self.fold(event);
            }
        }
    }

    /// The id `take-branch` and `dismiss-branch` name a run kept apart by.
    fn kept_id(kept: &KeptApart) -> String {
        let run = serde_json::to_vec(&kept.run).expect("a run serializes");
        blake3::hash(&run).to_hex()[..12].to_string()
    }

    fn orphan_id(leftover: &Leftover) -> String {
        let orphan = &leftover.orphan;
        format!("{}.{}", orphan.writer, orphan.record)
    }

    fn kept_json(kept: &KeptApart) -> Value {
        let mut value = serde_json::to_value(kept).expect("kept apart serializes");
        value["id"] = json!(kept_id(kept));
        value
    }

    fn unfinished_json(leftover: &Leftover) -> Value {
        let orphan = &leftover.orphan;
        json!({
            "id": orphan_id(leftover),
            "label": orphan.label,
            "paths": orphan.paths.iter().map(|path| path.as_str()).collect::<Vec<_>>(),
            "ours": orphan.ours,
            "live": orphan.live,
            "unfinishable": leftover.unfinishable,
        })
    }

    /// Every writer the folder holds a log of, and each of this install's pool: whether
    /// it is this install's, and whether this install retired it.
    fn writers(library: &Path, local: &Path) -> Value {
        let logs = library.join(".drawbar/library/writers.2");
        let mut writers: BTreeMap<String, Value> = names(&logs)
            .into_iter()
            .map(|writer| (writer, json!({ "this_install": false })))
            .collect();
        for genesis in names(local) {
            let head = std::fs::read(local.join(&genesis).join("head.json"));
            let Some(head) = head
                .ok()
                .and_then(|head| serde_json::from_slice::<Value>(&head).ok())
            else {
                continue;
            };
            let Some(writer) = head["writer"].as_str() else {
                continue;
            };
            let retired = head["retired"].as_bool().unwrap_or(false);
            let pooled = json!({ "this_install": true, "retired": retired });
            writers.insert(writer.to_string(), pooled);
        }
        json!(writers)
    }

    fn names(dir: &Path) -> Vec<String> {
        let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
        entries
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect()
    }

    /// One line of what drawbar said, at `level`.
    fn say(level: &str, text: String) {
        eprintln!("drawbar {level}: {text}");
    }
}
