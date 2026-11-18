// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Kind, ListQuery, Page};

use super::page_by::page_by;

/// Filter by name/labels, then page by id (ids are time-ordered).
pub(crate) fn page<K: Kind>(items: Vec<K>, q: &ListQuery) -> Page<K> {
    page_by(items, q, |_| true)
}
