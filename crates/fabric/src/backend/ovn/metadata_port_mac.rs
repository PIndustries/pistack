// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

/// A stable locally administered MAC for the metadata port, from the
/// random tail of the network id. `06:` keeps it clear of the `02:` MACs
/// fabric gives guests and DHCP servers.
pub fn metadata_port_mac(net: &Id) -> String {
    let b = net.uuid.as_bytes();
    format!(
        "06:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        b[11], b[12], b[13], b[14], b[15]
    )
}
