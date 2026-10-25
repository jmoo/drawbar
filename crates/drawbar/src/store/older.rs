//! What a drawbar from before the library was toshokan's kept in a library's folder:
//! tags, unsaved edits and the slot contents it rescued. Nothing reads or removes them;
//! the app tells the user once per library where they are ([`notice`]).

use std::io;

use serde::{Deserialize, Serialize};
use toshokan::asynch::Fs;
use toshokan::io::{Kind, Root};
use toshokan::RelPath;

use super::keys::DIR;
use super::storage;

/// One kind of thing an older drawbar kept, in the folder of drawbar's own data in the
/// library.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Older {
    Tags,
    Edits,
    Rescues,
}

impl Older {
    /// What it is, and where it is under the folder of drawbar's own data.
    pub fn said(self) -> (&'static str, &'static str) {
        match self {
            Older::Tags => ("tags", "library.ron"),
            Older::Edits => ("unsaved edits", "working"),
            Older::Rescues => ("rescued slot contents", "tmp"),
        }
    }
}

fn at(path: &str) -> RelPath {
    RelPath::new(path).expect("a fixed path")
}

/// What an older drawbar kept in the folder.
pub async fn found<F: Fs>(fs: &F) -> io::Result<Vec<Older>> {
    let kind = async |path: &str| -> io::Result<Option<Kind>> {
        Ok(
            storage::meta(fs, Root::Folder, &at(&format!("{DIR}/{path}")))
                .await?
                .map(|meta| meta.kind),
        )
    };
    let mut found = Vec::new();
    if kind("library.ron").await? == Some(Kind::File) {
        found.push(Older::Tags);
    }
    if kind("working").await? == Some(Kind::Directory) {
        found.push(Older::Edits);
    }
    if kind("tmp").await? == Some(Kind::Directory) {
        let tmp = storage::list(fs, Root::Folder, &at(&format!("{DIR}/tmp"))).await?;
        if tmp.iter().any(|(name, _)| name.starts_with("rescued-")) {
            found.push(Older::Rescues);
        }
    }
    Ok(found)
}

/// What the user is told of what an older drawbar kept in the folder.
pub fn notice(older: &[Older]) -> String {
    let said: Vec<String> = older
        .iter()
        .map(|older| {
            let (what, at) = older.said();
            format!("{what} in {DIR}/{at}")
        })
        .collect();
    format!(
        "An older drawbar kept {} in this folder. This drawbar does not read them, and \
         leaves them where they are.",
        crate::strings::listed(&said)
    )
}
