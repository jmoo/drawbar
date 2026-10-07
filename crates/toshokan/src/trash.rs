//! A writer's trash: bytes its intents displaced, kept for undo until it empties
//! them. Emptying a trash is the only way bytes leave the folder, and only its
//! owner empties it.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::flow::{self, each};
use crate::ids::WriterId;
use crate::io::{Kind, Root, Task};
use crate::layout::Layout;
use crate::path::RelPath;
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

/// The items of `logged`, what `writer`'s entries say they displaced, still in
/// its trash, oldest first, each with its length there. Files in the trash no
/// entry displaced are not listed, and so never emptied. Reads only, in one
/// request.
pub fn list(
    layout: &Layout,
    writer: WriterId,
    logged: Vec<TrashItem>,
) -> Task<'static, Result<Vec<TrashItem>>> {
    flow::list_stat(Root::Folder, &layout.trash_dir(writer))
        .map_ok(move |entries| {
            let mut logged: BTreeMap<String, TrashItem> = logged
                .into_iter()
                .map(|item| (item.item.to_string(), item))
                .collect();
            let mut items: Vec<TrashItem> = entries
                .into_iter()
                .filter(|(_, meta)| meta.kind == Kind::File)
                .filter_map(|(name, meta)| {
                    let item = logged.remove(&name)?;
                    Some(TrashItem {
                        len: meta.len,
                        ..item
                    })
                })
                .collect();
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
/// at wall time `now_ms`, then syncs the trash once.
pub fn empty(
    layout: &Layout,
    writer: WriterId,
    items: Vec<TrashItem>,
    policy: Policy,
    now_ms: u64,
) -> Task<'static, Result<Emptied>> {
    let removed = expired(&items, policy, now_ms);
    let emptied = Emptied {
        removed: removed.iter().map(|item| item.item).collect(),
        bytes: removed
            .iter()
            .map(|item| item.len)
            .fold(0, u64::saturating_add),
    };
    if removed.is_empty() {
        return Task::ready(Ok(emptied));
    }
    let paths: Vec<RelPath> = removed
        .iter()
        .map(|item| layout.trash(writer, item.item))
        .collect();
    let dir = layout.trash_dir(writer);
    each(paths.into_iter(), |path| {
        flow::remove_if_present(Root::Folder, path)
    })
    .and_then(move |()| flow::sync(Root::Folder, &dir))
    .map_ok(move |()| emptied)
    .task()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EntryHash, Hlc, Nonce};

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
