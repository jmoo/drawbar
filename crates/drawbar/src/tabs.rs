//! The centre: one tab per open document.
//!
//! A tab is a view of an asset on this computer. Opening something off the instrument
//! copies it here first, so what a tab holds is always a working copy — editing it
//! changes nothing on the instrument until it is sent back.

use eframe::egui;

use crate::app::dot;
use crate::workspace::Workspace;

/// ⚠️ The strip's own scroll id. The strip and the document body are drawn into the same
/// `Ui`, and egui salts an unsalted `ScrollArea` with that `Ui` alone — two of them there
/// share one state, and a wheel over the body moves the strip instead of the document.
pub const SCROLL: &str = "tab_strip";

struct Tab {
    id: u64,
    /// The bytes as the tab opened them: what Revert goes back to, and what the byte
    /// diff is measured against. Held per tab, so switching tabs does not lose it.
    opened: Vec<u8>,
}

#[derive(Default)]
pub struct Tabs {
    open: Vec<Tab>,
    active: Option<u64>,
}

impl Tabs {
    pub fn open(&mut self, id: u64, workspace: &Workspace) {
        if !self.open.iter().any(|tab| tab.id == id) {
            let opened = workspace
                .get(id)
                .map(|e| e.bytes.clone())
                .unwrap_or_default();
            self.open.push(Tab { id, opened });
        }
        self.active = Some(id);
    }

    pub fn close(&mut self, id: u64) {
        self.open.retain(|tab| tab.id != id);
        if self.active == Some(id) {
            self.active = self.open.last().map(|tab| tab.id);
        }
    }

    pub fn active(&self) -> Option<u64> {
        self.active
    }

    /// Whether a tab is open on this document, in front or behind.
    pub fn holds(&self, id: u64) -> bool {
        self.open.iter().any(|tab| tab.id == id)
    }

    /// What the tab looked like when it opened.
    pub fn opened(&self, id: u64) -> &[u8] {
        self.open
            .iter()
            .find(|tab| tab.id == id)
            .map_or(&[], |tab| tab.opened.as_slice())
    }

    /// Drop tabs whose asset is no longer on this computer.
    pub fn prune(&mut self, workspace: &Workspace) {
        self.open
            .retain(|tab| workspace.entities().iter().any(|e| e.id == tab.id));
        if self
            .active
            .is_some_and(|id| !self.open.iter().any(|tab| tab.id == id))
        {
            self.active = self.open.last().map(|tab| tab.id);
        }
    }

    /// The strip. The open document draws itself below it.
    pub fn ui(&mut self, ui: &mut egui::Ui, workspace: &Workspace) {
        let mut close = None;
        let mut activate = None;
        egui::ScrollArea::horizontal()
            .id_salt(SCROLL)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for tab in &self.open {
                        let Some(entity) = workspace.get(tab.id) else {
                            continue;
                        };
                        // A view of the instrument's copy reads differently from a tab
                        // that holds something of this computer's.
                        let name = match workspace.is_view(tab.id) {
                            true => egui::RichText::new(&entity.name).italics(),
                            false => egui::RichText::new(&entity.name),
                        };
                        let mut label = ui.selectable_label(self.active == Some(tab.id), name);
                        if workspace.is_view(tab.id) {
                            label = label.on_hover_text("the instrument's copy, viewed in place");
                        }
                        if label.clicked() {
                            activate = Some(tab.id);
                        }
                        if entity.pending {
                            let owed = entity
                                .origin
                                .slot()
                                .map(|(class, at)| {
                                    format!("will be sent to {}", crate::strings::place(class, at))
                                })
                                .unwrap_or_default();
                            dot(ui, crate::app::warn(ui.visuals())).on_hover_text(owed);
                        } else if entity.dirty {
                            dot(ui, crate::app::good(ui.visuals()))
                                .on_hover_text("changed since it was opened");
                        }
                        if ui.small_button("×").clicked() {
                            close = Some(tab.id);
                        }
                        ui.separator();
                    }
                });
            });
        if let Some(id) = activate {
            self.active = Some(id);
        }
        if let Some(id) = close {
            self.close(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Workspace {
        Workspace::new(egui::Context::default())
    }

    /// Opening the same asset twice is the same tab, brought forward.
    #[test]
    fn opening_an_asset_that_is_already_open_just_activates_it() {
        let (mut tabs, ws) = (Tabs::default(), workspace());
        tabs.open(1, &ws);
        tabs.open(2, &ws);
        tabs.open(1, &ws);
        assert_eq!(tabs.open.len(), 2);
        assert_eq!(tabs.active(), Some(1));
    }

    /// Closing what is in front falls back to another tab rather than to nothing, and
    /// closing the last one leaves nothing showing.
    #[test]
    fn closing_the_active_tab_falls_back_to_another() {
        let (mut tabs, ws) = (Tabs::default(), workspace());
        tabs.open(1, &ws);
        tabs.open(2, &ws);
        tabs.close(2);
        assert_eq!(tabs.active(), Some(1));
        tabs.close(1);
        assert_eq!(tabs.active(), None);
    }

    /// Closing a tab that is not in front leaves the front one showing.
    #[test]
    fn closing_a_background_tab_leaves_the_front_one_showing() {
        let (mut tabs, ws) = (Tabs::default(), workspace());
        tabs.open(1, &ws);
        tabs.open(2, &ws);
        tabs.close(1);
        assert_eq!(tabs.active(), Some(2));
    }

    /// Whether a tab is open is its own question, not one inferred from the bytes it
    /// opened with — a document that opened empty is still open.
    #[test]
    fn a_tab_says_whether_it_is_open_whatever_it_holds() {
        let (mut tabs, ws) = (Tabs::default(), workspace());
        assert!(!tabs.holds(1));
        // Nothing in the workspace under this id, so the tab opens with no bytes at all.
        tabs.open(1, &ws);
        tabs.open(2, &ws);
        assert!(tabs.opened(1).is_empty());
        assert!(tabs.holds(1) && tabs.holds(2), "both are open");
        assert!(!tabs.holds(3));

        // Behind the front one still counts.
        tabs.close(2);
        assert!(tabs.holds(1) && !tabs.holds(2));
        tabs.close(1);
        assert!(!tabs.holds(1));
    }

    /// Each tab keeps the bytes it opened with, so Revert in one is not Revert in
    /// another.
    #[test]
    fn each_tab_keeps_the_bytes_it_opened_with() {
        let (mut tabs, mut ws) = (Tabs::default(), workspace());
        let mut log = crate::log::Log::default();
        let first = ws
            .create(crate::workspace::Fresh::Program, &mut log)
            .unwrap();
        let second = ws
            .create(crate::workspace::Fresh::Settings, &mut log)
            .unwrap();
        tabs.open(first, &ws);
        tabs.open(second, &ws);

        assert_eq!(tabs.opened(first), ws.get(first).unwrap().bytes.as_slice());
        assert_ne!(tabs.opened(first), tabs.opened(second));
        // A tab that was never opened has nothing to go back to.
        assert!(tabs.opened(999).is_empty());
    }
}
