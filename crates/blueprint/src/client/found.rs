// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::{Store, StoreError};
use pistack_types::{ApiResult, Id, Kind};

pub(crate) fn found<K: Kind>(
    store: &Store,
    project: Option<&Id>,
    name: &str,
) -> ApiResult<Option<K>> {
    match store.collection::<K>().get_by_name(project, name) {
        Ok(row) => Ok(Some(row)),
        Err(StoreError::NotFound(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
