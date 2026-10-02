// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! IP and MAC address management.
//!
//! Allocation is deterministic: walk the subnet's pools in order and hand
//! out the first address that is not the network, broadcast or gateway
//! address and is not already held. Callers supply the set of held
//! addresses (scanned from the subnet's ports and the network's floating
//! IPs); this module keeps no state of its own, so it is trivially correct
//! under a single-writer store and easy to test.

use crate::types::{AllocationPool, SubnetSpec};
use ipnet::IpNet;
use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use uuid::Uuid;

/// Locally administered OUI for every MAC fabric generates: `02:70:69`
/// ("pi", with the local bit set).
pub const OUI: [u8; 3] = [0x02, 0x70, 0x69];

fn ip_to_u128(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(v4) => u32::from(v4) as u128,
        IpAddr::V6(v6) => u128::from(v6),
    }
}

fn u128_to_ip(n: u128, v4: bool) -> Option<IpAddr> {
    if v4 {
        u32::try_from(n).ok().map(|x| IpAddr::V4(Ipv4Addr::from(x)))
    } else {
        Some(IpAddr::V6(Ipv6Addr::from(n)))
    }
}

/// The address after `ip`, or `None` at the top of the address family.
pub fn next_ip(ip: IpAddr) -> Option<IpAddr> {
    u128_to_ip(ip_to_u128(ip).checked_add(1)?, ip.is_ipv4())
}

/// Addresses never handed out: the network address, the broadcast address
/// (IPv4 only) and the gateway.
pub fn is_reserved(spec: &SubnetSpec, ip: IpAddr) -> bool {
    if spec.gateway == Some(ip) {
        return true;
    }
    match spec.cidr {
        IpNet::V4(n) => n.network() == ip || n.broadcast() == ip,
        IpNet::V6(n) => n.network() == ip,
    }
}

/// The subnet's effective pools: what the user gave, or the whole range.
pub fn pools(spec: &SubnetSpec) -> Vec<AllocationPool> {
    if !spec.allocation_pools.is_empty() {
        return spec.allocation_pools.clone();
    }
    let (start, end) = match spec.cidr {
        IpNet::V4(n) => {
            if n.prefix_len() >= 31 {
                (IpAddr::V4(n.network()), IpAddr::V4(n.broadcast()))
            } else {
                let s = u32::from(n.network()).saturating_add(1);
                let e = u32::from(n.broadcast()).saturating_sub(1);
                (IpAddr::V4(Ipv4Addr::from(s)), IpAddr::V4(Ipv4Addr::from(e)))
            }
        }
        IpNet::V6(n) => {
            let s = u128::from(n.network()).saturating_add(1);
            (IpAddr::V6(Ipv6Addr::from(s)), IpAddr::V6(n.broadcast()))
        }
    };
    vec![AllocationPool { start, end }]
}

/// Reject a subnet spec whose pools or gateway do not fit its CIDR.
pub fn validate_subnet(spec: &SubnetSpec) -> ApiResult<()> {
    if let Some(gw) = spec.gateway {
        if !spec.cidr.contains(&gw) {
            return Err(ApiError::bad_request(format!(
                "gateway {gw} is outside {}",
                spec.cidr
            )));
        }
    }
    for p in &spec.allocation_pools {
        if p.start.is_ipv4() != spec.cidr.addr().is_ipv4() || p.end.is_ipv4() != p.start.is_ipv4() {
            return Err(ApiError::bad_request(
                "allocation pool family does not match cidr",
            ));
        }
        if !spec.cidr.contains(&p.start) || !spec.cidr.contains(&p.end) {
            return Err(ApiError::bad_request(format!(
                "allocation pool {}-{} is outside {}",
                p.start, p.end, spec.cidr
            )));
        }
        if ip_to_u128(p.start) > ip_to_u128(p.end) {
            return Err(ApiError::bad_request(format!(
                "allocation pool start {} is after end {}",
                p.start, p.end
            )));
        }
    }
    for r in &spec.host_routes {
        if r.nexthop.is_ipv4() != spec.cidr.addr().is_ipv4() {
            return Err(ApiError::bad_request(
                "host route nexthop family does not match cidr",
            ));
        }
    }
    Ok(())
}

/// Allocate the lowest free address in the subnet's pools.
pub fn allocate(spec: &SubnetSpec, used: &BTreeSet<IpAddr>) -> ApiResult<IpAddr> {
    for pool in pools(spec) {
        let mut cur = Some(pool.start);
        let end = ip_to_u128(pool.end);
        while let Some(ip) = cur {
            if ip_to_u128(ip) > end {
                break;
            }
            if !is_reserved(spec, ip) && !used.contains(&ip) {
                return Ok(ip);
            }
            cur = next_ip(ip);
        }
    }
    Err(ApiError::conflict("subnet full").with_detail("cidr", spec.cidr.to_string()))
}

/// Check that a caller-chosen address is usable in this subnet.
pub fn validate_fixed(spec: &SubnetSpec, ip: IpAddr, used: &BTreeSet<IpAddr>) -> ApiResult<()> {
    if !spec.cidr.contains(&ip) {
        return Err(ApiError::bad_request(format!(
            "{ip} is outside {}",
            spec.cidr
        )));
    }
    if is_reserved(spec, ip) {
        return Err(ApiError::bad_request(format!(
            "{ip} is reserved in {}",
            spec.cidr
        )));
    }
    if used.contains(&ip) {
        return Err(ApiError::conflict(format!("{ip} is already in use"))
            .with_detail("address", ip.to_string()));
    }
    Ok(())
}

/// Total allocatable addresses in the pools (reserved ones excluded).
pub fn capacity(spec: &SubnetSpec) -> u128 {
    pools(spec)
        .iter()
        .map(|p| {
            let s = ip_to_u128(p.start);
            let e = ip_to_u128(p.end);
            let mut n = e.saturating_sub(s) + 1;
            for r in [
                Some(spec.cidr.network()),
                spec.gateway,
                broadcast(spec.cidr),
            ]
            .into_iter()
            .flatten()
            {
                let x = ip_to_u128(r);
                if x >= s && x <= e {
                    n = n.saturating_sub(1);
                }
            }
            n
        })
        .sum()
}

fn broadcast(cidr: IpNet) -> Option<IpAddr> {
    match cidr {
        IpNet::V4(n) => Some(IpAddr::V4(n.broadcast())),
        IpNet::V6(_) => None,
    }
}

/// Render a MAC as `aa:bb:cc:dd:ee:ff`.
pub fn format_mac(b: [u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5]
    )
}

/// Parse `aa:bb:cc:dd:ee:ff` (or `-` separated).
pub fn parse_mac(s: &str) -> ApiResult<[u8; 6]> {
    let parts: Vec<&str> = s.split([':', '-']).collect();
    if parts.len() != 6 {
        return Err(ApiError::bad_request(format!("invalid mac '{s}'")));
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16)
            .map_err(|_| ApiError::bad_request(format!("invalid mac '{s}'")))?;
    }
    if out[0] & 0x01 != 0 {
        return Err(ApiError::bad_request(format!(
            "mac '{s}' is a multicast address"
        )));
    }
    Ok(out)
}

/// Deterministically derive a MAC under [`OUI`] from a seed (the port id),
/// stepping past any address in `taken`. The seed makes it reproducible
/// in tests; the collision check makes it unique on a network.
pub fn generate_mac(seed: &Uuid, taken: &BTreeSet<String>) -> ApiResult<String> {
    let bytes = seed.as_bytes();
    let base = u32::from_be_bytes([0, bytes[13], bytes[14], bytes[15]]);
    for step in 0..0x1_0000u32 {
        let nic = (base.wrapping_add(step)) & 0x00ff_ffff;
        let [_, a, b, c] = nic.to_be_bytes();
        let mac = format_mac([OUI[0], OUI[1], OUI[2], a, b, c]);
        if !taken.contains(&mac) {
            return Ok(mac);
        }
    }
    Err(ApiError::conflict("no free mac address"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::{kind, Id};

    fn spec(cidr: &str, gw: Option<&str>) -> SubnetSpec {
        SubnetSpec {
            network: Id::new(kind::NETWORK),
            cidr: cidr.parse().unwrap(),
            gateway: gw.map(|g| g.parse().unwrap()),
            allocation_pools: vec![],
            dns_nameservers: vec![],
            enable_dhcp: true,
            host_routes: vec![],
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn skips_network_gateway_and_used() {
        let s = spec("10.0.0.0/24", Some("10.0.0.1"));
        let mut used = BTreeSet::new();
        assert_eq!(allocate(&s, &used).unwrap(), ip("10.0.0.2"));
        used.insert(ip("10.0.0.2"));
        used.insert(ip("10.0.0.3"));
        assert_eq!(allocate(&s, &used).unwrap(), ip("10.0.0.4"));
    }

    #[test]
    fn allocation_is_deterministic() {
        let s = spec("192.168.7.0/28", Some("192.168.7.1"));
        let used = BTreeSet::from([ip("192.168.7.2")]);
        let a = allocate(&s, &used).unwrap();
        let b = allocate(&s, &used).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, ip("192.168.7.3"));
    }

    #[test]
    fn respects_pools() {
        let mut s = spec("10.1.0.0/24", Some("10.1.0.1"));
        s.allocation_pools = vec![
            AllocationPool {
                start: ip("10.1.0.100"),
                end: ip("10.1.0.101"),
            },
            AllocationPool {
                start: ip("10.1.0.200"),
                end: ip("10.1.0.200"),
            },
        ];
        let mut used = BTreeSet::new();
        for want in ["10.1.0.100", "10.1.0.101", "10.1.0.200"] {
            let got = allocate(&s, &used).unwrap();
            assert_eq!(got, ip(want));
            used.insert(got);
        }
        let err = allocate(&s, &used).unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Conflict);
        assert_eq!(err.message, "subnet full");
    }

    #[test]
    fn exhaustion_is_conflict() {
        // /30: .0 network, .1 gateway, .2 usable, .3 broadcast.
        let s = spec("10.2.0.0/30", Some("10.2.0.1"));
        assert_eq!(capacity(&s), 1);
        let mut used = BTreeSet::new();
        let a = allocate(&s, &used).unwrap();
        assert_eq!(a, ip("10.2.0.2"));
        used.insert(a);
        let err = allocate(&s, &used).unwrap_err();
        assert_eq!(
            err,
            ApiError::conflict("subnet full").with_detail("cidr", "10.2.0.0/30")
        );
    }

    #[test]
    fn ipv6_allocation() {
        let s = spec("fd00::/64", Some("fd00::1"));
        let used = BTreeSet::from([ip("fd00::2")]);
        assert_eq!(allocate(&s, &used).unwrap(), ip("fd00::3"));
    }

    #[test]
    fn fixed_ip_validation() {
        let s = spec("10.0.0.0/24", Some("10.0.0.1"));
        let used = BTreeSet::from([ip("10.0.0.5")]);
        assert!(validate_fixed(&s, ip("10.0.0.6"), &used).is_ok());
        assert!(validate_fixed(&s, ip("10.0.0.1"), &used).is_err());
        assert!(validate_fixed(&s, ip("10.0.0.255"), &used).is_err());
        assert!(validate_fixed(&s, ip("10.0.1.6"), &used).is_err());
        assert_eq!(
            validate_fixed(&s, ip("10.0.0.5"), &used).unwrap_err().code,
            pistack_types::error::ErrorCode::Conflict
        );
    }

    #[test]
    fn subnet_validation() {
        let mut s = spec("10.0.0.0/24", Some("10.0.1.1"));
        assert!(validate_subnet(&s).is_err());
        s.gateway = Some(ip("10.0.0.1"));
        s.allocation_pools = vec![AllocationPool {
            start: ip("10.0.0.50"),
            end: ip("10.0.0.20"),
        }];
        assert!(validate_subnet(&s).is_err());
        s.allocation_pools = vec![AllocationPool {
            start: ip("10.0.0.20"),
            end: ip("10.0.0.50"),
        }];
        assert!(validate_subnet(&s).is_ok());
    }

    #[test]
    fn mac_generation() {
        let seed = Uuid::now_v7();
        let taken = BTreeSet::new();
        let m1 = generate_mac(&seed, &taken).unwrap();
        let m2 = generate_mac(&seed, &taken).unwrap();
        assert_eq!(m1, m2, "same seed, same mac");
        assert!(m1.starts_with("02:70:69:"));
        assert_eq!(parse_mac(&m1).unwrap()[..3], OUI);
        let taken = BTreeSet::from([m1.clone()]);
        let m3 = generate_mac(&seed, &taken).unwrap();
        assert_ne!(m1, m3);
        assert!(m3.starts_with("02:70:69:"));
    }

    #[test]
    fn mac_parsing() {
        assert!(parse_mac("02:70:69:00:00:01").is_ok());
        assert!(parse_mac("02-70-69-00-00-01").is_ok());
        assert!(parse_mac("02:70:69:00:00").is_err());
        assert!(
            parse_mac("01:00:5e:00:00:01").is_err(),
            "multicast rejected"
        );
        assert!(parse_mac("zz:70:69:00:00:01").is_err());
    }
}
