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
use nord_format::Entity;

use crate::edit::{print_byte_diff, write_file};
use crate::editors::{self, ProjectEditor, SampleEditor, SongEditor};
use crate::ui::Ui;
use crate::EditArgs;

#[derive(Args)]
pub struct FileEditArgs {
    /// The file to edit: any format with settable fields — a program, synth
    /// or organ/piano preset whose body decodes, a set list, a sample
    /// instrument, or a Sample Editor project.
    pub file: PathBuf,

    /// `path=value`, repeatable. Paths are what `--fields` lists.
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub set: Vec<String>,

    /// Report what would change — including which bytes — and write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// List every settable field with its current value, then exit.
    #[arg(long)]
    pub fields: bool,

    /// Write the edit here instead of over the input file.
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Confirm the write. Editing the file in place needs it.
    #[arg(long)]
    pub yes: bool,
}

/// Whether the file verb has anything to set on this entity.
pub fn editable(entity: &Entity) -> bool {
    entity.registry().is_some()
        || matches!(
            entity,
            Entity::Song(nord_format::Song::Electro5(_))
                | Entity::Sample(nord_format::Sample::V2(_))
                | Entity::SampleProject(_)
        )
}

pub fn run(ui: &Ui, args: FileEditArgs) -> Result<(), String> {
    let path = &args.file;
    let original = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut entity = nord_format::from_stream(&mut std::io::Cursor::new(&original))
        .map_err(|e| format!("{}: {e}", path.display()))?;

    let noun_args = EditArgs {
        target: None,
        set: args.set.clone(),
        dry_run: args.dry_run,
        fields: args.fields,
        out: None,
        yes: args.yes,
    };
    // End the mutable editor borrow before `to_bytes` reads the whole entity.
    let staged = if entity.registry().is_some() {
        crate::edit::stage(ui, &noun_args, entity.registry_mut().unwrap())?
    } else {
        match &mut entity {
            Entity::Song(nord_format::Song::Electro5(song)) => {
                editors::stage(ui, args.fields, &args.set, &mut SongEditor(song))?
            }
            Entity::Sample(nord_format::Sample::V2(sample)) => {
                editors::stage(ui, args.fields, &args.set, &mut SampleEditor(sample))?
            }
            Entity::Sample(nord_format::Sample::V3(_)) => {
                return Err(
                    "this instrument is nsmp3/nsmp4 content; only v2 (.nsmp) can be edited".into(),
                )
            }
            Entity::SampleProject(project) => {
                editors::stage(ui, args.fields, &args.set, &mut ProjectEditor(project))?
            }
            _ => {
                let id = entity.identity();
                return Err(format!(
                    "nothing in a {} ({}) is settable yet; `nord inspect` still reads it",
                    id.kind, id.format,
                ));
            }
        }
    };
    // `--fields` has listed them and is done.
    let Some(changed) = staged else {
        return Ok(());
    };
    if changed == 0 {
        ui.note("no field changed; writing nothing");
        return Ok(());
    }

    let edited = nord_format::to_bytes(&entity).map_err(|e| e.to_string())?;
    print_byte_diff(ui, &original, &edited);

    if args.dry_run {
        ui.note("--dry-run: nothing written");
        return Ok(());
    }

    match args.out {
        Some(out) => write_file(ui, &out, &edited),
        None => {
            ui.note(format!(
                "about to {} {} in place",
                ui.danger("overwrite"),
                path.display()
            ));
            ui.confirm(args.yes)?;
            write_file(ui, path, &edited)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nord_format::cbin::{Cbin, Header};
    use nord_format::formats::{ne5, ns3};

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

    /// Every editable shape answers, and the stubs say no — the dispatch the
    /// file verb and the steers both rest on.
    #[test]
    fn editable_knows_every_shape() {
        let stage3 = nord_format::from_stream(&mut std::io::Cursor::new(stage3_program())).unwrap();
        assert!(editable(&stage3));

        let song = Entity::Song(nord_format::Song::Electro5(ne5::song::new(
            (0, 0).try_into().unwrap(),
            ne5::song::DEFAULT_VERSION,
            [(0, 0).try_into().unwrap(); 4],
        )));
        assert!(editable(&song));

        let project = Entity::SampleProject(
            nord_format::formats::nsmpproj::Project::new(
                "X",
                &[nord_format::formats::nsmpproj::NewZone {
                    path: "x.wav".into(),
                    sample_rate: 44100,
                    frames: 44100,
                    root_key: 60,
                }],
                0,
            )
            .unwrap(),
        );
        assert!(editable(&project));

        let stub = Entity::PipeLibrary(Cbin {
            header: Header::new("npip", (0xffff, 0xffff), 1),
            body: nord_format::cbin::RawBody(vec![0; 4]),
        });
        assert!(!editable(&stub));
    }
}
