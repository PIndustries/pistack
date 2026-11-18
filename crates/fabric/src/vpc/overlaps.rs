// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;

use super::meets::meets;

/// Do two blocks of the same family share an address?
pub(crate) fn overlaps(a: IpNet, b: IpNet) -> bool {
    meets(a, b) && (a.contains(&b.addr()) || b.contains(&a.addr()))
}
