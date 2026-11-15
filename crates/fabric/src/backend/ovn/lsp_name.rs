// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

/// Name of the switch port a guest or pod plugs into: the port id without
/// its `port-` prefix. The host sets it as `iface-id`, and libvirt takes
/// nothing but a UUID there.
pub(in crate::backend::ovn) fn lsp_name(port: &Id) -> String {
    port.uuid.to_string()
}
