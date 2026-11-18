// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

use super::pick_current::pick_current;

/// The current record of `key`, or the error a read of the key is owed.
///
/// A key whose current generation is a delete marker is not there: 404, with
/// the marker's version in `details` so the S3 route can answer
/// `x-amz-delete-marker: true` and name it.
pub(crate) fn current_to_read(all: &[Object], key: &str) -> ApiResult<Object> {
    match pick_current(all.iter().filter(|o| o.spec.key == key).cloned().collect()) {
        None => Err(ApiError::not_found(format!("object '{key}' not found"))),
        Some(obj) if obj.status.delete_marker => Err(crate::delete_marker::hidden(key, &obj)),
        Some(obj) => Ok(obj),
    }
}
