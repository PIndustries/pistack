// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::api_state::ApiState;

/// Ask for a NOTIFY, if this node runs the notify loop. The write is already
/// in the store; the response to the caller does not wait for any of it.
pub(crate) fn asked(st: &ApiState, apex: &str) {
    if let Some(n) = &st.notifier {
        n.notify_zone(apex);
    }
}
