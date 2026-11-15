// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::ContainerState;
use chrono::Utc;
pub(in crate::controllers::pod) fn mark_running(state: &mut ContainerState) {
    if !matches!(state, ContainerState::Running { .. }) {
        *state = ContainerState::Running {
            started_at: Utc::now(),
        };
    }
}
