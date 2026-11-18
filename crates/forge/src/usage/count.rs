// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiResult, Id, Resources};
use std::collections::BTreeMap;

use crate::types::{Machine, Size, DELETE_ANNOTATION};

use super::usage_type::Usage;

/// Count the project's machines and what their sizes add up to.
pub fn count(store: &Store, project: &Id) -> ApiResult<Usage> {
    let sizes: BTreeMap<String, Resources> = store
        .collection::<Size>()
        .list(None)?
        .into_iter()
        .map(|s| (s.meta.name.clone(), s.spec.resources))
        .collect();
    let mut usage = Usage::default();
    for m in store.collection::<Machine>().list(Some(project))? {
        if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
            continue;
        }
        let r = sizes.get(&m.spec.size).copied().unwrap_or_default();
        usage.add(&r);
    }
    Ok(usage)
}
