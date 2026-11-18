// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Kind, ListQuery, Page};

pub(crate) fn page<K: Kind>(items: Vec<K>, q: &ListQuery) -> Page<K> {
    let sel = q.label_selector();
    let mut items: Vec<K> = items
        .into_iter()
        .filter(|k| {
            sel.iter()
                .all(|(lk, lv)| k.meta().labels.get(lk) == Some(lv))
        })
        .filter(|k| q.name.as_deref().is_none_or(|n| k.meta().name == n))
        .collect();
    let total = items.len();
    if let Some(l) = q.limit {
        items.truncate(l);
    }
    Page {
        items,
        next: None,
        total,
    }
}
