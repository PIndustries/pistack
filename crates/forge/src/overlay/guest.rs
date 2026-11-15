// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::Port;
use std::net::Ipv4Addr;

pub(in crate::overlay) struct Guest {
    pub(in crate::overlay) port: Port,
    pub(in crate::overlay) underlay: Ipv4Addr,
    pub(in crate::overlay) node: String,
}
