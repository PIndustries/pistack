// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Kind, ListQuery, Page};

use super::default_limit::DEFAULT_LIMIT;
use super::max_limit::MAX_LIMIT;

/// [`page`] with one more question every item has to answer.
pub(crate) fn page_by<K: Kind>(
    mut items: Vec<K>,
    q: &ListQuery,
    mut keep: impl FnMut(&K) -> bool,
) -> Page<K> {
    let sel = q.label_selector();
    items.retain(|k| {
        let m = k.meta();
        keep(k)
            && q.name.as_deref().is_none_or(|n| m.name == n)
            && sel.iter().all(|(lk, lv)| m.labels.get(lk) == Some(lv))
    });
    items.sort_by(|a, b| a.meta().id.cmp(&b.meta().id));
    let total = items.len();
    if let Some(after) = &q.after {
        items.retain(|k| k.meta().id.to_string().as_str() > after.as_str());
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let next = if items.len() > limit {
        items.get(limit - 1).map(|k| k.meta().id.to_string())
    } else {
        None
    };
    items.truncate(limit);
    Page { items, next, total }
}
