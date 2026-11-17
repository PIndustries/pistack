// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Picker tests: the algorithm names a pool spec accepts, the shares each
//! algorithm pays out, and the members that never become a choice.

use super::backend::*;
use super::eligible::*;
use super::hash_ip::hash_ip;
use super::select::*;
use crate::types::{kinds, Algorithm, Member, MemberSpec, MemberStatus, OperatingStatus, PoolSpec};
use pistack_types::{Id, Meta};
use std::net::{IpAddr, SocketAddr};

fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn backend(port: u16, weight: u16) -> Backend {
    Backend {
        addr: addr(port),
        weight,
        conns: 0,
    }
}

fn counted(port: u16, weight: u16, conns: u64) -> Backend {
    Backend {
        addr: addr(port),
        weight,
        conns,
    }
}

fn member(port: u16, weight: u16, up: OperatingStatus) -> Member {
    Member {
        meta: Meta::new(kinds::MEMBER, "m", None),
        spec: MemberSpec {
            pool: Id::new(kinds::POOL),
            address: addr(port).ip(),
            port,
            weight,
            backup: false,
            admin_up: true,
        },
        status: MemberStatus {
            operating_status: up,
            ..MemberStatus::default()
        },
    }
}

fn ip(last: u8) -> IpAddr {
    IpAddr::from([10, 0, 0, last])
}

/// Every choice over `n` picks, as ports.
fn spin(p: &Picker, backends: &[Backend], n: usize, client: Option<IpAddr>) -> Vec<u16> {
    (0..n)
        .map(|_| p.pick(backends, client).map(|a| a.port()).unwrap())
        .collect()
}

fn tally(ports: &[u16]) -> Vec<(u16, usize)> {
    let mut out: Vec<(u16, usize)> = Vec::new();
    for p in ports {
        match out.iter_mut().find(|(k, _)| k == p) {
            Some((_, n)) => *n += 1,
            None => out.push((*p, 1)),
        }
    }
    out.sort();
    out
}

/// A pool body carrying `raw` as its algorithm. `None` leaves the field
/// out, which is how a pool written before algorithms existed reads.
fn body(raw: Option<&str>) -> String {
    let lb = format!("lb-{}", uuid::Uuid::nil());
    match raw {
        Some(v) => {
            format!(r#"{{"balancer":"{lb}","protocol":"tcp","algorithm":{v},"admin_up":true}}"#)
        }
        None => format!(r#"{{"balancer":"{lb}","protocol":"tcp","admin_up":true}}"#),
    }
}

fn accepted(raw: &str) -> Algorithm {
    serde_json::from_str::<PoolSpec>(&body(Some(raw)))
        .unwrap()
        .algorithm
}

fn refused(raw: &str) -> String {
    match serde_json::from_str::<PoolSpec>(&body(Some(raw))) {
        Ok(spec) => panic!("accepted {:?}", spec.algorithm),
        Err(e) => e.to_string(),
    }
}

#[test]
fn an_unknown_algorithm_is_refused_with_the_names() {
    assert_eq!(Algorithm::parse("round_robin"), Ok(Algorithm::RoundRobin));
    assert_eq!(Algorithm::parse(" least_conn "), Ok(Algorithm::LeastConn));
    assert_eq!(Algorithm::parse("source_ip"), Ok(Algorithm::SourceIp));
    let err = Algorithm::parse("weighted").unwrap_err();
    assert!(err.contains("weighted"), "{err}");
    for name in Algorithm::NAMES {
        assert!(err.contains(name), "{err}");
    }
    assert_eq!(accepted("\"least_conn\""), Algorithm::LeastConn);
    assert_eq!(accepted("\"source_ip\""), Algorithm::SourceIp);
    let msg = refused("\"hash\"");
    assert!(msg.contains("hash"), "{msg}");
    for name in Algorithm::NAMES {
        assert!(msg.contains(name), "{msg}");
    }
    let msg = refused("\"Round_Robin\"");
    assert!(msg.contains("Round_Robin"), "{msg}");
    assert!(msg.contains("round_robin"), "{msg}");
    let spec: PoolSpec = serde_json::from_str(&body(None))
        .unwrap_or_else(|e| panic!("an absent algorithm must read as the default: {e}"));
    assert_eq!(spec.algorithm, Algorithm::RoundRobin);

    assert_eq!(
        serde_json::to_string(&Algorithm::LeastConn).unwrap(),
        "\"least_conn\""
    );
}

#[test]
fn round_robin_shares_by_weight() {
    // A weight 2 member holds two slots in the walk, so it takes half
    // the traffic; the round pays out one connection at a time.
    let backends = [backend(1, 2), backend(2, 1), backend(3, 1)];
    let p = Picker::new(Algorithm::RoundRobin);
    assert_eq!(spin(&p, &backends, 8, None), vec![1, 2, 3, 1, 1, 2, 3, 1]);
    let only = Picker::new(Algorithm::RoundRobin);
    assert_eq!(spin(&only, &[backend(1, 5)], 3, None), vec![1, 1, 1]);
}

#[test]
fn round_robin_gives_a_zero_weight_member_nothing() {
    let backends = [backend(1, 2), backend(2, 0), backend(3, 1)];
    let p = Picker::new(Algorithm::RoundRobin);
    assert_eq!(spin(&p, &backends, 6, None), vec![1, 3, 1, 1, 3, 1]);
    assert_eq!(p.pick(&[backend(2, 0)], None), None);
}

#[test]
fn round_robin_copes_with_a_member_leaving() {
    let all = [backend(1, 1), backend(2, 1), backend(3, 1)];
    let p = Picker::new(Algorithm::RoundRobin);
    assert_eq!(spin(&p, &all, 3, None), vec![1, 2, 3]);
    // The member that left was the last one dialed. The walk starts at
    // the top of the shorter list rather than dialing it twice, then
    // settles straight back into the rotation.
    let two = [backend(1, 1), backend(2, 1)];
    assert_eq!(spin(&p, &two, 4, None), vec![1, 2, 1, 2]);
    assert_eq!(spin(&p, &all, 3, None), vec![1, 2, 3]);
}

#[test]
fn least_conn_takes_the_least_loaded() {
    let backends = [
        counted(1, 1, 3),
        counted(2, 1, 1),
        counted(3, 1, 2),
        counted(4, 1, 0),
    ];
    let p = Picker::new(Algorithm::LeastConn);
    assert_eq!(spin(&p, &backends, 1, None), vec![4]);
}

#[test]
fn least_conn_breaks_a_tie_by_weight_then_order() {
    let p = Picker::new(Algorithm::LeastConn);
    let tied = [counted(1, 1, 2), counted(2, 5, 2), counted(3, 5, 2)];
    assert_eq!(spin(&p, &tied, 1, None), vec![2], "heavier first");
    let ordered = [counted(7, 1, 2), counted(4, 1, 2), counted(9, 1, 2)];
    assert_eq!(spin(&p, &ordered, 1, None), vec![7], "earlier first");
}

#[test]
fn least_conn_gives_a_zero_weight_member_nothing() {
    let backends = [counted(1, 9, 9), counted(2, 0, 0)];
    let p = Picker::new(Algorithm::LeastConn);
    assert_eq!(spin(&p, &backends, 4, None), vec![1; 4]);
    assert_eq!(p.pick(&[counted(1, 0, 0)], None), None);
    assert!(p.pick(&[], None).is_none());
}

#[test]
fn source_ip_pins_a_client_while_the_set_holds_still() {
    let backends = [backend(1, 1), backend(2, 1), backend(3, 1)];
    let p = Picker::new(Algorithm::SourceIp);
    let clients: Vec<IpAddr> = (11..=40).map(ip).collect();
    for ip in &clients {
        let first = spin(&p, &backends, 1, Some(*ip));
        assert_eq!(spin(&p, &backends, 6, Some(*ip)), vec![first[0]; 6]);
    }
    let seen: Vec<u16> = clients
        .iter()
        .map(|c| spin(&p, &backends, 1, Some(*c))[0])
        .collect();
    let spread = tally(&seen);
    assert_eq!(spread.len(), 3, "every member takes a client: {spread:?}");
}

#[test]
fn source_ip_moves_only_clients_on_a_gone_member() {
    let three = [backend(1, 1), backend(2, 1), backend(3, 1)];
    let p = Picker::new(Algorithm::SourceIp);
    let clients: Vec<IpAddr> = (11..=40).map(ip).collect();
    let before: Vec<u16> = clients
        .iter()
        .map(|c| spin(&p, &three, 1, Some(*c))[0])
        .collect();
    let two = [backend(1, 1), backend(2, 1)];
    let after: Vec<u16> = clients
        .iter()
        .map(|c| spin(&p, &two, 1, Some(*c))[0])
        .collect();
    let moved: Vec<usize> = before
        .iter()
        .zip(&after)
        .enumerate()
        .filter(|(_, (was, now))| was != now)
        .map(|(i, _)| i)
        .collect();
    assert!(
        moved.iter().all(|i| before[*i] == 3),
        "only clients on the gone member moved: {moved:?}"
    );
    assert!(
        after.iter().all(|p| *p != 3),
        "the gone member takes nothing"
    );
    for ip in &clients {
        let again = spin(&p, &two, 1, Some(*ip))[0];
        assert_eq!(spin(&p, &two, 3, Some(*ip))[0], again);
    }
}

#[test]
fn source_ip_gives_a_zero_weight_member_nothing() {
    let backends = [backend(1, 1), backend(2, 0)];
    let p = Picker::new(Algorithm::SourceIp);
    let seen: Vec<u16> = (11..=30)
        .map(|i| spin(&p, &backends, 1, Some(ip(i)))[0])
        .collect();
    assert!(seen.iter().all(|p| *p == 1), "{seen:?}");
}

#[test]
fn a_member_no_one_dials_is_never_picked() {
    for algo in [
        Algorithm::RoundRobin,
        Algorithm::LeastConn,
        Algorithm::SourceIp,
    ] {
        let p = Picker::new(algo);
        assert_eq!(
            p.pick(&[backend(1, 0), backend(2, 0)], Some(ip(11))),
            None,
            "{algo}"
        );
        assert_eq!(p.pick(&[], Some(ip(11))), None, "{algo}");
    }
}

#[test]
fn source_ip_without_a_peer_address_goes_round_robin() {
    let backends = [backend(1, 1), backend(2, 1)];
    let p = Picker::new(Algorithm::SourceIp);
    assert_eq!(spin(&p, &backends, 4, None), vec![1, 2, 1, 2]);
}

#[test]
fn only_healthy_weighted_members_are_eligible() {
    let members = [
        member(1, 1, OperatingStatus::Online),
        member(2, 1, OperatingStatus::NoMonitor),
        member(3, 1, OperatingStatus::Offline),
        member(4, 1, OperatingStatus::Draining),
        member(5, 0, OperatingStatus::Online),
    ];
    assert_eq!(
        dialable(&members),
        vec![addr(1), addr(2)],
        "offline, draining and zero-weight members take nothing"
    );
    assert_eq!(Backend::of(&members[0]), backend(1, 1));
    assert!(eligible(&members[..3])
        .iter()
        .all(|m| m.status.operating_status != OperatingStatus::Offline));
}

#[test]
fn the_ip_hash_is_stable_and_reaches_every_slot() {
    let a: IpAddr = "10.0.0.11".parse().unwrap();
    assert_eq!(hash_ip(a), hash_ip("10.0.0.11".parse().unwrap()));
    assert_ne!(hash_ip(a), hash_ip("10.0.0.12".parse().unwrap()));
    assert_ne!(
        hash_ip("::ffff:10.0.0.11".parse().unwrap()),
        hash_ip("::1".parse().unwrap())
    );
    let slots: std::collections::BTreeSet<u64> = (1..=200u8).map(|i| hash_ip(ip(i)) % 3).collect();
    assert_eq!(slots.len(), 3);
}

#[test]
fn a_heavier_member_takes_the_share_named_for_it() {
    let backends = [backend(1, 4), backend(2, 1)];
    let p = Picker::new(Algorithm::RoundRobin);
    let mut got = std::collections::BTreeMap::new();
    for _ in 0..1000 {
        *got.entry(p.pick(&backends, None).unwrap().port())
            .or_insert(0) += 1;
    }
    assert_eq!(got[&1], 800, "{got:?}");
    assert_eq!(got[&2], 200, "{got:?}");
    let even = [backend(1, 3), backend(2, 3), backend(3, 3)];
    let p = Picker::new(Algorithm::RoundRobin);
    let mut got = std::collections::BTreeMap::new();
    for _ in 0..900 {
        *got.entry(p.pick(&even, None).unwrap().port()).or_insert(0) += 1;
    }
    for port in [1, 2, 3] {
        assert_eq!(got[&port], 300, "{got:?}");
    }
}

#[test]
fn a_picker_shared_between_connections_keeps_one_rotation() {
    let backends = [backend(1, 1), backend(2, 1), backend(3, 1)];
    let p = Picker::new(Algorithm::RoundRobin);
    let a = p.clone();
    let b = p.clone();
    let mut seen = Vec::new();
    for handle in [&a, &b, &a, &b, &a, &b] {
        seen.push(handle.pick(&backends, None).unwrap().port());
    }
    assert_eq!(seen, vec![1, 2, 3, 1, 2, 3]);
    let p = Picker::new(Algorithm::SourceIp);
    let a = p.clone();
    let first = p.pick(&backends, Some(ip(11))).unwrap();
    assert_eq!(a.pick(&backends, Some(ip(11))).unwrap(), first);
}
