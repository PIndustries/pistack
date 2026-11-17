// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::StackStatus;

/// Ids this resource may update. A status written before `managed`
/// existed falls back to the ids its last apply inserted.
pub(crate) fn managed_ids(status: &StackStatus) -> Vec<String> {
    if status.managed.is_empty() {
        status.created.clone()
    } else {
        status.managed.clone()
    }
}
