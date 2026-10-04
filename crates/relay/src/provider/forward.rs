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
//! address is loopback. The TCP accept loop dials the first member and
//! copies both directions. An empty TCP member list, or a member that does
//! not accept, is accepted and closed. UDP copies datagrams to an up member
//! and is not ready when that list is empty. A later `ensure_listener`
//! replaces the member list without rebinding. `delete_listener` and
//! `delete_balancer` stop the task and drop the socket.

use super::{LbProvider, ListenerView, Realized};
use crate::types::{Balancer, ListenerProtocol, Provider};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

struct Slot {
    bound: SocketAddr,
    /// Held until the accept task starts, then `None`.
    listener: Option<TcpListener>,
    task: Option<JoinHandle<()>>,
    cancel: tokio_util::sync::CancellationToken,
    members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>,
    udp: Option<crate::udp::UdpForward>,
}

/// Local TCP forwarder. Serves [`Provider::Ovn`] (L4). HTTP stays on
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
    Ok(SocketAddr::new(ip, 0))
}

fn realized(bound: SocketAddr) -> Realized {
    Realized {
        vip: bound.ip(),
        provider_ref: bound.to_string(),
    }
}

fn member_addrs(view: &ListenerView) -> Vec<SocketAddr> {
    view.pool
        .as_ref()
        .map(|(_, members)| members.iter().map(|m| m.addr()).collect())
        .unwrap_or_default()
}

async fn accept_loop(
    listener: TcpListener,
    members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let members = members.clone();
                        tokio::spawn(async move {
                            forward_one(stream, members).await;
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

async fn forward_one(mut inbound: TcpStream, members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>) {
    let _ = inbound.set_nodelay(true);
    let dest = members.lock().first().copied();
    let Some(dest) = dest else {
        return;
    };
    let mut outbound = match TcpStream::connect(dest).await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::debug!(%dest, error = %e, "forward member is not up");
            return;
        }
    };
    let _ = outbound.set_nodelay(true);
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
}

#[async_trait]
impl LbProvider for ForwardProvider {
    fn name(&self) -> &'static str {
        "forward"
    }

    fn serves(&self, provider: Provider) -> bool {
        provider == Provider::Ovn
    }

    async fn ensure_balancer(&self, lb: &Balancer) -> ApiResult<Realized> {
        if let Some(bound) = self.slots.lock().get(&lb.meta.id).map(|s| s.bound) {
            return Ok(realized(bound));
        }
        let target = bind_target(lb)?;
        let listener = TcpListener::bind(target).await.map_err(|e| {
            ApiError::unavailable(format!("forward bind {target}: {e}"))
        })?;
        let bound = listener.local_addr().map_err(|e| {
            ApiError::unavailable(format!("forward local address: {e}"))
        })?;
        let mut slots = self.slots.lock();
        if let Some(existing) = slots.get(&lb.meta.id) {
            return Ok(realized(existing.bound));
        }
        slots.insert(
            lb.meta.id.clone(),
            Slot {
                bound,
                listener: Some(listener),
                task: None,
                cancel: tokio_util::sync::CancellationToken::new(),
                members: Arc::new(parking_lot::Mutex::new(Vec::new())),
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
        let addrs = member_addrs(view);
        let id = view.balancer.meta.id.clone();
        let mut slots = self.slots.lock();
        let slot = slots.get_mut(&id).ok_or_else(|| {
            ApiError::bad_request("forward balancer is not bound")
        })?;
        *slot.members.lock() = addrs;
        if slot.task.is_none() {
            let listener = slot.listener.take().ok_or_else(|| {
                ApiError::internal("forward socket is missing")
            })?;
            let members = slot.members.clone();
            let cancel = slot.cancel.clone();
            slot.task = Some(tokio::spawn(async move {
                accept_loop(listener, members, cancel).await;
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
        let ip = {
            let slots = self.slots.lock();
            let slot = slots.get(&id).ok_or_else(|| {
                ApiError::bad_request("forward balancer is not bound")
            })?;
            if let Some(udp) = &slot.udp {
                udp.set_members(targets);
                return Ok(udp.bound().to_string());
            }
            slot.bound.ip()
        };
        let udp = crate::udp::UdpForward::bind(ip, targets).await?;
        let bound = udp.bound().to_string();
        let mut slots = self.slots.lock();
        let slot = slots.get_mut(&id).ok_or_else(|| {
            ApiError::bad_request("forward balancer is not bound")
        })?;
        if let Some(existing) = &slot.udp {
            existing.set_members(crate::udp::targets(view));
            drop(udp);
            return Ok(existing.bound().to_string());
        }
        slot.udp = Some(udp);
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
                floating_ip: None,
                provider: Provider::Ovn,
                admin_up: true,
            },
            status: BalancerStatus::default(),
        }
    }

    fn view_with(lb: &Balancer, members: Vec<SocketAddr>) -> ListenerView {
        let pool_meta = Meta::new(kinds::POOL, "pool", None);
        let member_rows = members
            .into_iter()
            .map(|addr| Member {
                meta: Meta::new(kinds::MEMBER, "m", None),
                spec: MemberSpec {
                    pool: pool_meta.id.clone(),
                    address: addr.ip(),
                    port: addr.port(),
                    weight: 1,
                    backup: false,
                    admin_up: true,
                },
                status: MemberStatus::default(),
            })
            .collect();
        let pool = Pool {
            meta: pool_meta,
            spec: PoolSpec {
                balancer: lb.meta.id.clone(),
                protocol: PoolProtocol::Tcp,
                algorithm: Algorithm::default(),
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
                },
                status: ListenerStatus::default(),
            },
            pool: Some((pool, member_rows)),
            tls: None,
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
        assert!(p.serves(Provider::Ovn));
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
        let err = TcpStream::connect(&realized.provider_ref).await.unwrap_err();
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
        let err = TcpStream::connect(&realized.provider_ref).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
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
