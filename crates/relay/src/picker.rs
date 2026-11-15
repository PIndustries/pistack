// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Member selection. Both data planes ask here so a pool balances the
//! same way over TCP and over HTTP.
//!
//! The unit is a [`Backend`]: the address to dial, its weight, and how
//! many connections are open on it. Everything is keyed by address, so a
//! member list in a new order keeps its place rather than reshuffling the
//! traffic.
//!
//! * `round_robin` visits the backends in proportion to their weight, so
//!   a backend of weight 2 takes twice the share.
//! * `least_conn` takes the backend with the fewest open connections,
//!   breaking a tie by weight and then by order.
//! * `source_ip` maps a client address to one backend and keeps it there
//!   while the set of backends is unchanged.
//!
//! A backend with weight 0 takes no new connections, so it never appears
//! as a choice. Members that are not healthy never reach a picker at all:
//! the caller hands over only what may take traffic.

use crate::types::{Algorithm, Member, OperatingStatus};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// One dialable backend with what the picker needs to weigh it.
///
/// Built from a member with [`Backend::of`] so the weight and the health
/// rules cannot be forgotten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backend {
    pub addr: SocketAddr,
    pub weight: u16,
    /// Connections open on this backend right now. Only `least_conn` uses
    /// it; 0 is right when the caller is not counting.
    pub conns: u64,
}

impl Backend {
    /// A backend the picker may weigh by hand.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            weight: 1,
            conns: 0,
        }
    }

    /// The backend for `member`: its address and weight, and no
    /// connections until something opens one.
    pub fn of(member: &Member) -> Self {
        Self {
            addr: member.addr(),
            weight: member.spec.weight,
            conns: 0,
        }
    }
}

/// Members that may take traffic: administratively up, weighted above
/// zero, and `Online` or unmonitored. Under every algorithm a member
/// outside that is never a choice, so a data plane that dials addresses
/// rather than members starts from this list too.
pub fn eligible(members: &[Member]) -> Vec<&Member> {
    members
        .iter()
        .filter(|m| {
            m.spec.admin_up
                && m.spec.weight > 0
                && matches!(
                    m.status.operating_status,
                    OperatingStatus::Online | OperatingStatus::NoMonitor
                )
        })
        .collect()
}

/// The addresses of the members that may take traffic, in the same order.
pub fn dialable(members: &[Member]) -> Vec<SocketAddr> {
    eligible(members).into_iter().map(|m| m.addr()).collect()
}

/// Member selection for one pool. Cloning one hands the same rotation and
/// the same pins to the copy, so a data plane can give every connection
/// its own handle and still balance across all of them.
#[derive(Debug, Default)]
pub struct Picker {
    algo: Arc<std::sync::atomic::AtomicU8>,
    state: Arc<parking_lot::Mutex<Rotation>>,
    source: Arc<parking_lot::Mutex<Sticky>>,
}

impl Clone for Picker {
    fn clone(&self) -> Self {
        Self {
            algo: self.algo.clone(),
            state: self.state.clone(),
            source: self.source.clone(),
        }
    }
}

/// What each backend is still owed, in pool order.
#[derive(Debug, Default)]
struct Rotation {
    /// What each backend is still owed, in pool order.
    due: Vec<(SocketAddr, i64)>,
}

/// `source_ip`'s state: the backend set the table was built for, and the
/// address each seen client is pinned to.
#[derive(Debug, Default)]
struct Sticky {
    ring: Vec<SocketAddr>,
    pinned: Vec<(IpAddr, SocketAddr)>,
}

/// Kept private to this module so a bad name cannot reach [`Picker::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Algo {
    RoundRobin,
    LeastConn,
    SourceIp,
}

impl std::fmt::Display for Algorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl Algorithm {
    /// The wire name.
    pub fn name(self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::LeastConn => "least_conn",
            Self::SourceIp => "source_ip",
        }
    }

    /// Every name a pool spec may carry.
    pub const NAMES: [&'static str; 3] = ["round_robin", "least_conn", "source_ip"];

    /// Refused at create and update when a pool spec carries anything else.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "round_robin" => Ok(Self::RoundRobin),
            "least_conn" => Ok(Self::LeastConn),
            "source_ip" => Ok(Self::SourceIp),
            other => Err(format!(
                "algorithm {other:?} is not a balancing algorithm; use one of {}",
                Self::NAMES.join(", ")
            )),
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::RoundRobin => 0,
            Self::LeastConn => 1,
            Self::SourceIp => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::LeastConn,
            2 => Self::SourceIp,
            _ => Self::RoundRobin,
        }
    }

    fn algo(self) -> Algo {
        match self {
            Self::RoundRobin => Algo::RoundRobin,
            Self::LeastConn => Algo::LeastConn,
            Self::SourceIp => Algo::SourceIp,
        }
    }
}

/// A [`serde`] module so a pool spec carrying an unknown algorithm is
/// refused where it is read, with the names it may hold instead.
pub mod wire {
    use super::Algorithm;
    use serde::de::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(d: D) -> Result<Algorithm, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(d)?;
        Algorithm::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl Picker {
    pub fn new(algo: Algorithm) -> Self {
        Self {
            algo: Arc::new(std::sync::atomic::AtomicU8::new(algo.code())),
            state: Arc::new(parking_lot::Mutex::new(Rotation::default())),
            source: Arc::new(parking_lot::Mutex::new(Sticky::default())),
        }
    }

    pub fn algorithm(&self) -> Algorithm {
        Algorithm::from_code(self.algo.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Switch algorithm, keeping what the current one has accumulated. A
    /// pool re-pointed at another algorithm starts counting from where it
    /// stands rather than from the top of a rotation.
    pub fn set_algorithm(&self, algo: Algorithm) {
        self.algo
            .store(algo.code(), std::sync::atomic::Ordering::Relaxed);
    }

    /// The backend this request goes to, `None` when nothing may take it.
    /// `client` is the peer address; only `source_ip` needs it, and
    /// without one the request goes round robin.
    pub fn pick(&self, backends: &[Backend], client: Option<IpAddr>) -> Option<SocketAddr> {
        let live: Vec<&Backend> = backends.iter().filter(|b| b.weight > 0).collect();
        if live.is_empty() {
            return None;
        }
        match self.algorithm().algo() {
            Algo::LeastConn => least_conn(&live),
            Algo::SourceIp => match client {
                Some(ip) => self.sticky(ip, &live),
                None => self.round_robin(&live),
            },
            Algo::RoundRobin => self.round_robin(&live),
        }
    }

    /// Smooth weighted round robin: each round every backend is credited
    /// its weight, and the one owing the most takes the next connection
    /// and pays a round back. A backend of weight 2 is back after one turn
    /// by the others and never twice in a row, and the shares come out
    /// exactly. What is still owed when a member joins or leaves goes with
    /// it, so one change moves one connection and no more.
    fn round_robin(&self, live: &[&Backend]) -> Option<SocketAddr> {
        let total: u32 = live.iter().map(|b| u32::from(b.weight)).sum();
        if total == 0 {
            return None;
        }
        let mut state = self.state.lock();
        state
            .due
            .retain(|(addr, _)| live.iter().any(|b| b.addr == *addr && b.weight > 0));
        for b in live {
            if !state.due.iter().any(|(addr, _)| *addr == b.addr) {
                state.due.push((b.addr, 0));
            }
        }
        for (addr, owed) in state.due.iter_mut() {
            *owed += live
                .iter()
                .find(|b| b.addr == *addr)
                .map_or(0, |b| i64::from(b.weight));
        }
        let mut at = 0;
        for (i, (_, owed)) in state.due.iter().enumerate() {
            if owed > &state.due[at].1 {
                at = i;
            }
        }
        state.due[at].1 -= i64::from(total);
        let chosen = state.due[at].0;
        Some(chosen)
    }

    /// The client's address picks a slot in the backend set, so a set
    /// that loses a member moves as few clients as possible. The pin is
    /// remembered while the set holds still, so a client whose backend
    /// was passed over by the hash still lands on it every time.
    fn sticky(&self, client: IpAddr, live: &[&Backend]) -> Option<SocketAddr> {
        let mut ring: Vec<SocketAddr> = live.iter().map(|b| b.addr).collect();
        ring.sort_unstable();
        let mut state = self.source.lock();
        if state.ring != ring {
            state.pinned.retain(|(_, addr)| ring.contains(addr));
            state.ring.clone_from(&ring);
        }
        if let Some((_, addr)) = state.pinned.iter().find(|(ip, _)| *ip == client) {
            return Some(*addr);
        }
        let addr = state.ring[(hash_ip(client) as usize) % state.ring.len()];
        if state.pinned.len() < PINNED_CAP {
            state.pinned.push((client, addr));
        }
        Some(addr)
    }
}

/// How many client addresses `source_ip` remembers. Past this, a client
/// is placed by its hash again rather than by its history.
const PINNED_CAP: usize = 4096;

/// FNV-1a over the address bytes. Fixed so a restart, or another server
/// in the cluster, pins the same client to the same slot.
fn hash_ip(ip: IpAddr) -> u64 {
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

/// Fewest open connections; a tie goes to the heavier backend, and one
/// between equals goes to the earlier of the two.
fn least_conn(live: &[&Backend]) -> Option<SocketAddr> {
    let mut best: Option<&Backend> = None;
    for b in live {
        let better = match best {
            None => true,
            Some(have) => b.conns < have.conns || (b.conns == have.conns && b.weight > have.weight),
        };
        if better {
            best = Some(b);
        }
    }
    best.map(|b| b.addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{kinds, MemberSpec, MemberStatus, PoolSpec};
    use pistack_types::{Id, Meta};

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
        let slots: std::collections::BTreeSet<u64> =
            (1..=200u8).map(|i| hash_ip(ip(i)) % 3).collect();
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
}
