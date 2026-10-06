//! A writer's trash: bytes its intents displaced, kept for undo until it empties
//! them. Emptying a trash is the only way bytes leave the folder, and only its
//! owner empties it.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::flow::{self, each, fold, ok};
use crate::ids::WriterId;
use crate::io::{Kind, Root, Task};
use crate::layout::Layout;
use crate::log::EntryKind;
use crate::reader::WriterLog;
use crate::report::{Emptied, TrashItem};

/// What emptying keeps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Policy {
    /// Items displaced longer ago than this are removed.
    pub max_age_ms: u64,
    /// Then the oldest items are removed until the rest fit.
    pub max_bytes: u64,
}

impl Default for Policy {
    /// Thirty days, and a gibibyte.
    fn default() -> Self {
        Self {
            max_age_ms: 30 * 24 * 60 * 60 * 1000,
            max_bytes: 1 << 30,
        }
    }
}

/// What `own`'s entries say they displaced.
pub fn logged(own: &WriterLog) -> Vec<TrashItem> {
    own.entries()
        .iter()
        .flat_map(|entry| match &entry.kind {
            EntryKind::Intent(logged) => logged
                .displaced
                .iter()
                .map(|displaced| TrashItem {
                    item: displaced.item,
                    len: displaced.len,
                    from: displaced.from.clone(),
                    at: entry.at,
                    by: entry.hash(),
                })
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// The items of `logged` still in `writer`'s trash, oldest first, each with its
/// length there. Files in the trash no entry displaced are not listed, and so
/// never emptied. Reads only.
pub fn list(
    layout: &Layout,
    writer: WriterId,
    logged: Vec<TrashItem>,
) -> Task<'static, Result<Vec<TrashItem>>> {
    let dir = layout.trash_dir(writer);
    flow::list(Root::Folder, &dir)
        .and_then(move |entries| {
            let mut logged: BTreeMap<String, TrashItem> = logged
                .into_iter()
                .map(|item| (item.item.to_string(), item))
                .collect();
            let present: Vec<TrashItem> = entries
                .into_iter()
                .filter(|entry| entry.kind == Kind::File)
                .filter_map(|entry| logged.remove(&entry.name))
                .collect();
            fold(present.into_iter(), Vec::new(), move |mut items, item| {
                let path = dir
                    .join(&item.item.to_string())
                    .expect("a nonce is one component");
                flow::stat(Root::Folder, &path).map_ok(move |meta| {
                    items.extend(meta.map(|meta| TrashItem {
                        len: meta.len,
                        ..item
                    }));
                    items
                })
            })
        })
        .map_ok(|mut items: Vec<TrashItem>| {
            items.sort_by_key(|item| (item.at, item.item));
            items
        })
        .task()
}

/// The items `policy` does not keep at wall time `now_ms`, from `items` oldest
/// first.
pub fn expired(items: &[TrashItem], policy: Policy, now_ms: u64) -> Vec<TrashItem> {
    let mut items = items.to_vec();
    items.sort_by_key(|item| (item.at, item.item));
    let mut kept: u64 = items
        .iter()
        .map(|item| item.len)
        .fold(0, u64::saturating_add);
    items
        .into_iter()
        .take_while(|item| {
            let old = now_ms.saturating_sub(item.at.wall_ms) > policy.max_age_ms;
            let remove = old || kept > policy.max_bytes;
            if remove {
                kept -= item.len;
            }
            remove
        })
        .collect()
}

/// Removes from `writer`'s trash the items of `items` that `policy` does not keep
/// at wall time `now_ms`.
pub fn empty(
    layout: &Layout,
    writer: WriterId,
    items: Vec<TrashItem>,
    policy: Policy,
    now_ms: u64,
) -> Task<'static, Result<Emptied>> {
    let layout = layout.clone();
    let removed = expired(&items, policy, now_ms);
    let emptied = Emptied {
        removed: removed.iter().map(|item| item.item).collect(),
        bytes: removed
            .iter()
            .map(|item| item.len)
            .fold(0, u64::saturating_add),
    };
    each(removed.into_iter(), move |item| {
        flow::remove(Root::Folder, &layout.trash(writer, item.item))
    })
    .and_then(move |()| ok(emptied))
    .task()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EntryHash, Hlc, Nonce};
    use crate::path::RelPath;

    const DAY: u64 = 24 * 60 * 60 * 1000;

    fn item(n: u128, day: u64, len: u64) -> TrashItem {
        TrashItem {
            item: Nonce::from_u128(n),
            len,
            from: RelPath::new("f").unwrap(),
            at: Hlc {
                wall_ms: day * DAY,
                counter: 0,
            },
            by: EntryHash::from_u128(n),
        }
    }

    fn names(items: &[TrashItem]) -> Vec<u128> {
        items.iter().map(|item| item.item.to_u128()).collect()
    }

    #[test]
    fn emptying_removes_what_is_older_than_the_age_then_the_oldest_over_the_cap() {
        let policy = Policy {
            max_age_ms: 30 * DAY,
            max_bytes: 100,
        };
        let items = [
            item(3, 40, 60),
            item(1, 1, 10),
            item(2, 39, 60),
            item(4, 41, 30),
        ];
        assert_eq!(
            names(&expired(&items, policy, 45 * DAY)),
            [1, 2],
            "day 1 by age, day 39 by size"
        );
        assert_eq!(names(&expired(&items, policy, 20 * DAY)), [1, 2]);
        let roomy = Policy {
            max_bytes: 1000,
            ..policy
        };
        assert_eq!(names(&expired(&items, roomy, 20 * DAY)), Vec::<u128>::new());
        assert_eq!(names(&expired(&items, roomy, 71 * DAY)), [1, 2, 3]);
    }

    #[test]
    fn the_default_policy_keeps_thirty_days_and_a_gibibyte() {
        let items = [item(1, 0, 1 << 29), item(2, 10, 1 << 29), item(3, 20, 1)];
        assert_eq!(names(&expired(&items, Policy::default(), 30 * DAY)), [1]);
        assert_eq!(names(&expired(&items, Policy::default(), 31 * DAY)), [1]);
    }
}
