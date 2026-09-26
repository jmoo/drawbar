//! The field registry of any entity whose body declares one, and the mutation
//! check that applies to each such entity: every field is set to a value it does
//! not hold, written, and read back, and no other field changes.
//!
//! ⚠️ Not a test target. Each test target that includes this module compiles its
//! own copy.
#![allow(dead_code)]

use nord_format::cbin::Cbin;
use nord_format::fields::{Field, FieldValue};
use nord_format::layout::{BodyLayout, LayoutField};
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

/// The settable fields, each with its `set_field` spelling and its display.
pub fn fields(entity: &Entity) -> Option<Vec<Field>> {
    with_registry!(entity, |f| f.fields())
}

/// The decoded-value view.
pub fn field_values(entity: &Entity) -> Option<Vec<FieldValue>> {
    with_registry!(entity, |f| f.field_values())
}

/// The body type an entity decodes to, named by model module and type, such as
/// `ne5-Program`, so the name is usable in a file name. Entities over the same
/// body share the name: `ne5p` and `ne5l` are one body.
pub fn body_type(entity: &Entity) -> Option<String> {
    with_registry!(entity, |f| body_name(f))
}

/// The body's declared bit map. Nested entries link to their own layouts.
pub fn layout(entity: &Entity) -> Option<&'static [LayoutField]> {
    with_registry!(entity, |f| body_layout(f))
}

/// Generic over the container so `with_registry!` can name a body type it never
/// spells out.
fn body_name<B>(_: &Cbin<B>) -> String {
    model_qualified(std::any::type_name::<B>())
}

fn body_layout<B: BodyLayout>(_: &Cbin<B>) -> &'static [LayoutField] {
    B::layout()
}

/// `…::formats::ne5::program::Program` → `ne5-Program`. Several models declare a
/// `Program`, so the model module qualifies the type name.
fn model_qualified(path: &str) -> String {
    let parts: Vec<&str> = path.split("::").collect();
    let name = parts.last().copied().unwrap_or(path);
    match parts.iter().position(|p| *p == "formats") {
        Some(at) if at + 2 < parts.len() => format!("{}-{name}", parts[at + 1]),
        _ => name.to_string(),
    }
}

fn parse(bytes: &[u8]) -> Result<Entity, String> {
    nord_format::from_stream(&mut Cursor::new(bytes)).map_err(|e| e.to_string())
}

/// Sets each registry field of the entity in `bytes` to a value it does not hold,
/// writes the file, and reads it back. The field must read back the new value and
/// no other field may change. A field too wide to enumerate is skipped. `Ok` for
/// an entity with no registry.
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
