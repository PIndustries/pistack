// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::{Store, StoreError};
use pistack_types::{ApiError, ApiResult, Id, Kind};

pub(crate) fn find_named<K: Kind>(
    store: &Store,
    project: Option<&Id>,
    name_or_id: &str,
    owner_kind: &str,
    owner: &str,
    target: &str,
) -> ApiResult<K> {
    let col = store.collection::<K>();
    let row = if let Some(id) = Id::parse(K::KIND, name_or_id) {
        match col.get(&id) {
            Ok(row) => row,
            Err(StoreError::NotFound(_)) => {
                return Err(ApiError::bad_request(format!(
                    "{owner_kind} {owner}: {target} '{name_or_id}' was not found in this project"
                )))
            }
            Err(e) => return Err(e.into()),
        }
    } else {
        match col.get_by_name(project, name_or_id) {
            Ok(row) => row,
            Err(StoreError::NotFound(_)) => {
                return Err(ApiError::bad_request(format!(
                    "{owner_kind} {owner}: {target} '{name_or_id}' was not found in this project"
                )))
            }
            Err(e) => return Err(e.into()),
        }
    };
    if row.meta().project.as_ref() != project {
        return Err(ApiError::bad_request(format!(
            "{owner_kind} {owner}: {target} '{name_or_id}' is not in this project"
        )));
    }
    Ok(row)
}
