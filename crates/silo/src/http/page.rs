// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ListQuery, Page};

use super::default_limit::DEFAULT_LIMIT;
use super::max_limit::MAX_LIMIT;

pub(crate) fn page(mut items: Vec<Bucket>, q: &ListQuery) -> Page<Bucket> {
    let sel = q.label_selector();
    items.retain(|b| {
        q.name.as_deref().is_none_or(|n| b.meta.name == n)
            && sel.iter().all(|(k, v)| b.meta.labels.get(k) == Some(v))
    });
    items.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    let total = items.len();
    if let Some(after) = &q.after {
        items.retain(|b| b.meta.id.to_string().as_str() > after.as_str());
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let next = (items.len() > limit).then(|| items[limit - 1].meta.id.to_string());
    items.truncate(limit);
    Page { items, next, total }
}
