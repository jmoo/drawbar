//! The demo sounds: small instruments published beside the browser build, fetched when
//! asked for and filed in a folder of their own on this computer.
//!
//! Nothing here runs by itself. Asking again brings back whatever of them is no longer
//! on this computer, and adds nothing that still is.

use crate::folders::Folders;
use crate::log::Log;
use crate::workspace::{Origin, Workspace};

/// The folder the demo sounds are filed in, made on first use.
pub const FOLDER: &str = "Demo sounds";

/// The demo files: the name each is published under, and the name it is kept under.
///
/// ⚠️ The kept names differ by more than an extension, which drawbar does not show.
pub const FILES: [(&str, &str); 3] = [
    ("drawbar-tine.npno", "drawbar-tine.npno"),
    ("drawbar-pad.nsmp", "drawbar-pad v2.nsmp"),
    ("drawbar-pad.nsmp4", "drawbar-pad v4.nsmp4"),
];

/// Where the demo files are published.
///
/// Relative in a tab, as [`crate::shell::GUIDE`] is, so a preview serves its own; a
/// window reaches for the published ones.
#[cfg(target_arch = "wasm32")]
const PUBLISHED: &str = "demo/";
#[cfg(not(target_arch = "wasm32"))]
const PUBLISHED: &str = "https://drawbar.app/demo/";

/// Every demo file, or why they could not all be fetched.
///
/// All or nothing: a folder holding some of the demo sounds reads as a broken demo.
pub async fn fetch() -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = Vec::with_capacity(FILES.len());
    for (published, kept) in FILES {
        let bytes = get(&format!("{PUBLISHED}{published}"))
            .await
            .map_err(|why| format!("Could not fetch the demo sounds: {published}: {why}."))?;
        files.push((kept.to_string(), bytes));
    }
    Ok(files)
}

/// File the fetched `files` in [`FOLDER`], leaving out any whose exact bytes are already
/// on this computer, wherever they are filed. Returns how many were added.
pub fn file(
    files: Vec<(String, Vec<u8>)>,
    workspace: &mut Workspace,
    folders: &mut Folders,
    log: &mut Log,
) -> usize {
    let missing: Vec<_> = files
        .into_iter()
        .filter(|(_, bytes)| !workspace.holds(bytes))
        .collect();
    if missing.is_empty() {
        log.say("The demo sounds are already on this computer.");
        return 0;
    }
    let Some(folder) = folders.named_or_made(FOLDER) else {
        log.trouble("The folder list is full, so the demo sounds have nowhere to go.");
        return 0;
    };
    let added = missing.len();
    for (name, bytes) in missing {
        let id = workspace.ingest(name.clone(), Origin::File(name), bytes, log);
        folders.file(id, Some(folder));
    }
    added
}

#[cfg(target_arch = "wasm32")]
async fn get(url: &str) -> Result<Vec<u8>, String> {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window().ok_or("no window to fetch from")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|_| "the request failed".to_string())?
        .dyn_into()
        .map_err(|_| "the reply was not a response".to_string())?;
    if !response.ok() {
        return Err(format!("the server answered {}", response.status()));
    }
    let body = response
        .array_buffer()
        .map_err(|_| "the reply has no body".to_string())?;
    let body = JsFuture::from(body)
        .await
        .map_err(|_| "the body did not arrive".to_string())?;
    bounded(js_sys::Uint8Array::new(&body).to_vec())
}

/// ⚠️ Through the system's `curl` rather than an HTTP crate: an HTTPS client would bring
/// a TLS stack and its root store into the desktop build for three small files.
#[cfg(not(target_arch = "wasm32"))]
async fn get(url: &str) -> Result<Vec<u8>, String> {
    let output = std::process::Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--max-time", "60"])
        .args(["--max-filesize", &crate::store::MAX_ENTITY.to_string()])
        .arg(url)
        .output()
        .map_err(|e| format!("could not run curl ({e})"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(said.trim().trim_start_matches("curl: ").to_string());
    }
    bounded(output.stdout)
}

/// `bytes`, unless there are more of them than this computer keeps for one asset.
fn bounded(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    match bytes.len() > crate::store::MAX_ENTITY {
        true => Err(format!("{} bytes is more than a demo holds", bytes.len())),
        false => Ok(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The demo files as published: the site copies the fixtures directory whole.
    fn published() -> Vec<(String, Vec<u8>)> {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../nord-format/tests/fixtures/demo"
        );
        FILES
            .iter()
            .map(|(published, kept)| {
                let path = format!("{dir}/{published}");
                let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
                (kept.to_string(), bytes)
            })
            .collect()
    }

    fn filed(workspace: &Workspace, folders: &Folders) -> Vec<String> {
        let folder = folders
            .all()
            .iter()
            .find(|f| f.name == FOLDER)
            .expect("the demo folder exists")
            .id;
        let mut names: Vec<String> = folders
            .members(folder, workspace)
            .iter()
            .map(|e| e.name.clone())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn every_published_demo_reads_and_fits_what_this_computer_keeps() {
        for (name, bytes) in published() {
            assert!(
                bytes.len() <= crate::store::MAX_ENTITY,
                "{name} is {} bytes",
                bytes.len()
            );
            let mut workspace = Workspace::new(eframe::egui::Context::default());
            let id = workspace.ingest(name.clone(), Origin::Fresh, bytes, &mut Log::default());
            let entity = workspace.get(id).expect("ingested");
            assert!(
                entity.parse_error.is_none(),
                "{name}: {:?}",
                entity.parse_error
            );
        }
    }

    #[test]
    fn asking_twice_files_each_demo_once() {
        let mut workspace = Workspace::new(eframe::egui::Context::default());
        let mut folders = Folders::default();
        let mut log = Log::default();

        assert_eq!(
            file(published(), &mut workspace, &mut folders, &mut log),
            FILES.len()
        );
        assert_eq!(file(published(), &mut workspace, &mut folders, &mut log), 0);

        let mut expected: Vec<String> = FILES.iter().map(|(_, kept)| kept.to_string()).collect();
        expected.sort();
        assert_eq!(filed(&workspace, &folders), expected);
        assert_eq!(workspace.entities().len(), FILES.len());
        assert_eq!(folders.all().len(), 1, "one demo folder, reused");
    }

    #[test]
    fn a_demo_removed_from_this_computer_comes_back_and_the_rest_stay_single() {
        let mut workspace = Workspace::new(eframe::egui::Context::default());
        let mut folders = Folders::default();
        let mut log = Log::default();
        file(published(), &mut workspace, &mut folders, &mut log);

        let gone = workspace
            .entities()
            .iter()
            .find(|e| e.name == FILES[0].1)
            .expect("filed")
            .id;
        workspace.remove(gone, &mut log);
        folders.forget(gone);

        assert_eq!(file(published(), &mut workspace, &mut folders, &mut log), 1);
        assert_eq!(workspace.entities().len(), FILES.len());
    }

    #[test]
    fn a_demo_already_on_this_computer_elsewhere_is_not_added_again() {
        let mut workspace = Workspace::new(eframe::egui::Context::default());
        let mut folders = Folders::default();
        let mut log = Log::default();
        let (name, bytes) = published().remove(0);
        workspace.ingest(name, Origin::Fresh, bytes, &mut log);

        assert_eq!(
            file(published(), &mut workspace, &mut folders, &mut log),
            FILES.len() - 1
        );
    }
}
