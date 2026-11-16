// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `<mac> <address>...`, one OVN address entry.
pub(in crate::backend::ovn) fn address_entry(
    mac: &str,
    ips: impl IntoIterator<Item = String>,
) -> String {
    let mut a = mac.to_string();
    for ip in ips {
        a.push(' ');
        a.push_str(&ip);
    }
    a
}
