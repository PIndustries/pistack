// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use pistack_types::ApiError;

/// Why a host without power control cannot be evacuated without a
/// confirmation, naming both ways out.
pub(crate) fn needs_proof(host: &Host) -> ApiError {
    let name = &host.meta.name;
    ApiError::conflict(format!(
        "host {name} has no power control the server can drive, so it cannot prove the host is \
         off. Its machines may be running, and evacuating them would start second copies. Either \
         confirm the host is dead (confirm_dead, `pi host evacuate --confirm-dead`), or give the \
         host power control so the server can power it off and check (`pi host power set`)"
    ))
}
