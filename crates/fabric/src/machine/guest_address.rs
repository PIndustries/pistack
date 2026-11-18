// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Meta;
use std::net::IpAddr;

/// A machine and the first IPv4 forge reported for it.
pub(crate) struct GuestAddress {
    pub meta: Meta,
    pub ipv4: IpAddr,
}
