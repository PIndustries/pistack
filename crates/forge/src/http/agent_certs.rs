// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::app_state::AppState;

/// The certificates a host's expiry is read from. A list that cannot be
/// read leaves the column empty and does not fail the request.
pub(crate) fn agent_certs(s: &AppState) -> Vec<keep::Certificate> {
    s.store
        .collection::<keep::Certificate>()
        .list(None)
        .unwrap_or_default()
}
