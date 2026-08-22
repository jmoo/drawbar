//! The field registry behind any entity whose body declares one, and the
//! mutation check every such entity can answer: each field takes a value it
//! does not hold, reaches the bytes, reads back, and moves nothing else.
//!
//! ⚠️ A rustc-visible support module, not a test target — each test target that
//! includes it compiles its own copy.
#![allow(dead_code)]

use nord_format::fields::{Field, FieldValue};
use nord_format::{Entity, Live, OrganPreset, PianoPreset, Program, Settings, Song, Synth};
use std::io::Cursor;

/// Run `$body` on the `Cbin` behind `$entity` if its body declares a field
/// registry; `None` for the container-verified stubs, which decode nothing.
macro_rules! with_registry {
    ($entity:expr, |$file:ident| $body:expr) => {
        match $entity {
            Entity::Program(Program::Electro5($file)) | Entity::Live(Live::Electro5($file)) => {
                Some($body)
            }
            Entity::Program(Program::Stage2($file)) | Entity::Live(Live::Stage2($file)) => {
                Some($body)
            }
            Entity::Program(Program::Stage3($file)) | Entity::Live(Live::Stage3($file)) => {
                Some($body)
            }
            Entity::Program(Program::Stage4($file)) | Entity::Live(Live::Stage4($file)) => {
                Some($body)
            }
            Entity::Settings(Settings::Electro5($file)) => Some($body),
            Entity::Song(Song::Electro5($file)) => Some($body),
            Entity::Synth(Synth::Stage3($file)) => Some($body),
            Entity::Synth(Synth::Stage4($file)) => Some($body),
            Entity::OrganPreset(OrganPreset::Stage4($file)) => Some($body),
            Entity::PianoPreset(PianoPreset::Stage4($file)) => Some($body),
            _ => None,
        }
    };
}

/// The settable-field view, carrying both of a value's canonical spellings.
pub fn fields(entity: &Entity) -> Option<Vec<Field>> {
    with_registry!(entity, |f| f.fields())
}

/// The decoded-value view.
pub fn field_values(entity: &Entity) -> Option<Vec<FieldValue>> {
    with_registry!(entity, |f| f.field_values())
}

fn parse(bytes: &[u8]) -> Result<Entity, String> {
    nord_format::from_stream(&mut Cursor::new(bytes)).map_err(|e| e.to_string())
}

/// Every registry field of the entity in `bytes` takes one value it does not
/// hold, reaches the bytes, reads back, and moves no other field doing it. A
/// field too wide to enumerate is skipped. `Ok` for an entity with no registry.
pub fn each_field_moves_alone(bytes: &[u8]) -> Result<(), String> {
    let Some(baseline) = fields(&parse(bytes)?) else {
        return Ok(());
    };
    for field in &baseline {
        let Some(value) = (field.spec.legal)().into_iter().find(|v| *v != field.value) else {
            continue;
        };
        let mut edited = parse(bytes)?;
        with_registry!(&mut edited, |f| f.set_field(&field.path, &value))
            .unwrap()
            .map_err(|e| format!("{} = {value}: {e}", field.path))?;
        let out =
            nord_format::to_bytes(&edited).map_err(|e| format!("{} = {value}: {e}", field.path))?;
        let after = fields(&parse(&out)?).ok_or("the edited file lost its registry")?;
        if after.len() != baseline.len() {
            return Err(format!(
                "{} = {value}: {} fields before, {} after",
                field.path,
                baseline.len(),
                after.len()
            ));
        }
        for (before, after) in baseline.iter().zip(&after) {
            if after.path == field.path {
                if after.value != value {
                    return Err(format!(
                        "{} = {value} read back as {}",
                        field.path, after.value
                    ));
                }
            } else if before.value != after.value {
                return Err(format!(
                    "{} = {value} also moved {} ({} -> {})",
                    field.path, after.path, before.value, after.value
                ));
            }
        }
    }
    Ok(())
}
