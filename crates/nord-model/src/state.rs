//! One program's state, as the rules see it.

use nord_format::fields::Field;

/// A snapshot of `(path, value)` for one entity body.
///
/// Built from the registry's own `fields()`, so a field enters the state by being
/// declared. A hand-built partial state is legal and answers fewer conditions rather
/// than wrong ones — see [`Cond`](crate::Cond).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    entries: Vec<Entry>,
    body: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct Entry {
    path: String,
    value: String,
    /// Asked of the field's type when wanted, not when the state is built: walking every
    /// bit pattern a seven-bit field takes is not something a per-frame caller should pay
    /// for to ask whether a control is live.
    legal: Option<fn() -> Vec<String>>,
}

/// Two entries are the same when they hold the same value. What the field's type accepts
/// is a property of the type, not of the state, and comparing the function pointers that
/// answer it means nothing.
impl PartialEq for Entry {
    fn eq(&self, other: &Entry) -> bool {
        self.path == other.path && self.value == other.value
    }
}

impl Eq for Entry {}

impl State {
    pub fn new() -> State {
        State::default()
    }

    /// Every registered field's current value, with the values its type accepts.
    pub fn from_fields(fields: &[Field]) -> State {
        State {
            entries: fields
                .iter()
                .map(|field| Entry {
                    path: field.path.clone(),
                    value: field.value.clone(),
                    legal: Some(field.spec.legal),
                })
                .collect(),
            body: None,
        }
    }

    /// One path and value, for a state built by hand. The field's legal values are
    /// unknown to a state built this way, so it offers no choices.
    pub fn with(mut self, path: impl Into<String>, value: impl Into<String>) -> State {
        self.set(path, value);
        self
    }

    /// The entity body the state was read from, which is what
    /// [`check`](crate::DeviceModel::check) needs to reach bits no field claims.
    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> State {
        self.body = Some(body.into());
        self
    }

    pub fn set(&mut self, path: impl Into<String>, value: impl Into<String>) {
        let (path, value) = (path.into(), value.into());
        match self.entries.iter_mut().find(|entry| entry.path == path) {
            Some(entry) => entry.value = value,
            None => self.entries.push(Entry {
                path,
                value,
                legal: None,
            }),
        }
    }

    pub fn value(&self, path: &str) -> Option<&str> {
        self.entry(path).map(|entry| entry.value.as_str())
    }

    /// Every value the field's type accepts. Empty for a field too wide to enumerate,
    /// and for one the state was told about by hand.
    pub fn legal(&self, path: &str) -> Vec<String> {
        self.entry(path)
            .and_then(|entry| entry.legal)
            .map(|legal| legal())
            .unwrap_or_default()
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.path.as_str())
    }

    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }

    fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.path == path)
    }
}
