// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;

use super::marker_status::marker_status;

/// A delete marker for `key` at `version`: no bytes, no checksum, size 0.
pub(crate) fn new_marker(bucket: &crate::types::Bucket, key: &str, version: u64) -> Object {
    let spec = crate::types::ObjectSpec {
        bucket: bucket.meta.id.clone(),
        key: key.to_owned(),
        content_type: crate::types::content_type_or_default(None),
    };
    let mut obj = Object::new("object", bucket.meta.project.clone(), spec);
    obj.meta.name = obj.meta.id.to_string();
    obj.status = marker_status(version);
    obj
}
