// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

/// Name of the network's metadata port. The agent on each host binds its
/// end to the port by this name.
pub fn metadata_port_name(net: &Id) -> String {
    format!("pimeta-{}", net.uuid)
}
