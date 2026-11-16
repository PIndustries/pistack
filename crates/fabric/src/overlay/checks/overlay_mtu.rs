// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::plan_error::PlanError;

pub(in crate::overlay) fn overlay_mtu(
    network_mtu: u16,
    underlay_mtu: u16,
) -> Result<u16, PlanError> {
    let mtu = network_mtu.min(underlay_mtu.saturating_sub(50));
    if mtu == 0 {
        Err(PlanError::Mtu)
    } else {
        Ok(mtu)
    }
}
