//! A library folder as one writer sees it: open it, change it by intents, undo them.

use crate::blobs::{self, Collection};
use crate::compact::{self, Compaction};
use crate::effects::{self, Effect, Precondition, Report, Source};
use crate::error::{Error, Result};
use crate::fs::{hash_file, Fs, RelPath};
use crate::ids::{EntityId, IntentId, Version, WriterId};
use crate::journal::{self, Recovered};
use crate::layout::Layout;
use crate::log::{read_log, read_logs, Entry, Kind, LogWriter, Torn, WriterLog};
use crate::merge::{merge, State};
use crate::scan::{scan, Scan};
use crate::undo::{plan_redo, plan_undo, History};
use crate::value::Value;
use crate::{CONTENT_FIELD, PATH_FIELD};

/// A library folder opened by one writer.
///
/// Every change is an intent: a group of entries appended to this writer's log under
/// one intent id, with at most the file effects the intent names. An intent is
/// refused before anything is written when the writer is read-only, the entity does
/// not allow it, or a file is not as expected.
///
/// The state is every writer's log as read at open, or at the last
/// [`Library::refresh`], plus this writer's own intents since.
pub struct Library<F: Fs> {
    fs: F,
    layout: Layout,
    log: LogWriter,
    own: WriterLog,
    state: State,
    scan: Scan,
    recovered: Vec<Recovered>,
    torn: Vec<TornSegment>,
}

/// What an intent did.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Change {
    pub intent: IntentId,
    /// The entries appended under the intent, its `Intent` entry first.
    pub entries: Vec<Entry>,
    /// What its file effects did to the files.
    pub files: Report,
}

/// A segment whose readable entries end early.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TornSegment {
    pub writer: WriterId,
    pub segment: u64,
    pub torn: Torn,
}

impl<F: Fs> Library<F> {
    /// Open the library in `fs` as `writer`: read every writer's log, finish or roll
    /// back this writer's interrupted effects, merge, and scan the files.
    ///
    /// With nothing to recover, opening writes nothing. A writer whose own log holds
    /// entries this build does not understand opens read-only and leaves its journal
    /// for a build that does.
    pub async fn open(fs: F, layout: Layout, writer: WriterId) -> Result<Self> {
        let mut logs = read_logs(&fs, &layout).await?;
        let mut log = LogWriter::open(writer, &logs);
        let recovered = match log.read_only() {
            Some(_) => Vec::new(),
            None => journal::recover(&fs, &layout, &mut log).await?,
        };
        if !recovered.is_empty() {
            logs = read_logs(&fs, &layout).await?;
        }
        let mut library = Self {
            fs,
            layout,
            log,
            own: WriterLog::new(writer),
            state: State::default(),
            scan: Scan::default(),
            recovered,
            torn: Vec::new(),
        };
        library.load(logs);
        library.rescan().await?;
        Ok(library)
    }

    pub fn writer(&self) -> WriterId {
        self.log.writer()
    }

    pub fn fs(&self) -> &F {
        &self.fs
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// The latest scan of the files.
    pub fn scan(&self) -> &Scan {
        &self.scan
    }

    /// The effects a crash interrupted, as open settled them.
    pub fn recovered(&self) -> &[Recovered] {
        &self.recovered
    }

    /// The segments whose last lines could not be read, as of the last read.
    pub fn torn(&self) -> &[TornSegment] {
        &self.torn
    }

    /// Why this writer cannot change the library, if it cannot.
    pub fn read_only(&self) -> Option<&str> {
        self.log.read_only()
    }

    /// Read every writer's log again, taking in what other writers appended since.
    pub async fn refresh(&mut self) -> Result<()> {
        let logs = read_logs(&self.fs, &self.layout).await?;
        self.load(logs);
        Ok(())
    }

    /// Scan the files again for the current state.
    pub async fn rescan(&mut self) -> Result<&Scan> {
        self.scan = scan(&self.fs, &self.layout, &self.state, &self.scan).await?;
        Ok(&self.scan)
    }

    fn load(&mut self, logs: Vec<WriterLog>) {
        let writer = self.writer();
        self.state = merge(&logs);
        let seen = logs.iter().map(WriterLog::max_lamport).max().unwrap_or(0);
        self.log.observe(Version::new(seen, writer));
        self.torn = logs
            .iter()
            .flat_map(|log| {
                log.torn.iter().map(|&(segment, torn)| TornSegment {
                    writer: log.writer,
                    segment,
                    torn,
                })
            })
            .collect();
        self.own = logs
            .into_iter()
            .find(|log| log.writer == writer)
            .unwrap_or_else(|| WriterLog::new(writer));
    }

    /// A new entity, existing from this intent on.
    pub async fn create(&mut self) -> Result<(EntityId, Change)> {
        let entity = self.log.new_entity();
        let change = self.record(vec![Kind::Create { entity }]).await?;
        Ok((entity, change))
    }

    /// Set a field, or clear it with `None`. `path` and `content` belong to file
    /// effects and [`Library::bind`].
    pub async fn set(
        &mut self,
        entity: EntityId,
        name: &str,
        value: Option<Value>,
    ) -> Result<Change> {
        self.existing(entity)?;
        if name == PATH_FIELD || name == CONTENT_FIELD {
            return Err(Error::Entity {
                entity,
                reason: "has its path and content set only by file effects and binding",
            });
        }
        let field = self.field(entity, name, value);
        self.record(vec![field]).await
    }

    /// Add `value` to a set.
    pub async fn add(&mut self, entity: EntityId, set: &str, value: Value) -> Result<Change> {
        self.existing(entity)?;
        let name = set.to_owned();
        self.record(vec![Kind::SetAdd {
            entity,
            name,
            value,
        }])
        .await
    }

    /// Remove `value` from a set, as far as this writer has seen it added. An add this
    /// writer has not seen survives.
    pub async fn remove(&mut self, entity: EntityId, set: &str, value: &Value) -> Result<Change> {
        self.existing(entity)?;
        let observed = self.state.tags(entity, set, value);
        if observed.is_empty() {
            return Err(Error::Entity {
                entity,
                reason: "does not hold that value in that set",
            });
        }
        let name = set.to_owned();
        let value = value.clone();
        let remove = Kind::SetRemove {
            entity,
            name,
            value,
            observed,
        };
        self.record(vec![remove]).await
    }

    /// Delete an entity. Its fields are kept, and its file, if any, stays where it is.
    pub async fn delete(&mut self, entity: EntityId) -> Result<Change> {
        self.existing(entity)?;
        self.record(vec![Kind::Delete { entity }]).await
    }

    /// Bind the file at `path` to an entity as it is now, as after a scan finds it.
    pub async fn bind(&mut self, entity: EntityId, path: &RelPath) -> Result<Change> {
        self.existing(entity)?;
        self.layout.check_library_path(path)?;
        let (blob, _) = hash_file(&self.fs, path).await?;
        let fields = vec![
            self.field(
                entity,
                PATH_FIELD,
                Some(Value::Text(path.as_str().to_owned())),
            ),
            self.field(entity, CONTENT_FIELD, Some(Value::Blob(blob))),
        ];
        self.record(fields).await
    }

    /// Write an entity's file at `path`, where the file must meet `expect`. Old
    /// contents move into the blob store.
    pub async fn save(
        &mut self,
        entity: EntityId,
        path: &RelPath,
        bytes: Vec<u8>,
        expect: Precondition,
    ) -> Result<Change> {
        self.existing(entity)?;
        let save = Effect::Save {
            entity,
            path: path.clone(),
            contents: Source::Bytes(bytes),
            expect,
        };
        self.commit(header(), vec![save]).await
    }

    /// Move an entity's file to `to`, where nothing may be.
    pub async fn rename(&mut self, entity: EntityId, to: &RelPath) -> Result<Change> {
        let from = self.bound(entity)?;
        let rename = Effect::Rename {
            entity,
            from,
            to: to.clone(),
        };
        self.commit(header(), vec![rename]).await
    }

    /// Move a directory and everything in it to `to`, where nothing may be, and every
    /// entity bound under it.
    pub async fn move_tree(&mut self, from: &RelPath, to: &RelPath) -> Result<Change> {
        let move_tree = Effect::MoveTree {
            from: from.clone(),
            to: to.clone(),
        };
        self.commit(header(), vec![move_tree]).await
    }

    /// Move an entity's file, which must meet `expect`, into the blob store and unbind
    /// it. The entity still exists.
    pub async fn delete_file(&mut self, entity: EntityId, expect: Precondition) -> Result<Change> {
        let path = self.bound(entity)?;
        let delete = Effect::Delete {
            entity,
            path,
            expect,
        };
        self.commit(header(), vec![delete]).await
    }

    /// Reverse this writer's latest intent that is not undone.
    pub async fn undo(&mut self) -> Result<Change> {
        self.log.writable()?;
        let plan = plan_undo(&History::new(&self.own), &self.state)?;
        self.commit(plan.entries, plan.effects).await
    }

    /// Reverse this writer's latest undo.
    pub async fn redo(&mut self) -> Result<Change> {
        self.log.writable()?;
        let plan = plan_redo(&History::new(&self.own), &self.state)?;
        self.commit(plan.entries, plan.effects).await
    }

    /// Fold this writer's log into a snapshot that keeps its last `window` intents
    /// undoable.
    pub async fn compact(&mut self, window: usize) -> Result<Compaction> {
        self.log.writable()?;
        let own = read_log(&self.fs, &self.layout, self.writer()).await?;
        let compacted = compact::compact(&self.fs, &self.layout, &mut self.log, &own, window).await;
        self.reloaded(compacted).await
    }

    /// Remove this writer's blobs that nothing needs, oldest first, until they occupy at
    /// most `budget` bytes.
    pub async fn collect(&mut self, budget: u64) -> Result<Collection> {
        let collected =
            blobs::collect(&self.fs, &self.layout, &mut self.log, &self.state, budget).await;
        self.reloaded(collected).await
    }

    fn existing(&self, entity: EntityId) -> Result<()> {
        match self.state.exists(entity) {
            true => Ok(()),
            false => Err(Error::Entity {
                entity,
                reason: "does not exist",
            }),
        }
    }

    /// The path of an existing entity's file.
    fn bound(&self, entity: EntityId) -> Result<RelPath> {
        self.existing(entity)?;
        match self.state.field(entity, PATH_FIELD) {
            Some(Value::Text(path)) => RelPath::new(path),
            _ => Err(Error::Entity {
                entity,
                reason: "has no file",
            }),
        }
    }

    fn field(&self, entity: EntityId, name: &str, value: Option<Value>) -> Kind {
        Kind::Field {
            entity,
            name: name.to_owned(),
            value,
            prior: self.state.field(entity, name).cloned(),
        }
    }

    /// Append an intent of `kinds` that changes no file.
    async fn record(&mut self, kinds: Vec<Kind>) -> Result<Change> {
        let mut entries = header();
        entries.extend(kinds);
        self.commit(entries, Vec::new()).await
    }

    /// Append `kinds`, which start with the intent's `Intent` entry, and apply
    /// `effects` in order. The entries are journaled with the first effect, so they
    /// reach the log only once its files are changed.
    async fn commit(&mut self, kinds: Vec<Kind>, effects: Vec<Effect>) -> Result<Change> {
        self.log.writable()?;
        let intent = self.log.new_intent();
        if effects.is_empty() {
            let entries: Vec<Entry> = kinds
                .into_iter()
                .map(|kind| self.log.stamp(intent, kind))
                .collect();
            if let Err(error) = self.log.append(&self.fs, &self.layout, &entries).await {
                return self.reloaded(Err(error)).await;
            }
            for entry in &entries {
                self.state.apply(entry);
            }
            self.own.entries.extend(entries.iter().cloned());
            let files = Report::default();
            return Ok(Change {
                intent,
                entries,
                files,
            });
        }
        let mut kinds = Some(kinds);
        let mut files = Report::default();
        for effect in &effects {
            let entries = kinds.take().unwrap_or_default();
            let applied = effects::apply(
                &self.fs,
                &self.layout,
                &mut self.log,
                &self.state,
                intent,
                entries,
                effect,
            )
            .await;
            let report = self.reloaded(applied).await?;
            files.displaced.extend(report.displaced);
            files.moved.extend(report.moved);
        }
        let entries = self
            .own
            .all_entries()
            .filter(|entry| entry.intent == intent)
            .cloned()
            .collect();
        Ok(Change {
            intent,
            entries,
            files,
        })
    }

    /// `result`, after reading every log again, since the operation that gave it may
    /// have appended to this writer's log, folded it, or failed partway.
    async fn reloaded<T>(&mut self, result: Result<T>) -> Result<T> {
        let refreshed = self.refresh().await;
        let value = result?;
        refreshed?;
        Ok(value)
    }
}

fn header() -> Vec<Kind> {
    vec![Kind::Intent {
        label: None,
        reverses: None,
    }]
}
