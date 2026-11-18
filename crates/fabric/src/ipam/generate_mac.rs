// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeSet;
use uuid::Uuid;

use super::format_mac::format_mac;
use super::oui::OUI;

/// Deterministically derive a MAC under [`OUI`] from a seed (the port id),
/// stepping past any address in `taken`. The seed makes it reproducible
/// in tests; the collision check makes it unique on a network.
pub fn generate_mac(seed: &Uuid, taken: &BTreeSet<String>) -> ApiResult<String> {
    let bytes = seed.as_bytes();
    let base = u32::from_be_bytes([0, bytes[13], bytes[14], bytes[15]]);
    for step in 0..0x1_0000u32 {
        let nic = (base.wrapping_add(step)) & 0x00ff_ffff;
        let [_, a, b, c] = nic.to_be_bytes();
        let mac = format_mac([OUI[0], OUI[1], OUI[2], a, b, c]);
        if !taken.contains(&mac) {
            return Ok(mac);
        }
    }
    Err(ApiError::conflict("no free mac address"))
}
