// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;

use super::index::index;
use super::meets::meets;
use super::span::span;

/// Is `small` wholly inside `big`?
pub(crate) fn inside(big: IpNet, small: IpNet) -> bool {
    if !meets(big, small) {
        return false;
    }
    let base = index(big.network());
    let start = index(small.network());
    start >= base
        && start
            .saturating_sub(base)
            .saturating_add(span(small).saturating_sub(1))
            <= span(big).saturating_sub(1)
}
