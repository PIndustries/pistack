// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, Size};
use pistack_store::Store;
use pistack_types::{ApiResult, Resources};

/// Sizes by name and by id, as `spec.size` may hold either.
pub(crate) fn size_lookup(store: &Store) -> ApiResult<impl Fn(&Machine) -> Option<Resources>> {
    let sizes: Vec<Size> = store.collection::<Size>().list(None)?;
    Ok(move |m: &Machine| {
        sizes
            .iter()
            .find(|s| s.meta.name == m.spec.size || s.meta.id.to_string() == m.spec.size)
            .map(|s| s.spec.resources)
    })
}
