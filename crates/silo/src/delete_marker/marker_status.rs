// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::ObjectStatus;

/// The status of a marker at `version`.
pub(crate) fn marker_status(version: u64) -> ObjectStatus {
    ObjectStatus {
        sha256: String::new(),
        md5: String::new(),
        size_bytes: 0,
        version,
        latest: true,
        delete_marker: true,
    }
}
