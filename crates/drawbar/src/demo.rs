//! The demo sounds: small instruments published beside the browser build, fetched when
//! asked for and filed in a folder of their own on this computer.
//!
//! Nothing here runs by itself. Asking again brings back whatever of them is no longer
//! on this computer, and adds nothing that still is.

use crate::folders::Folders;
use crate::log::Log;
use crate::store::LibPath;
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
        let bytes = crate::net::get(&format!("{PUBLISHED}{published}"))
            .await
            .map_err(|why| format!("Could not fetch the demo sounds: {published}: {why}."))?;
        files.push((kept.to_string(), bytes));
    }
    Ok(files)
}

/// The assets that might already hold one of `files` but are not read far enough to
/// tell. Filing waits until there are none.
pub fn unsure(files: &[(String, Vec<u8>)], workspace: &Workspace) -> Vec<u64> {
    let unsure = workspace
        .listed()
        .filter(|entity| files.iter().any(|(_, bytes)| entity.might_hold(bytes)));
    unsure.map(|entity| entity.id).collect()
}

/// File the fetched `files` as new files in [`FOLDER`], at the top of the library,
/// leaving out any whose exact bytes are already on this computer, wherever they are
/// filed. A file in the folder under a demo's name is never replaced: the demo takes the
/// next free name beside it. Returns how many were added.
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
    let made = folders.named_or_made(&LibPath::root(), FOLDER, workspace);
    let Some(dir) = made.and_then(|id| folders.path_of(id).cloned()) else {
        log.trouble(format!(
            "A file is called “{FOLDER}”, so the demo sounds have no folder to go in."
        ));
        return 0;
    };
    let added = missing.len();
    for (name, bytes) in missing {
        let name = folders.free(&dir, &name, workspace);
        let id = workspace.ingest(name.clone(), Origin::File(name.clone()), bytes, log);
        workspace.place(id, dir.join(&name));
    }
    added
}

/// The demo files as published: the site copies the fixtures directory whole.
#[cfg(test)]
pub(crate) fn published() -> Vec<(String, Vec<u8>)> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Bench;

    fn filing(bench: &mut Bench) -> usize {
        let Bench {
            workspace,
            browser,
            log,
            ..
        } = bench;
        file(published(), workspace, &mut browser.folders, log)
    }

    /// The path of every asset, sorted.
    fn paths(bench: &Bench) -> Vec<String> {
        let listed = bench.workspace.listed();
        let mut paths: Vec<String> = listed
            .map(|entity| entity.path.as_ref().expect("placed").as_str().to_string())
            .collect();
        paths.sort();
        paths
    }

    fn in_folder(names: &[&str]) -> Vec<String> {
        let mut paths: Vec<String> = names
            .iter()
            .map(|name| format!("{FOLDER}/{name}"))
            .collect();
        paths.sort();
        paths
    }

    fn kept() -> Vec<&'static str> {
        FILES.iter().map(|(_, kept)| *kept).collect()
    }

    #[test]
    fn every_published_demo_reads_and_fits_what_a_fetch_takes() {
        for (name, bytes) in published() {
            assert!(
                bytes.len() <= crate::net::LIMIT,
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
        let mut bench = Bench::new();

        assert_eq!(filing(&mut bench), FILES.len());
        assert_eq!(filing(&mut bench), 0);

        assert_eq!(paths(&bench), in_folder(&kept()));
        let folders = bench.browser.folders.all();
        let names: Vec<&str> = folders.iter().map(|folder| folder.path.as_str()).collect();
        assert_eq!(names, [FOLDER], "one demo folder, reused");
    }

    #[test]
    fn a_demo_removed_from_this_computer_comes_back_and_the_rest_stay_single() {
        let mut bench = Bench::new();
        filing(&mut bench);

        let gone = bench
            .workspace
            .listed()
            .find(|e| e.name == FILES[0].1)
            .expect("filed")
            .id;
        bench.workspace.remove(gone, &mut bench.log);

        assert_eq!(filing(&mut bench), 1);
        assert_eq!(paths(&bench), in_folder(&kept()));
    }

    #[test]
    fn a_demo_already_on_this_computer_elsewhere_is_not_added_again() {
        let mut bench = Bench::new();
        let (name, bytes) = published().remove(0);
        let id = bench
            .workspace
            .ingest(name.clone(), Origin::Fresh, bytes, &mut bench.log);
        bench
            .workspace
            .place(id, LibPath::root().join("moved.npno"));

        assert_eq!(filing(&mut bench), FILES.len() - 1);
        let mut expected = in_folder(&kept()[1..]);
        expected.push("moved.npno".to_string());
        expected.sort();
        assert_eq!(paths(&bench), expected);
    }

    #[test]
    fn a_file_under_a_demos_name_with_other_bytes_is_kept_and_the_demo_lands_beside_it() {
        let mut bench = Bench::new();
        filing(&mut bench);
        let tine = bench
            .workspace
            .listed()
            .find(|e| e.name == FILES[0].1)
            .expect("filed")
            .id;
        let mut edited = published().remove(0).1;
        edited.push(0);
        bench
            .workspace
            .replace_bytes(tine, edited.clone(), &mut bench.log);
        bench.workspace.mark_saved(tine);

        assert_eq!(filing(&mut bench), 1);
        let held = bench.workspace.get(tine).expect("kept");
        assert_eq!(
            *held.bytes,
            edited[..],
            "the file under the name is not replaced"
        );
        let mut expected = in_folder(&kept());
        expected.push(format!("{FOLDER}/drawbar-tine 2.npno"));
        expected.sort();
        assert_eq!(paths(&bench), expected);
    }

    #[test]
    fn an_unread_asset_as_long_as_a_demo_is_read_before_the_demos_are_filed() {
        let mut bench = Bench::new();
        let len = published()[1].1.len() as u64;
        let id = bench.workspace.next_id();
        bench.workspace.restore(
            vec![crate::workspace::Saved {
                id,
                name: "pad.nsmp".to_string(),
                path: Some(LibPath::root().join("pad.nsmp")),
                origin: Origin::File("pad.nsmp".to_string()),
                saved: Vec::new(),
                file: None,
                unread: Some(len),
                unsaved: None,
            }],
            None,
            &mut bench.log,
        );

        assert_eq!(unsure(&published(), &bench.workspace), [id]);
    }
}
