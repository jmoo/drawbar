//! `nord edit` — change fields inside any editable file, dispatched on what
//! the file is rather than on an object class.
//!
//! The noun commands edit what the Electro 5 stores; this is the file verb
//! beside `inspect` and `verify`, so the formats with no noun — the Stage
//! programs and presets, the Sample Editor project — are editable too.
//! Everything `nord-format` can set is settable here: the generated registry
//! where the body declares one, and the accessor-backed editors otherwise.

use std::path::PathBuf;

use clap::Args;

use crate::edit::{editor_for, print_byte_diff, write_edit, SetArgs};
use crate::editors;
use crate::ui::Ui;

#[derive(Args)]
pub struct FileEditArgs {
    /// The file to edit: any format with settable fields — a program, synth
    /// or organ/piano preset whose body decodes, a set list, a sample
    /// instrument, or a Sample Editor project.
    pub file: PathBuf,

    #[command(flatten)]
    pub common: SetArgs,
}

pub fn run(ui: &Ui, args: FileEditArgs) -> Result<(), String> {
    let path = &args.file;
    let original = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut entity = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .map_err(|e| format!("{}: {e}", path.display()))?;

    // The editor's mutable borrow ends here, before `to_bytes` reads the whole entity.
    let staged = editors::stage(
        ui,
        args.common.fields,
        &args.common.set,
        editor_for(&mut entity)?.as_mut(),
    )?;
    let Some(changed) = staged else {
        return Ok(());
    };
    if changed == 0 {
        ui.note("no field changed; writing nothing");
        return Ok(());
    }

    let edited = nord_format::to_bytes(&entity).map_err(|e| e.to_string())?;
    print_byte_diff(ui, &original, &edited);

    if args.common.dry_run {
        ui.note("--dry-run: nothing written");
        return Ok(());
    }

    write_edit(ui, path, args.common.out, args.common.yes, &edited)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nord_format::cbin::{Cbin, Header};
    use nord_format::formats::{ne5, ns3};

    /// The file verb writes over the same file the noun edits do, so `-o` naming its
    /// input meets the same guard.
    #[test]
    fn an_output_that_is_the_input_takes_the_in_place_guard() {
        let dir = crate::edit::tests::scratch("file-edit-in-place");
        let path = dir.join("p.ne5p");
        let original = nord_format::to_bytes(&nord_format::Entity::Program(
            nord_format::Program::Electro5(ne5::program::new((0, 0).try_into().unwrap())),
        ))
        .unwrap();
        std::fs::write(&path, &original).unwrap();

        let args = FileEditArgs {
            file: path.clone(),
            common: SetArgs {
                set: vec!["center_panel.gain=64".into()],
                dry_run: false,
                fields: false,
                out: Some(dir.join(".").join("p.ne5p")),
                yes: false,
            },
        };
        let err = run(&Ui::piped(), args).unwrap_err();
        assert!(err.contains("--yes"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    /// A zeroed Stage 3 program: every field's type decodes the whole of its
    /// slot, so a body of zeros is legal — the same construction drawbar's New
    /// menu uses.
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
