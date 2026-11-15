// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

/// A stable locally administered MAC for a subnet's DHCP server, from the
/// random tail of its id.
pub(in crate::backend::ovn) fn server_mac(id: &Id) -> String {
    let b = id.uuid.as_bytes();
    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        b[11], b[12], b[13], b[14], b[15]
    )
}
