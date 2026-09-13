//! The opening notice the browser build shows once per version: what this software still
//! is, and what changed in the version now running.
//!
//! The notes are GitHub's release body for `drawbar-v<version>`, which
//! `scripts/release.bash` writes in a fixed shape. [`classify`] reads that shape back so
//! the modal can paint it without a markdown parser, and anything it does not recognise
//! stays the plain line it was.
//!
//! The shape a modal wears — its width, its spacing, its title line, its links — is here
//! rather than in the browser-only half, because [`crate::about`] wears the same one on
//! every target.

use eframe::egui;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::Splash;

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How wide a modal is, and the room between two of its lines.
pub(crate) const WIDTH: f32 = 560.0;
pub(crate) const GAP: f32 = 4.0;

/// The line a modal opens with: the app, and the version of it running.
pub(crate) fn title(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(format!("drawbar {VERSION}"))
            .font(egui::FontId::new(18.0, crate::app::bold())),
    );
}

/// ⚠️ Always a new tab: in a browser the app *is* the page, and following a link in
/// place ends the session and everything unsaved in it.
pub(crate) fn link(ui: &mut egui::Ui, label: &str, url: &str) {
    ui.add(egui::Hyperlink::from_label_and_url(label, url).open_in_new_tab(true));
}

/// One line of the notes, in the terms the modal paints.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Line<'a> {
    Blank,
    /// `### Features`
    Heading(&'a str),
    /// `- **scope:** description ([sha](url))`, either half of which may be absent.
    Item {
        scope: Option<&'a str>,
        text: &'a str,
        commit: Option<Commit<'a>>,
    },
    /// `**Full changelog**: <url>`
    Changelog(&'a str),
    Text(&'a str),
}

/// The commit an item is credited to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Commit<'a> {
    pub sha: &'a str,
    pub url: &'a str,
}

/// A release body with the emoji variation selector taken out.
///
/// ⚠️ No bundled font has a glyph for U+FE0F, and a missing one renders as an empty box
/// — so `### ⚠️ Breaking changes` would read as a warning sign beside a blank tile.
pub fn plain(body: &str) -> String {
    body.replace('\u{fe0f}', "")
}

/// Read one line of a release body.
pub fn classify(line: &str) -> Line<'_> {
    let line = line.trim_end();
    if line.is_empty() {
        return Line::Blank;
    }
    if let Some(title) = line.strip_prefix("### ") {
        return Line::Heading(title);
    }
    if let Some(url) = line.strip_prefix("**Full changelog**: ") {
        if let Some(url) = https(url) {
            return Line::Changelog(url);
        }
    }
    let Some(item) = line.strip_prefix("- ") else {
        return Line::Text(line);
    };
    let (item, commit) = split_commit(item);
    let (scope, text) = split_scope(item);
    Line::Item {
        scope,
        text,
        commit,
    }
}

/// A URL is offered as a link only when it is `https`.
///
/// The body is fetched text, and every URL `scripts/release.bash` writes is an https one.
fn https(url: &str) -> Option<&str> {
    match url
        .strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty())
    {
        true => Some(url),
        false => None,
    }
}

/// The trailing `([sha](url))`, and the item with it taken off.
fn split_commit(item: &str) -> (&str, Option<Commit<'_>>) {
    const OPEN: &str = " ([";

    let Some(at) = item.rfind(OPEN) else {
        return (item, None);
    };
    let Some(inner) = item[at + OPEN.len()..].strip_suffix("))") else {
        return (item, None);
    };
    let Some((sha, url)) = inner.split_once("](") else {
        return (item, None);
    };
    if sha.is_empty() {
        return (item, None);
    }
    let Some(url) = https(url) else {
        return (item, None);
    };
    (&item[..at], Some(Commit { sha, url }))
}

/// The `**scope:**` an item may open with, and the description after it.
fn split_scope(item: &str) -> (Option<&str>, &str) {
    let Some(rest) = item.strip_prefix("**") else {
        return (None, item);
    };
    let Some((scope, text)) = rest.split_once(":** ") else {
        return (None, item);
    };
    // A description of its own can hold `:** `; a scope never holds a star.
    match scope.contains('*') {
        true => (None, item),
        false => (Some(scope), text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The head of the `drawbar-v0.5.0` release body, as `gh release view` returns it.
    const RELEASED: &str = "\
### ⚠️ Breaking changes
- **nord-cli:** ship the v2 sample encoder unflagged and gate v3/v4 as unverified (#83) ([163abc4](https://github.com/jmoo/drawbar/commit/163abc4e00b0b088fca9a1212fd26faf4cb11770))

### Features
- name every write drawbar sends, and read settings from nord-cli (#71) ([3029a35](https://github.com/jmoo/drawbar/commit/3029a35ff1b0f812ccb21cb1a86fbaf0ae5e0256))

**Full changelog**: https://github.com/jmoo/drawbar/compare/drawbar-v0.4.0...drawbar-v0.5.0";

    #[test]
    fn a_heading_keeps_its_title_alone() {
        assert_eq!(classify("### Bug fixes"), Line::Heading("Bug fixes"));
    }

    #[test]
    fn an_item_splits_its_scope_its_text_and_its_commit() {
        let line = "- **drawbar:** say what changed ([abc1234](https://example.com/c/abc1234))";
        assert_eq!(
            classify(line),
            Line::Item {
                scope: Some("drawbar"),
                text: "say what changed",
                commit: Some(Commit {
                    sha: "abc1234",
                    url: "https://example.com/c/abc1234",
                }),
            }
        );
    }

    #[test]
    fn an_unscoped_item_keeps_its_whole_description() {
        assert_eq!(
            classify("- say what changed"),
            Line::Item {
                scope: None,
                text: "say what changed",
                commit: None,
            }
        );
    }

    #[test]
    fn a_commit_link_that_is_not_https_stays_in_the_text() {
        let line = "- fixed it ([abc1234](javascript:alert(1)))";
        assert_eq!(
            classify(line),
            Line::Item {
                scope: None,
                text: "fixed it ([abc1234](javascript:alert(1)))",
                commit: None,
            }
        );
    }

    #[test]
    fn a_changelog_line_that_is_not_https_is_not_a_link() {
        assert_eq!(
            classify("**Full changelog**: ftp://example.com/log"),
            Line::Text("**Full changelog**: ftp://example.com/log")
        );
    }

    #[test]
    fn a_line_the_shape_does_not_cover_stays_as_it_reads() {
        assert_eq!(
            classify("  Ordinary prose."),
            Line::Text("  Ordinary prose.")
        );
        assert_eq!(classify("   "), Line::Blank);
    }

    #[test]
    fn a_heading_keeps_its_warning_sign_without_the_variation_selector() {
        assert_eq!(
            plain("### \u{26a0}\u{fe0f} Breaking changes"),
            "### \u{26a0} Breaking changes"
        );
    }

    #[test]
    fn every_line_of_a_published_release_body_is_recognised() {
        let read: Vec<Line<'_>> = RELEASED.lines().map(classify).collect();
        assert_eq!(
            read,
            vec![
                Line::Heading("⚠️ Breaking changes"),
                Line::Item {
                    scope: Some("nord-cli"),
                    text: "ship the v2 sample encoder unflagged and gate v3/v4 as unverified (#83)",
                    commit: Some(Commit {
                        sha: "163abc4",
                        url: "https://github.com/jmoo/drawbar/commit/163abc4e00b0b088fca9a1212fd26faf4cb11770",
                    }),
                },
                Line::Blank,
                Line::Heading("Features"),
                Line::Item {
                    scope: None,
                    text: "name every write drawbar sends, and read settings from nord-cli (#71)",
                    commit: Some(Commit {
                        sha: "3029a35",
                        url: "https://github.com/jmoo/drawbar/commit/3029a35ff1b0f812ccb21cb1a86fbaf0ae5e0256",
                    }),
                },
                Line::Blank,
                Line::Changelog(
                    "https://github.com/jmoo/drawbar/compare/drawbar-v0.4.0...drawbar-v0.5.0"
                ),
            ]
        );
    }
}
