// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;

use super::head::head;

/// The next generation number at `key`, counting markers: a key whose three
/// generations ended in a plain delete is written at 4, not 3.
pub(crate) fn next_version(all: &[Object], key: &str) -> u64 {
    head(all, key)
        .map(|o| o.status.version.saturating_add(1))
        .unwrap_or(1)
}
