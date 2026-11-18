// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;
use pistack_types::ApiResult;

use super::no_such_version::no_such_version;

/// The record of `key` at generation `version` among `all`, or
/// `NoSuchVersion`.
pub(crate) fn generation(all: &[Object], key: &str, version: u64) -> ApiResult<Object> {
    let mut found: Option<Object> = None;
    for obj in all {
        if obj.spec.key != key || obj.status.version != version {
            continue;
        }
        // Two records claiming one generation means a writer raced; the newer
        // row is the one still standing.
        if found
            .as_ref()
            .is_none_or(|cur| obj.meta.revision >= cur.meta.revision)
        {
            found = Some(obj.clone());
        }
    }
    found.ok_or_else(|| no_such_version(&version.to_string()))
}
