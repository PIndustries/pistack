// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::overlay_sent_map::OverlaySentMap;

/// Mark the send as failed, unless a newer plan was sent since.
pub(crate) fn overlay_failed(sent: &OverlaySentMap, node: &str, body: &str) {
    if let Some(s) = sent.lock().get_mut(node) {
        if s.body == body {
            s.failed = Some(std::time::Instant::now());
        }
    }
}
