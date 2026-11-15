// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use crate::types::EVACUATED_FROM_ANNOTATION;
use pistack_types::Id;
/// Whether `m` carries the marker of an evacuation off `host`.
pub(super) fn evacuated_off(m: &Machine, host: &Id) -> bool {
    m.meta
        .annotations
        .get(EVACUATED_FROM_ANNOTATION)
        .is_some_and(|from| *from == host.to_string())
}
