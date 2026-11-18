// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;

/// The record standing at `key`'s newest generation, marker or not, or `None`
/// when the key holds no record.
///
/// The current generation of a key is its newest one, and a plain delete on a
/// versioned bucket leaves a delete marker as that generation, so this is the
/// record an S3 answer is taken from — unlike the record `put_here` replaces,
/// which is the newest one that holds bytes.
pub(crate) fn head(all: &[Object], key: &str) -> Option<Object> {
    all.iter()
        .filter(|o| o.spec.key == key)
        .max_by_key(|o| (o.status.version, o.meta.revision))
        .cloned()
}
