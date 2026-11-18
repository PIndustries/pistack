// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::next_id::next_id;
use super::notify_bytes::notify_bytes;

/// Encode a NOTIFY for `apex` at `serial`.
pub fn notify_message(apex: &str, serial: u32) -> anyhow::Result<Vec<u8>> {
    notify_bytes(next_id(), apex, serial)
}
