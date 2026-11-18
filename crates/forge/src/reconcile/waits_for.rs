// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::error::ErrorCode;
use pistack_types::ApiError;

/// A start error that only means fabric has not caught up: the machine
/// stays in creating and is started again on the next pass. The value is
/// the reason recorded on the `Scheduled` condition.
pub(crate) fn waits_for(e: &ApiError) -> Option<&'static str> {
    if e.code != ErrorCode::Unavailable {
        return None;
    }
    if e.message.contains("no segment") {
        Some("Segment")
    } else if e.message.contains("not bound in OVN") {
        Some("PortBinding")
    } else {
        None
    }
}
