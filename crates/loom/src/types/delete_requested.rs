// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::delete_annotation::DELETE_ANNOTATION;

/// Whether a delete has been requested (see `DELETE_ANNOTATION`).
pub fn delete_requested<S, T>(r: &Resource<S, T>) -> bool {
    r.meta.annotations.contains_key(DELETE_ANNOTATION)
}
