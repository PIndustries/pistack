// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;

pub(crate) fn pick_current(matches: Vec<Object>) -> Option<Object> {
    // Newest generation wins — that is the key's current record, marker
    // included. `latest` only breaks a tie between two records written at the
    // same generation, which a racing writer can leave behind.
    matches
        .into_iter()
        .max_by_key(|o| (o.status.version, o.status.latest, o.meta.revision))
}
