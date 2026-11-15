// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::Network;

pub(in crate::overlay) fn real_vni(net: &Network) -> Option<u32> {
    match net.status.segment_id {
        Some(vni) if (1..=16_777_215).contains(&vni) => Some(vni),
        _ => None,
    }
}
