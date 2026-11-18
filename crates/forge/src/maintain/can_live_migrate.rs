// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;

use super::live_migration_driver::LIVE_MIGRATION_DRIVER;

/// Whether `host`'s agent can take part in a live migration, as source or
/// as destination.
pub fn can_live_migrate(host: &Host) -> bool {
    host.spec
        .drivers
        .iter()
        .any(|d| d.trim().eq_ignore_ascii_case(LIVE_MIGRATION_DRIVER))
}
