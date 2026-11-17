// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Bind-address tests: what a forwarder may bind and what it refuses.

use super::allowed::allowed;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[test]
fn allows_loopback_rfc1918_and_unique_local() {
    assert!(allowed(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    assert!(allowed(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    assert!(allowed(IpAddr::V4(Ipv4Addr::new(10, 255, 255, 254))));
    assert!(allowed(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
    assert!(allowed(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
    assert!(allowed(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
    assert!(allowed(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    assert!(allowed(IpAddr::V6("fd00::1".parse().unwrap())));
    assert!(allowed(IpAddr::V6("fc00::1".parse().unwrap())));
}

#[test]
fn refuses_unspecified_public_multicast_broadcast_link_local_and_cgnat() {
    let refused = [
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V4(Ipv4Addr::BROADCAST),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
        IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(169, 254, 0, 0)),
        IpAddr::V4(Ipv4Addr::new(100, 64, 0, 0)),
        IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255)),
        IpAddr::V4(Ipv4Addr::new(172, 15, 255, 255)),
        IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(192, 167, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(11, 0, 0, 1)),
        IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        IpAddr::V6("ff02::1".parse().unwrap()),
        IpAddr::V6("fe80::1".parse().unwrap()),
        IpAddr::V6("2001:db8::1".parse().unwrap()),
        IpAddr::V4(Ipv4Addr::new(100, 63, 255, 255)),
        IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1)),
    ];
    for ip in refused {
        assert!(!allowed(ip), "{ip}");
    }
}
