// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;

pub(crate) fn member_slug(addr: &SocketAddr) -> String {
    format!("{}-{}", addr.ip(), addr.port())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
