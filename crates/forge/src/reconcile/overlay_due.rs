// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;

use super::overlay_retry::OVERLAY_RETRY;
use super::overlay_sent::OverlaySent;

/// Whether `node` needs this plan sent (again).
pub(crate) fn overlay_due(
    prev: Option<&OverlaySent>,
    body: &str,
    hello: Option<chrono::DateTime<Utc>>,
) -> bool {
    match prev {
        None => true,
        Some(s) => {
            s.body != body
                || s.hello != hello
                || s.failed.is_some_and(|t| t.elapsed() >= OVERLAY_RETRY)
        }
    }
}
