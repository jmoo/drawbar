//! The folders a desktop window opens as its library, and the list of those it opened
//! lately, kept in eframe's store.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;

use super::THIS_COMPUTER;
use crate::folders::Library;

/// The libraries opened lately, most recent first.
#[derive(Debug, Default, PartialEq)]
pub struct Recent(Vec<PathBuf>);

impl Recent {
    const KEY: &'static str = "drawbar.libraries";
    const MOST: usize = 10;

    /// The list as the last session left it. One that does not read is empty.
    pub fn restore(storage: &dyn eframe::Storage) -> Recent {
        let paths: Vec<String> = storage
            .get_string(Recent::KEY)
            .and_then(|text| ron::from_str(&text).ok())
            .unwrap_or_default();
        Recent(
            paths
                .into_iter()
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .take(Recent::MOST)
                .collect(),
        )
    }

    /// ⚠️ eframe's store holds text, so a path that is not Unicode is not remembered.
    pub fn keep(&self, storage: &mut dyn eframe::Storage) {
        let paths: Vec<&str> = self.0.iter().filter_map(|path| path.to_str()).collect();
        if let Ok(text) = ron::to_string(&paths) {
            storage.set_string(Recent::KEY, text);
        }
    }

    /// Put `root` first.
    pub fn opened(&mut self, root: &Path) {
        self.0.retain(|held| held != root);
        self.0.insert(0, root.to_path_buf());
        self.0.truncate(Recent::MOST);
    }

    /// The library to open at start: the one open last, if its folder is still there.
    pub fn last(&self) -> Option<&Path> {
        self.0
            .first()
            .map(PathBuf::as_path)
            .filter(|root| root.is_dir())
    }

    /// The libraries a menu offers, most recent first, with the default library always
    /// among them. `open` is checked.
    pub fn offered(&self, default: Option<&Path>, open: Option<&Path>) -> Vec<Library> {
        let missing = default.filter(|default| !self.0.iter().any(|held| held == default));
        self.0
            .iter()
            .map(PathBuf::as_path)
            .chain(missing)
            .map(|root| Library {
                root: root.to_path_buf(),
                name: name(root, default).unwrap_or_else(|| THIS_COMPUTER.to_string()),
                open: Some(root) == open,
            })
            .collect()
    }
}

/// What the browser calls the library at `root`: its folder's name, or `None` for the
/// default library, which is This computer.
pub fn name(root: &Path, default: Option<&Path>) -> Option<String> {
    if Some(root) == default {
        return None;
    }
    Some(root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    ))
}

/// The folder picker, and the folder it came back with.
pub struct Picker {
    tx: Sender<PathBuf>,
    rx: Receiver<PathBuf>,
}

impl Default for Picker {
    fn default() -> Picker {
        let (tx, rx) = channel();
        Picker { tx, rx }
    }
}

impl Picker {
    pub fn pick(&self, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        crate::workspace::spawn(async move {
            let picked = rfd::AsyncFileDialog::new()
                .set_title("Open a folder as the library")
                .pick_folder()
                .await;
            if let Some(handle) = picked {
                let _ = tx.send(handle.path().to_path_buf());
            }
            ctx.request_repaint();
        });
    }

    /// The folder picked since the last call, if any.
    pub fn picked(&self) -> Option<PathBuf> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str) -> PathBuf {
        std::env::temp_dir().join(path)
    }

    #[test]
    fn the_last_library_opened_comes_first_once_and_the_list_is_kept() {
        let mut recent = Recent::default();
        for name in ["a", "b", "a", "c"] {
            recent.opened(&at(name));
        }
        assert_eq!(recent, Recent(vec![at("c"), at("a"), at("b")]));
        for n in 0..20 {
            recent.opened(&at(&n.to_string()));
        }
        assert_eq!(recent.0.len(), Recent::MOST, "the oldest go");
        assert_eq!(recent.0[0], at("19"));

        let mut storage = crate::testing::Fake::default();
        recent.keep(&mut storage);
        assert_eq!(Recent::restore(&storage), recent);
    }

    #[test]
    fn a_list_that_does_not_read_or_names_a_relative_path_is_left_out() {
        let mut storage = crate::testing::Fake::default();
        eframe::Storage::set_string(&mut storage, Recent::KEY, "not ron".into());
        assert_eq!(Recent::restore(&storage), Recent::default());
        let absolute = at("kept").display().to_string();
        let text = ron::to_string(&vec!["relative/path", absolute.as_str()]).unwrap();
        eframe::Storage::set_string(&mut storage, Recent::KEY, text);
        assert_eq!(Recent::restore(&storage), Recent(vec![at("kept")]));
    }

    #[test]
    fn the_default_library_is_always_offered_and_the_open_one_is_checked() {
        let (default, pack) = (at("Music/drawbar"), at("Cello Pack"));
        let mut recent = Recent::default();
        recent.opened(&pack);
        let offered = recent.offered(Some(&default), Some(&pack));
        let names: Vec<(&str, bool)> = offered
            .iter()
            .map(|library| (library.name.as_str(), library.open))
            .collect();
        assert_eq!(names, [("Cello Pack", true), (THIS_COMPUTER, false)]);

        recent.opened(&default);
        let offered = recent.offered(Some(&default), Some(&default));
        assert_eq!(offered.len(), 2, "listed once");
        assert_eq!(offered[0].name, THIS_COMPUTER);
    }

    #[test]
    fn a_library_is_opened_at_start_only_while_its_folder_is_there() {
        let root = crate::testing::Temp::new();
        let mut recent = Recent::default();
        recent.opened(&root.0);
        assert_eq!(recent.last(), Some(root.0.as_path()));
        recent.opened(&root.at("gone"));
        assert_eq!(recent.last(), None, "the default library opens instead");
    }
}
