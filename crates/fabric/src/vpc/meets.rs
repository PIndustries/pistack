// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;

/// Two blocks meet when they are of one family: only then can they share an
/// address, or one hold the other.
pub(crate) fn meets(a: IpNet, b: IpNet) -> bool {
    a.addr().is_ipv4() == b.addr().is_ipv4()
}
