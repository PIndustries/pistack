// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_types::Condition;

/// Replace-or-append a condition by type.
pub fn set_condition(
    conditions: &mut Vec<Condition>,
    kind: &str,
    status: bool,
    reason: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    if let Some(c) = conditions.iter_mut().find(|c| c.kind == kind) {
        if c.status != status {
            c.last_transition = Utc::now();
        }
        c.status = status;
        c.reason = reason.to_owned();
        c.message = message;
        return;
    }
    conditions.push(Condition {
        kind: kind.to_owned(),
        status,
        reason: reason.to_owned(),
        message,
        last_transition: Utc::now(),
    });
}
