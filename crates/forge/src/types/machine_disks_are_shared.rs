// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::disk_label::DISK_LABEL;
use super::disk_shared::DISK_SHARED;
use super::host::Host;

/// Whether this host's machine root disks are on storage every host can see.
/// A host that did not report the label is treated as local.
pub fn machine_disks_are_shared(host: &Host) -> bool {
    host.meta
        .labels
        .get(DISK_LABEL)
        .is_some_and(|v| v == DISK_SHARED)
}
