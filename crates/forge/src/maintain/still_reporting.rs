// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use pistack_types::ApiError;

/// Why a host that is disabled but still reporting cannot be evacuated on the
/// strength of a disabled flag alone. The host answers its control plane, so
/// its machines are almost certainly running.
pub(crate) fn still_reporting(host: &Host) -> ApiError {
    let name = &host.meta.name;
    let consequence = if host.spec.power_control.is_some() {
        "evacuating it cuts the host's power and hard-stops every machine on it, since their root \
         disks are here"
    } else {
        "the server cannot cut its power, so evacuating its machines risks starting second copies \
         while these keep running"
    };
    ApiError::conflict(format!(
        "host {name} is disabled but still reporting. {consequence}. Confirm the host is dead \
         (confirm_dead, `pi host evacuate --confirm-dead`) to go ahead, or use drain to empty a \
         working host",
    ))
}
