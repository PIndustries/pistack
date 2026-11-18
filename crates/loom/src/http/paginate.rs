// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Kind, ListQuery, Page};

/// Sort by id, apply name/label filters, cut a page.
pub fn paginate<K: Kind>(mut items: Vec<K>, q: &ListQuery) -> Page<K> {
    let sel = q.label_selector();
    items.retain(|k| {
        let m = k.meta();
        q.name.as_deref().is_none_or(|n| m.name == n)
            && sel.iter().all(|(lk, lv)| m.labels.get(lk) == Some(lv))
    });
    items.sort_by(|a, b| a.meta().id.cmp(&b.meta().id));
    let total = items.len();
    if let Some(after) = q.after.as_deref() {
        items.retain(|k| k.meta().id.to_string().as_str() > after);
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let next = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|k| k.meta().id.to_string())
    } else {
        None
    };
    Page { items, next, total }
}
