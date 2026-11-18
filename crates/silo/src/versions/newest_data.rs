// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;

/// The newest generation of `key` that holds bytes, ignoring delete markers —
/// the record a put builds on, and the one the key path's file belongs to.
pub(crate) fn newest_data(all: &[Object], key: &str) -> Option<Object> {
    all.iter()
        .filter(|o| o.spec.key == key && !o.status.delete_marker)
        .max_by_key(|o| (o.status.version, o.meta.revision))
        .cloned()
}
