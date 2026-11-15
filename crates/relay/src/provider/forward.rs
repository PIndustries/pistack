// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! In-process L4 forwarder. One TCP socket per balancer, and a UDP socket
//! when the listener is UDP.
//!
//! `ensure_balancer` binds `127.0.0.1:0` unless `spec.vip_address` is
//! loopback or private, in which case it binds that address on an ephemeral
//! port. Unspecified, public, multicast, broadcast, link-local, and
//! 100.64/10 addresses are refused.
//! `provider_ref` is the bound socket, `127.0.0.1:PORT` when the bind
//! address is loopback. The TCP accept loop picks a member under the
//! pool's algorithm and copies both directions. An empty TCP member list,
//! or a member that does not accept, is accepted and closed. UDP copies
//! datagrams to an up member and is not ready when that list is empty. A
//! later `ensure_listener` replaces the member list without rebinding.
//! `delete_listener` and `delete_balancer` stop the task and drop the
//! socket.

use super::{LbProvider, ListenerView, Realized};
use crate::picker::{Backend, Picker};
use crate::types::{Algorithm, Balancer, ListenerProtocol, Member, Provider};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// What the forwarder dials: the members that may take traffic, with their
/// weights, plus the connections open on each so `least_conn` can see them.
#[derive(Clone)]
struct Dial {
    backends: Arc<parking_lot::Mutex<Vec<Backend>>>,
    picker: Picker,
    conns: Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
}

impl Dial {
    fn new(algo: Algorithm) -> Self {
        Self {
            backends: Arc::new(parking_lot::Mutex::new(Vec::new())),
            picker: Picker::new(algo),
            conns: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        }
    }

    /// Replace the member list. A member re-added at the same address
    /// starts counting again.
    fn set_members(&self, members: &[Member]) {
        *self.backends.lock() = members.iter().map(Backend::of).collect();
    }

    /// The backends to choose between, each with its live connection count.
    fn snapshot(&self) -> Vec<Backend> {
        let conns = self.conns.lock();
        self.backends
            .lock()
            .iter()
            .map(|b| Backend {
                conns: conns.get(&b.addr).copied().unwrap_or(0),
                ..*b
            })
            .collect()
    }

    fn open(&self, addr: SocketAddr) {
        *self.conns.lock().entry(addr).or_insert(0) += 1;
    }

    fn close(&self, addr: SocketAddr) {
        let mut conns = self.conns.lock();
        if let Some(n) = conns.get_mut(&addr) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                conns.remove(&addr);
            }
        }
    }
}

/// One balancer's socket and the task that serves it.
struct Slot {
    bound: SocketAddr,
    /// Held until the accept task starts, then `None`.
    listener: Option<TcpListener>,
    task: Option<JoinHandle<()>>,
    cancel: tokio_util::sync::CancellationToken,
    /// Who the accept loop dials and what it owes them. Replaced by
    /// `ensure_listener`, read by every connection it accepts.
    dial: Arc<parking_lot::Mutex<Arc<Dial>>>,
    udp: Option<crate::udp::UdpForward>,
}

/// Local TCP forwarder. Serves [`Provider::Forward`] (L4). HTTP stays on
/// [`super::RelayL7Provider`].
pub struct ForwardProvider {
    slots: parking_lot::Mutex<HashMap<Id, Slot>>,
}

impl Default for ForwardProvider {
    fn default() -> Self {
        Self {
            slots: parking_lot::Mutex::new(HashMap::new()),
        }
    }
}

impl ForwardProvider {
    pub fn new() -> Self {
        Self::default()
    }

    async fn stop(&self, id: &Id) {
        let slot = self.slots.lock().remove(id);
        if let Some(mut slot) = slot {
            slot.cancel.cancel();
            if let Some(udp) = slot.udp.take() {
                udp.shutdown().await;
            }
            if let Some(task) = slot.task {
                let _ = task.await;
            }
        }
    }
}

impl Drop for ForwardProvider {
    fn drop(&mut self) {
        let slots = std::mem::take(&mut *self.slots.lock());
        for (_, slot) in slots {
            slot.cancel.cancel();
            if let Some(task) = slot.task {
                task.abort();
            }
        }
    }
}

fn vip_allowed(ip: IpAddr) -> bool {
    crate::bind::allowed(ip)
}

fn bind_target(lb: &Balancer) -> ApiResult<SocketAddr> {
    let ip = match lb.spec.vip_address {
        None => IpAddr::V4(Ipv4Addr::LOCALHOST),
        Some(ip) if vip_allowed(ip) => ip,
        Some(ip) => {
            return Err(ApiError::bad_request(format!(
                "forward refuses vip {ip}; only a loopback or private address can be bound"
            )));
        }
    };
    Ok(SocketAddr::new(ip, lb.spec.vip_port.unwrap_or(0)))
}

fn realized(bound: SocketAddr) -> Realized {
    Realized {
        vip: bound.ip(),
        provider_ref: bound.to_string(),
    }
}

/// The listener's pool and members as the forwarder dials them. Members
/// not fit to take traffic are already out of the view.
fn dial_for(view: &ListenerView) -> Arc<Dial> {
    let empty: &[Member] = &[];
    let (algo, members) = match &view.pool {
        Some((pool, members)) if pool.spec.admin_up => (pool.spec.algorithm, members.as_slice()),
        // No pool, or one administratively down: nothing to dial.
        _ => (Algorithm::default(), empty),
    };
    let dial = Arc::new(Dial::new(algo));
    dial.set_members(members);
    dial
}

async fn accept_loop(
    listener: TcpListener,
    dial: Arc<parking_lot::Mutex<Arc<Dial>>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        let dial = dial.lock().clone();
                        tokio::spawn(async move {
                            forward_one(stream, dial, peer.ip()).await;
                        });
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "forward accept ended");
                        break;
                    }
                }
            }
        }
    }
}

async fn forward_one(mut inbound: TcpStream, dial: Arc<Dial>, client: IpAddr) {
    let _ = inbound.set_nodelay(true);
    let Some(dest) = dial.picker.pick(&dial.snapshot(), Some(client)) else {
        return;
    };
    dial.open(dest);
    let outbound = TcpStream::connect(dest).await;
    let mut outbound = match outbound {
        Ok(stream) => stream,
        Err(e) => {
            dial.close(dest);
            tracing::debug!(%dest, error = %e, "forward member is not up");
            return;
        }
    };
    let _ = outbound.set_nodelay(true);
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
    dial.close(dest);
}

#[async_trait]
impl LbProvider for ForwardProvider {
    fn name(&self) -> &'static str {
        "forward"
    }

    fn serves(&self, provider: Provider) -> bool {
        provider == Provider::Forward
    }

    async fn ensure_balancer(&self, lb: &Balancer) -> ApiResult<Realized> {
        if let Some(bound) = self.slots.lock().get(&lb.meta.id).map(|s| s.bound) {
            return Ok(realized(bound));
        }
        let target = bind_target(lb)?;
        // A fixed port is bound by the listener that uses it, so a udp
        // listener never holds the tcp port (or the other way round).
        let (listener, bound) = if target.port() != 0 {
            (None, target)
        } else {
            let listener = TcpListener::bind(target)
                .await
                .map_err(|e| ApiError::unavailable(format!("forward bind {target}: {e}")))?;
            let bound = listener
                .local_addr()
                .map_err(|e| ApiError::unavailable(format!("forward local address: {e}")))?;
            (Some(listener), bound)
        };
        let mut slots = self.slots.lock();
        if let Some(existing) = slots.get(&lb.meta.id) {
            return Ok(realized(existing.bound));
        }
        slots.insert(
            lb.meta.id.clone(),
            Slot {
                bound,
                listener,
                task: None,
                cancel: tokio_util::sync::CancellationToken::new(),
                dial: Arc::new(parking_lot::Mutex::new(Arc::new(Dial::new(
                    Algorithm::default(),
                )))),
                udp: None,
            },
        );
        Ok(realized(bound))
    }

    async fn delete_balancer(&self, id: &Id) -> ApiResult<()> {
        self.stop(id).await;
        Ok(())
    }

    async fn ensure_listener(&self, view: &ListenerView) -> ApiResult<String> {
        if matches!(view.listener.spec.protocol, ListenerProtocol::Udp) {
            return self.ensure_udp(view).await;
        }
        let dial = dial_for(view);
        let id = view.balancer.meta.id.clone();
        let unbound = {
            let slots = self.slots.lock();
            let slot = slots
                .get(&id)
                .ok_or_else(|| ApiError::bad_request("forward balancer is not bound"))?;
            (slot.task.is_none() && slot.listener.is_none()).then_some(slot.bound)
        };
        let fresh = match unbound {
            Some(at) => Some(
                TcpListener::bind(at)
                    .await
                    .map_err(|e| ApiError::unavailable(format!("forward bind {at}: {e}")))?,
            ),
            None => None,
        };
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&id)
            .ok_or_else(|| ApiError::bad_request("forward balancer is not bound"))?;
        *slot.dial.lock() = dial;
        if slot.task.is_none() && slot.listener.is_none() {
            slot.listener = fresh;
        }
        if slot.task.is_none() {
            let listener = slot
                .listener
                .take()
                .ok_or_else(|| ApiError::internal("forward socket is missing"))?;
            let dial = slot.dial.clone();
            let cancel = slot.cancel.clone();
            slot.task = Some(tokio::spawn(async move {
                accept_loop(listener, dial, cancel).await;
            }));
        }
        Ok(slot.bound.to_string())
    }

    async fn delete_listener(&self, balancer: &Id, _listener: &Id) -> ApiResult<()> {
        self.stop(balancer).await;
        Ok(())
    }
}

impl ForwardProvider {
    async fn ensure_udp(&self, view: &ListenerView) -> ApiResult<String> {
        let targets = crate::udp::targets(view);
        let id = view.balancer.meta.id.clone();
        if targets.is_empty() {
            if let Some(slot) = self.slots.lock().get(&id) {
                if let Some(udp) = &slot.udp {
                    udp.set_members(Vec::new());
                }
            }
            return Err(ApiError::unavailable("udp pool has no up member"));
        }
        let at = {
            let slots = self.slots.lock();
            let slot = slots
                .get(&id)
                .ok_or_else(|| ApiError::bad_request("forward balancer is not bound"))?;
            if let Some(udp) = &slot.udp {
                udp.set_members(targets);
                return Ok(udp.bound().to_string());
            }
            slot.bound
        };
        // Same address and port the balancer reports.
        let udp = crate::udp::UdpForward::bind(at, targets).await?;
        let bound = udp.bound().to_string();
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&id)
            .ok_or_else(|| ApiError::bad_request("forward balancer is not bound"))?;
        if let Some(existing) = &slot.udp {
            existing.set_members(crate::udp::targets(view));
            drop(udp);
            return Ok(existing.bound().to_string());
        }
        slot.udp = Some(udp);
        if slot.task.is_none() {
            // Free the tcp port held for an ephemeral balancer; a tcp
            // listener added later binds it again.
            slot.listener = None;
        }
        Ok(bound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ListenerView;
    use crate::types::*;
    use pistack_types::error::ErrorCode;
    use pistack_types::{kind, Meta};
    use std::net::Ipv6Addr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn balancer(vip: Option<IpAddr>) -> Balancer {
        Balancer {
            meta: Meta::new(kind::BALANCER, "lb", None),
            spec: BalancerSpec {
                vip_network: "net".into(),
                vip_address: vip,
                vip_port: None,
                floating_ip: None,
                provider: Provider::Forward,
                admin_up: true,
            },
            status: BalancerStatus::default(),
        }
    }

    fn view_with(lb: &Balancer, members: Vec<SocketAddr>) -> ListenerView {
        let weights = vec![1u16; members.len()];
        let up = vec![OperatingStatus::NoMonitor; members.len()];
        weighted_view(lb, members, weights, up, Algorithm::default())
    }

    fn weighted_view(
        lb: &Balancer,
        members: Vec<SocketAddr>,
        weights: Vec<u16>,
        up: Vec<OperatingStatus>,
        algorithm: Algorithm,
    ) -> ListenerView {
        let pool_meta = Meta::new(kinds::POOL, "pool", None);
        let member_rows = members
            .into_iter()
            .zip(weights)
            .zip(up)
            .map(|((addr, weight), status)| Member {
                meta: Meta::new(kinds::MEMBER, "m", None),
                spec: MemberSpec {
                    pool: pool_meta.id.clone(),
                    address: addr.ip(),
                    port: addr.port(),
                    weight,
                    backup: false,
                    admin_up: true,
                },
                status: MemberStatus {
                    operating_status: status,
                    ..MemberStatus::default()
                },
            })
            .collect();
        let pool = Pool {
            meta: pool_meta,
            spec: PoolSpec {
                balancer: lb.meta.id.clone(),
                protocol: PoolProtocol::Tcp,
                algorithm,
                session_persistence: None,
                health_monitor: None,
                admin_up: true,
            },
            status: PoolStatus::default(),
        };
        ListenerView {
            balancer: lb.clone(),
            listener: Listener {
                meta: Meta::new(kinds::LISTENER, "ln", None),
                spec: ListenerSpec {
                    balancer: lb.meta.id.clone(),
                    protocol: ListenerProtocol::Tcp,
                    port: 80,
                    default_pool: Some(pool.meta.id.clone()),
                    connection_limit: 0,
                    admin_up: true,
                    door: None,
                    routes: Vec::new(),
                },
                status: ListenerStatus::default(),
            },
            pool: Some((pool, member_rows)),
            tls: None,
            routes: Vec::new(),
        }
    }

    fn udp_view(lb: &Balancer, members: Vec<SocketAddr>) -> ListenerView {
        let mut view = view_with(lb, members);
        view.listener.spec.protocol = ListenerProtocol::Udp;
        if let Some((pool, _)) = view.pool.as_mut() {
            pool.spec.protocol = PoolProtocol::Udp;
        }
        view
    }

    async fn answer(listener: TcpListener, body: &'static [u8]) {
        let (mut sock, _) = listener.accept().await.unwrap();
        let _ = sock.set_nodelay(true);
        let mut buf = [0u8; 4];
        sock.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        sock.write_all(body).await.unwrap();
        let _ = sock.read(&mut buf).await;
    }

    async fn read_n(addr: &str, n: usize) -> Vec<u8> {
        let mut client = TcpStream::connect(addr).await.unwrap();
        let _ = client.set_nodelay(true);
        client.write_all(b"ping").await.unwrap();
        let mut buf = vec![0u8; n];
        tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut buf))
            .await
            .expect("timed out waiting for the member")
            .unwrap();
        buf
    }

    #[test]
    fn serves_l4_not_pingora() {
        let p = ForwardProvider::new();
        assert_eq!(p.name(), "forward");
        assert!(p.serves(Provider::Forward));
        assert!(!p.serves(Provider::Relay));
    }

    #[test]
    fn vip_allowlist() {
        assert!(vip_allowed(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(vip_allowed(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(vip_allowed(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
        assert!(vip_allowed(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(vip_allowed(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
        assert!(vip_allowed(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!vip_allowed(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        assert!(!vip_allowed(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!vip_allowed(IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1))));
        assert!(!vip_allowed(IpAddr::V4(Ipv4Addr::BROADCAST)));
        assert!(!vip_allowed(IpAddr::V4(Ipv4Addr::new(172, 15, 255, 255))));
        assert!(!vip_allowed(IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))));
        assert!(!vip_allowed(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
        assert!(!vip_allowed(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(!vip_allowed(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        assert!(!vip_allowed("fe80::1".parse().unwrap()));
        assert!(vip_allowed("fd00::1".parse().unwrap()));
    }

    #[tokio::test]
    async fn refuses_unspecified_public_and_multicast() {
        let p = ForwardProvider::new();
        for ip in [
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
        ] {
            let err = p.ensure_balancer(&balancer(Some(ip))).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::BadRequest, "{ip}");
        }
    }

    #[tokio::test]
    async fn forwards_ping_and_delete_refuses_new_connects() {
        let member = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let member_addr = member.local_addr().unwrap();
        let member_task = tokio::spawn(answer(member, b"pong"));

        let p = ForwardProvider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        let again = p.ensure_balancer(&lb).await.unwrap();
        assert_eq!(again, realized);
        assert_eq!(realized.vip, IpAddr::V4(Ipv4Addr::LOCALHOST));
        let bound: SocketAddr = realized.provider_ref.parse().unwrap();
        assert_eq!(bound.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(bound.port(), 0);
        assert_eq!(realized.provider_ref, format!("127.0.0.1:{}", bound.port()));

        let view = view_with(&lb, vec![member_addr]);
        p.ensure_listener(&view).await.unwrap();
        assert_eq!(read_n(&realized.provider_ref, 4).await, b"pong");
        member_task.await.unwrap();

        p.delete_listener(&lb.meta.id, &view.listener.meta.id)
            .await
            .unwrap();
        p.delete_listener(&lb.meta.id, &view.listener.meta.id)
            .await
            .unwrap();
        let err = TcpStream::connect(&realized.provider_ref)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
    }

    #[tokio::test]
    async fn ensure_listener_replaces_the_member_list() {
        let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let first_addr = first.local_addr().unwrap();
        let second_addr = second.local_addr().unwrap();
        tokio::spawn(answer(first, b"one!"));
        tokio::spawn(answer(second, b"two!"));

        let p = ForwardProvider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        let mut view = view_with(&lb, vec![first_addr]);
        p.ensure_listener(&view).await.unwrap();
        assert_eq!(read_n(&realized.provider_ref, 4).await, b"one!");

        view.pool.as_mut().unwrap().1[0].spec.address = second_addr.ip();
        view.pool.as_mut().unwrap().1[0].spec.port = second_addr.port();
        p.ensure_listener(&view).await.unwrap();
        assert_eq!(read_n(&realized.provider_ref, 4).await, b"two!");
        p.delete_balancer(&lb.meta.id).await.unwrap();
        let err = TcpStream::connect(&realized.provider_ref)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
    }

    #[tokio::test]
    async fn connections_take_the_members_in_turn() {
        let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addrs = vec![first.local_addr().unwrap(), second.local_addr().unwrap()];
        for (member, body) in [(first, b"one!"), (second, b"two!")] {
            tokio::spawn(async move {
                loop {
                    let (mut sock, _) = member.accept().await.unwrap();
                    let mut buf = [0u8; 4];
                    sock.read_exact(&mut buf).await.unwrap();
                    sock.write_all(body).await.unwrap();
                }
            });
        }

        let p = ForwardProvider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        p.ensure_listener(&view_with(&lb, addrs)).await.unwrap();
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(read_n(&realized.provider_ref, 4).await);
        }
        assert_eq!(seen, [b"one!", b"two!", b"one!", b"two!"]);
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }

    /// Two members answering with their own name, plus the ports they listen on.
    /// Members that answer with their own name, so a connection says who
    /// dialed it.
    async fn speakers(bodies: &[&'static [u8]]) -> Vec<SocketAddr> {
        let mut addrs = Vec::new();
        for body in bodies {
            let member = TcpListener::bind("127.0.0.1:0").await.unwrap();
            addrs.push(member.local_addr().unwrap());
            let body = *body;
            tokio::spawn(async move {
                loop {
                    let (mut sock, _) = member.accept().await.unwrap();
                    let mut buf = [0u8; 4];
                    sock.read_exact(&mut buf).await.unwrap();
                    sock.write_all(body).await.unwrap();
                }
            });
        }
        addrs
    }

    /// Open `n` connections through the VIP in turn, each closed before the
    /// next, and return what the member answered with.
    async fn dial_n(vip: &str, n: usize) -> Vec<Vec<u8>> {
        let mut seen = Vec::new();
        for _ in 0..n {
            let mut client = TcpStream::connect(vip).await.unwrap();
            let _ = client.set_nodelay(true);
            client.write_all(b"ping").await.unwrap();
            let mut buf = vec![0u8; 4];
            tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut buf))
                .await
                .expect("timed out waiting for the member")
                .unwrap();
            seen.push(buf);
            drop(client);
        }
        seen
    }

    fn tally(bodies: &[Vec<u8>]) -> Vec<(String, usize)> {
        let mut out: Vec<(String, usize)> = Vec::new();
        for body in bodies {
            let name = String::from_utf8_lossy(body).to_string();
            match out.iter_mut().find(|(k, _)| *k == name) {
                Some((_, n)) => *n += 1,
                None => out.push((name, 1)),
            }
        }
        out.sort();
        out
    }

    #[tokio::test]
    async fn round_robin_honours_weight() {
        let addrs = speakers(&[b"one!", b"two!"]).await;
        let p = ForwardProvider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        let view = weighted_view(
            &lb,
            addrs,
            vec![3, 1],
            vec![OperatingStatus::NoMonitor; 2],
            Algorithm::RoundRobin,
        );
        p.ensure_listener(&view).await.unwrap();
        let seen = dial_n(&realized.provider_ref, 8).await;
        assert_eq!(
            tally(&seen),
            vec![("one!".into(), 6), ("two!".into(), 2)],
            "weight 3 against weight 1 takes three in four: {seen:?}"
        );
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }

    #[tokio::test]
    async fn least_conn_picks_the_member_with_the_fewest_open() {
        let addrs = speakers(&[b"one!", b"two!"]).await;
        let p = ForwardProvider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        let view = weighted_view(
            &lb,
            addrs.clone(),
            vec![1, 1],
            vec![OperatingStatus::NoMonitor; 2],
            Algorithm::LeastConn,
        );
        p.ensure_listener(&view).await.unwrap();
        // One connection held open on the first member: the next must be
        // dialed to the second.
        let mut hold = TcpStream::connect(&realized.provider_ref).await.unwrap();
        hold.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(5), hold.read(&mut buf))
            .await
            .expect("timed out")
            .unwrap();
        assert_eq!(
            buf, *b"one!",
            "the held connection sits on the first member"
        );
        assert_eq!(
            dial_n(&realized.provider_ref, 1).await[0],
            b"two!",
            "the member already holding one is not picked again"
        );
        drop(hold);
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }

    #[tokio::test]
    async fn source_ip_keeps_one_client_on_one_member() {
        let addrs = speakers(&[b"one!", b"two!", b"thr!"]).await;
        let p = ForwardProvider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        let view = weighted_view(
            &lb,
            addrs,
            vec![1, 1, 1],
            vec![OperatingStatus::NoMonitor; 3],
            Algorithm::SourceIp,
        );
        p.ensure_listener(&view).await.unwrap();
        // Every connection here arrives from 127.0.0.1, so all of them must
        // land on the one member that address hashes to. Against three
        // members, plain round robin would answer `one!` twice in six.
        let seen = dial_n(&realized.provider_ref, 6).await;
        assert_eq!(
            tally(&seen).len(),
            1,
            "one client address moved between members: {seen:?}"
        );
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }

    #[tokio::test]
    async fn an_unhealthy_member_is_never_dialed() {
        let addrs = speakers(&[b"one!", b"two!"]).await;
        for algo in [
            Algorithm::RoundRobin,
            Algorithm::LeastConn,
            Algorithm::SourceIp,
        ] {
            let p = ForwardProvider::new();
            let lb = balancer(None);
            let realized = p.ensure_balancer(&lb).await.unwrap();
            // The second member answers, but is Offline, so the reconciler
            // never hands it to the forwarder at all.
            let view = weighted_view(
                &lb,
                vec![addrs[0]],
                vec![1],
                vec![OperatingStatus::NoMonitor],
                algo,
            );
            p.ensure_listener(&view).await.unwrap();
            for body in dial_n(&realized.provider_ref, 4).await {
                assert_eq!(body, b"one!", "{algo}: the offline member took traffic");
            }
            p.delete_balancer(&lb.meta.id).await.unwrap();
        }
    }

    #[tokio::test]
    async fn no_member_is_accepted_and_closed() {
        let p = ForwardProvider::new();
        let lb = balancer(Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        let realized = p.ensure_balancer(&lb).await.unwrap();
        assert!(realized.provider_ref.starts_with("127.0.0.1:"));
        let mut view = view_with(&lb, vec![]);
        view.pool = None;
        p.ensure_listener(&view).await.unwrap();
        let mut client = TcpStream::connect(&realized.provider_ref).await.unwrap();
        let mut buf = [0u8; 1];
        let n = tokio::time::timeout(Duration::from_secs(2), client.read(&mut buf))
            .await
            .expect("timed out")
            .unwrap();
        assert_eq!(n, 0);

        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);
        p.ensure_listener(&view_with(&lb, vec![dead_addr]))
            .await
            .unwrap();
        let mut client = TcpStream::connect(&realized.provider_ref).await.unwrap();
        let n = tokio::time::timeout(Duration::from_secs(2), client.read(&mut buf))
            .await
            .expect("timed out")
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn udp_copies_to_an_up_member_and_empty_is_not_ready() {
        let p = ForwardProvider::new();
        let lb = balancer(Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        p.ensure_balancer(&lb).await.unwrap();
        let empty = p.ensure_listener(&udp_view(&lb, vec![])).await.unwrap_err();
        assert_eq!(empty.code, ErrorCode::Unavailable);
        assert!(empty.message.contains("no up member"));

        let member = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let member_addr = member.local_addr().unwrap();
        let member_task = tokio::spawn(async move {
            let mut buf = [0u8; 16];
            let (n, peer) = member.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"ping");
            member.send_to(b"pong", peer).await.unwrap();
        });
        let bound = p
            .ensure_listener(&udp_view(&lb, vec![member_addr]))
            .await
            .unwrap();
        let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(b"ping", &bound).await.unwrap();
        let mut buf = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(2), client.recv(&mut buf))
            .await
            .expect("timed out")
            .unwrap();
        assert_eq!(&buf[..n], b"pong");
        member_task.await.unwrap();

        let err = p.ensure_listener(&udp_view(&lb, vec![])).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }
}
