// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Addresses a listener may bind.
//!
//! Loopback, RFC 1918, or IPv6 unique local. Unspecified, public, multicast,
//! broadcast, link-local, and 100.64/10 are refused.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// True when `ip` may be bound by a forwarder or the HTTP proxy.
pub fn allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_allowed(v4),
        IpAddr::V6(v6) => v6_allowed(v6),
    }
}

fn v4_allowed(v4: Ipv4Addr) -> bool {
    if v4.is_unspecified()
        || v4.is_multicast()
        || v4.is_broadcast()
        || v4.is_link_local()
        || is_shared(v4)
    {
        return false;
    }
    v4.is_loopback() || is_rfc1918(v4)
}

fn is_rfc1918(v4: Ipv4Addr) -> bool {
    let [a, b, _, _] = v4.octets();
    a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168)
}

/// Carrier-grade NAT, 100.64.0.0/10.
fn is_shared(v4: Ipv4Addr) -> bool {
    let [a, b, _, _] = v4.octets();
    a == 100 && (64..128).contains(&b)
}

fn v6_allowed(v6: Ipv6Addr) -> bool {
    if v6.is_unspecified() || v6.is_multicast() || v6.is_unicast_link_local() {
        return false;
    }
    v6.is_loopback() || v6.is_unique_local()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
