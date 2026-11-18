// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;

/// True when the host's last heartbeat said it is a working chassis.
pub(crate) fn chassis_ready(host: &Host) -> bool {
    host.status.ovn.as_ref().is_some_and(|o| o.ready)
}
