// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;

/// An address written bare and the same address with its host prefix are one
/// pair, so both are read as a prefix.
pub(crate) fn pair_net(net: IpNet) -> String {
    let t = net.trunc();
    format!("{}/{}", t.addr(), t.prefix_len())
}
