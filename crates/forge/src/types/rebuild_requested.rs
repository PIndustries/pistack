// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::machine::Machine;
use super::rebuild_annotation::REBUILD_ANNOTATION;

/// Whether a root disk rebuild is owed to this machine (see
/// `REBUILD_ANNOTATION`).
pub fn rebuild_requested(m: &Machine) -> bool {
    m.meta
        .annotations
        .get(REBUILD_ANNOTATION)
        .is_some_and(|v| !v.is_empty())
}
