// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The hash `source_ip` places a client by: FNV-1a over the address bytes,
//! fixed so any server in the cluster pins the same client to the same slot.

use std::net::IpAddr;

/// FNV-1a over the address bytes. Fixed so a restart, or another server
/// in the cluster, pins the same client to the same slot.
pub(super) fn hash_ip(ip: IpAddr) -> u64 {
    let mut bytes: Vec<u8> = Vec::new();
    match ip {
        IpAddr::V4(a) => bytes.extend_from_slice(&a.octets()),
        IpAddr::V6(a) => bytes.extend_from_slice(&a.octets()),
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
