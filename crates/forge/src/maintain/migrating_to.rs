// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use pistack_types::Phase;

/// The host a machine is being live-migrated to, read off the status
/// `begin_migration` wrote.
pub(crate) fn migrating_to(m: &Machine) -> Option<&str> {
    if m.status.phase != Phase::Updating {
        return None;
    }
    m.status.message.strip_prefix("migrating ")
}
