// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

pub(crate) fn project_allows(mine: Option<&Id>, theirs: Option<&Id>) -> bool {
    match (mine, theirs) {
        (_, None) => true,
        (Some(a), Some(b)) => a == b,
        (None, Some(_)) => false,
    }
}
