//! What a bundle of objects on the instrument holds: the set lists and programs named,
//! every program a set list among them plays, and every piano and sample those programs
//! play.

use nord_format::bundle::Key;

use crate::device::Device;
use crate::op;
use crate::transport::Transport;
use crate::wire::{Dependency, Location, ObjectClass};
use crate::{Error, Result};

/// A piano or sample a bundle needs, and the slot that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub class: ObjectClass,
    pub at: Location,
    /// The id the programs that play it know it by.
    pub id: u32,
    pub name: String,
}

/// The objects a bundle of roots holds, as the instrument holds them now.
#[derive(Default)]
pub struct Closure {
    /// Each set list and program, set lists first, with its dependency rows as the
    /// instrument gave them.
    pub objects: Vec<(ObjectClass, Location, Vec<Dependency>)>,
    pub libraries: Vec<Library>,
    /// Each required piano or sample row no slot holds under its name, or more than one
    /// does.
    pub unfound: Vec<Dependency>,
}

impl Closure {
    /// Every slot the bundle reads, objects first.
    pub fn slots(&self) -> impl Iterator<Item = (ObjectClass, Location)> + '_ {
        let objects = self.objects.iter().map(|(class, at, _)| (*class, *at));
        objects.chain(
            self.libraries
                .iter()
                .map(|library| (library.class, library.at)),
        )
    }
}

/// What an object needs, as the instrument's dependency rows for it say.
///
/// The rows, not the object's file, are what it plays: the instrument can report a
/// piano or sample the file leaves at zero. Confirmed on hardware.
pub fn needs(rows: &[Dependency]) -> Vec<Key> {
    let required = rows.iter().filter(|row| row.is_required());
    required
        .filter_map(|row| match (row.class, row.location) {
            (ObjectClass::Piano, _) => Some(Key::Piano(row.id)),
            (ObjectClass::Sample, _) => Some(Key::Sample(row.id)),
            (ObjectClass::Program, Some(at)) => Some(Key::Program(
                u16::try_from(at.bank).ok()?,
                u16::try_from(at.slot).ok()?,
            )),
            _ => None,
        })
        .collect()
}

/// Walk `roots`, set lists and programs, to everything a bundle of them holds. Read-only:
/// one session per class.
///
/// A piano or sample is found by the name its referrer's dependency row gives it, since
/// an object's info does not carry the id programs know it by. Confirmed on hardware.
pub async fn closure<T: Transport>(
    device: &mut Device<T>,
    roots: &[(ObjectClass, Location)],
) -> Result<Closure> {
    if let Some((class, _)) = roots
        .iter()
        .find(|(class, _)| !matches!(class, ObjectClass::SetList | ObjectClass::Program))
    {
        return Err(Error::InvalidArgument(format!(
            "a bundle is made of set lists and programs, not {}",
            class.label()
        )));
    }
    let of = |wanted: ObjectClass| -> Vec<Location> {
        let mut held: Vec<Location> = Vec::new();
        for (_, at) in roots.iter().filter(|(class, _)| *class == wanted) {
            if !held.contains(at) {
                held.push(*at);
            }
        }
        held
    };
    let mut closure = Closure::default();

    let mut programs = of(ObjectClass::Program);
    for (at, deps) in dependencies(device, ObjectClass::SetList, &of(ObjectClass::SetList)).await? {
        let played = deps.iter().filter(|row| row.is_required());
        for program in played.filter_map(|row| row.location) {
            if !programs.contains(&program) {
                programs.push(program);
            }
        }
        closure.objects.push((ObjectClass::SetList, at, deps));
    }

    let mut wanted: Vec<Dependency> = Vec::new();
    for (at, deps) in dependencies(device, ObjectClass::Program, &programs).await? {
        for row in deps
            .iter()
            .filter(|row| row.is_required() && row.class.is_library())
        {
            let seen = wanted
                .iter()
                .any(|w| (w.class, w.id) == (row.class, row.id));
            if !seen {
                wanted.push(row.clone());
            }
        }
        closure.objects.push((ObjectClass::Program, at, deps));
    }

    for class in [ObjectClass::Piano, ObjectClass::Sample] {
        let rows: Vec<Dependency> = wanted
            .iter()
            .filter(|row| row.class == class)
            .cloned()
            .collect();
        if rows.is_empty() {
            continue;
        }
        let held = named(device, class).await?;
        for row in rows {
            let name = row.name.trim_end();
            let mut holding = held.iter().filter(|(_, held)| held.trim_end() == name);
            match (holding.next(), holding.next()) {
                (Some((at, _)), None) => closure.libraries.push(Library {
                    class,
                    at: *at,
                    id: row.id,
                    name: name.to_string(),
                }),
                _ => closure.unfound.push(row),
            }
        }
    }
    Ok(closure)
}

/// Each of `slots` with its dependency rows, in one session of `class`.
async fn dependencies<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
    slots: &[Location],
) -> Result<Vec<(Location, Vec<Dependency>)>> {
    if slots.is_empty() {
        return Ok(Vec::new());
    }
    device
        .read(class, async |s| {
            let mut read = Vec::new();
            for &at in slots {
                read.push((at, op::dependencies(s, at).await?));
            }
            Ok(read)
        })
        .await
}

/// Every occupied slot of `class` with its name, in one session.
async fn named<T: Transport>(
    device: &mut Device<T>,
    class: ObjectClass,
) -> Result<Vec<(Location, String)>> {
    let banks = device.geometry().await?.banks(class)?.to_vec();
    device
        .read(class, async |s| {
            let mut held = Vec::new();
            for at in op::occupied_slots(s, &banks).await? {
                match op::info(s, at).await {
                    Ok(info) => held.push((at, info.name)),
                    // The cursor may land on an empty starting address.
                    Err(Error::DeviceStatus(op::VACANT)) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(held)
        })
        .await
}
