// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::app_state::AppState;
use super::resolve_size::resolve_size;

/// The resources a machine is counted for: its size, or nothing when that
/// size is gone.
pub(crate) fn resources_of(s: &AppState, name_or_id: &str) -> pistack_types::Resources {
    resolve_size(s, name_or_id)
        .map(|sz| sz.spec.resources)
        .unwrap_or_default()
}
