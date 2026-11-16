// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use forge::{Machine, PowerState};
use pistack_types::Phase;
pub(in crate::controllers::pod) fn machine_live(m: &Machine) -> bool {
    m.status.phase == Phase::Ready
        && m.status.power != PowerState::Off
        && m.spec.restarts <= m.status.restarts
}
