// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use ipnet::IpNet;
pub(crate) fn prefix_len(net: IpNet) -> u8 {
    match net {
        IpNet::V4(n) => n.prefix_len(),
        IpNet::V6(n) => n.prefix_len(),
    }
}
