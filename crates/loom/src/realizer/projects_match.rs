// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

pub(crate) fn projects_match(a: &Option<Id>, b: &Option<Id>) -> bool {
    match (a, b) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}
