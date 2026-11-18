// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_types::Condition;

// ---- kinds and well-known keys ------------------------------------------------

/// Set a condition on a list, replacing one of the same type; bumps
/// `last_transition` only when `status` or `reason` changed.
pub fn set_condition(
    conditions: &mut Vec<Condition>,
    kind: &str,
    status: bool,
    reason: &str,
    message: &str,
) {
    if let Some(c) = conditions.iter_mut().find(|c| c.kind == kind) {
        if c.status != status || c.reason != reason {
            c.last_transition = Utc::now();
        }
        c.status = status;
        c.reason = reason.to_owned();
        c.message = message.to_owned();
        return;
    }
    conditions.push(Condition {
        kind: kind.to_owned(),
        status,
        reason: reason.to_owned(),
        message: message.to_owned(),
        last_transition: Utc::now(),
    });
}
