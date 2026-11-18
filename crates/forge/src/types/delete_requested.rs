// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Phase;

use super::delete_annotation::DELETE_ANNOTATION;
use super::machine::Machine;

/// Whether a delete has been requested (see `DELETE_ANNOTATION`).
pub fn delete_requested(m: &Machine) -> bool {
    m.meta.annotations.contains_key(DELETE_ANNOTATION) || m.status.phase == Phase::Deleting
}
