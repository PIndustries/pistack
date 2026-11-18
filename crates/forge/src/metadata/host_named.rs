// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiResult, Id};

use crate::types::Host;

pub(crate) fn host_named(store: &Store, node: &str) -> ApiResult<Option<Id>> {
    Ok(store
        .collection::<Host>()
        .list(None)?
        .into_iter()
        .find(|h| h.spec.node_name == node)
        .map(|h| h.meta.id.clone()))
}
