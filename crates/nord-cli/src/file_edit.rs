//! `nord edit`: change fields in any editable file, dispatched on the file's
//! format.
//!
//! The noun commands edit what the instrument stores. This file verb sits beside
//! `inspect` and `verify`, so formats with no noun (the Stage programs and
//! presets, the Sample Editor project) are editable too. It reaches everything
//! `nord-format` can set: the generated registry where the body declares one,
//! and the accessor-backed editors otherwise.

use std::path::PathBuf;

use clap::Args;

use crate::edit::{staged_bytes, write_edit, SetArgs};
use crate::ui::Ui;

#[derive(Args)]
pub struct FileEditArgs {
    /// The file to edit, in any format with settable fields: a program, live
    /// slot, settings, or synth, organ or piano preset whose body decodes, a set
    /// list, a sample instrument, or a Sample Editor project.
    pub file: PathBuf,

    #[command(flatten)]
    pub common: SetArgs,
}

pub fn run(ui: &Ui, args: FileEditArgs) -> Result<(), String> {
    let path = &args.file;
    let original = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut entity = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .map_err(|e| format!("{}: {e}", path.display()))?;

    let Some(edited) = staged_bytes(ui, &mut entity, &original, &args.common)? else {
        return Ok(());
    };
    write_edit(ui, path, args.common.out, args.common.yes, &edited)
}

#[cfg(test)]
pub(crate) mod tests {
    use nord_format::cbin::{Cbin, Header};
    use nord_format::formats::ns3;

    /// A zeroed Stage 3 program. Every field's type accepts any value of its
    /// bits, so an all-zero body is valid; drawbar's New menu builds one the same
    /// way.
    pub fn stage3_program() -> Vec<u8> {
        let body =
            ns3::program::Program::try_from([0u8; ns3::program::BODY_LEN]).expect("legal body");
        let file = Cbin {
            header: Header::new(
                ns3::program::FORMAT,
                (0, 0),
                ns3::program::KNOWN_VERSIONS[0],
            ),
            body,
        };
        let mut out = std::io::Cursor::new(Vec::new());
        file.write_to(&mut out).unwrap();
        out.into_inner()
    }
}
