// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{AllocationPool, AllowedAddressPair, SubnetSpec};
use ipnet::IpNet;
use pistack_types::ApiError;
use std::collections::BTreeSet;
use std::net::IpAddr;
use uuid::Uuid;

use super::*;
use pistack_types::{kind, Id};

/// A pair's address as the API reads it: a CIDR, or one address given its
/// host prefix.
fn net(s: &str) -> IpNet {
    s.parse::<IpNet>()
        .or_else(|_| s.parse::<IpAddr>().map(IpNet::from))
        .unwrap()
}

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

fn pair(ip: &str, mac: Option<&str>) -> AllowedAddressPair {
    AllowedAddressPair {
        ip_address: net(ip),
        mac_address: mac.map(str::to_string),
    }
}

#[test]
fn pair_validation() {
    assert!(validate_pairs(&[]).is_ok());
    assert!(validate_pairs(&[pair("10.0.0.9", None)]).is_ok());
    assert!(validate_pairs(&[pair("10.0.0.0/28", Some("02:00:00:00:00:09"))]).is_ok());
    assert!(validate_pairs(&[pair("fd00::9", None), pair("fd00::/64", None)]).is_ok());
    assert!(
        validate_pairs(&[pair("0.0.0.0/0", None)]).is_err(),
        "every v4 address"
    );
    assert!(
        validate_pairs(&[pair("::/0", None)]).is_err(),
        "every v6 address"
    );
    assert!(
        validate_pairs(&[pair("10.0.0.9", Some("nope"))]).is_err(),
        "mac must parse"
    );
    assert!(
        validate_pairs(&[pair("10.0.0.9", Some("01:00:5e:00:00:01"))]).is_err(),
        "multicast mac"
    );
    assert!(validate_pairs(&[pair("10.0.0.9", None), pair("10.0.0.9", None)]).is_err());
    assert!(
        validate_pairs(&[
            pair("10.0.0.9", None),
            pair("10.0.0.9", Some("02:00:00:00:00:09")),
        ])
        .is_ok(),
        "same address, different mac"
    );
    // `-` separated is the same mac written another way.
    assert!(validate_pairs(&[
        pair("10.0.0.9", Some("02:00:00:00:00:09")),
        pair("10.0.0.9", Some("02-00-00-00-00-09")),
    ])
    .is_err());
    let ten = (0..10).map(|i| pair(&format!("10.0.0.{i}"), None));
    assert!(validate_pairs(&ten.collect::<Vec<_>>()).is_ok());
    let eleven = (0..11).map(|i| pair(&format!("10.0.0.{i}"), None));
    assert!(validate_pairs(&eleven.collect::<Vec<_>>()).is_err());
}

#[test]
fn a_bare_address_and_its_host_prefix_are_one_pair() {
    assert!(validate_pairs(&[pair("10.0.0.9", None), pair("10.0.0.9/32", None)]).is_err());
    assert!(validate_pairs(&[pair("fd00::9", None), pair("fd00::9/128", None)]).is_err());
}
