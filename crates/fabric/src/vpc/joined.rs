// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;

pub(crate) fn joined(cidrs: &[IpNet]) -> String {
    cidrs
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
