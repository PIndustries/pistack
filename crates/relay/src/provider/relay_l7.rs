// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! In-process HTTP reverse proxy. One socket per balancer.
//!
//! `ensure_balancer` binds `127.0.0.1:0` unless `spec.vip_address` is
//! loopback or private. `0.0.0.0`, public addresses, and multicast are
//! refused. An `Http` listener rewrites the request onto the next member
//! and copies the response back. `Tcp` and `Https` are byte copies.
//! `Terminated` stays unavailable until a certificate is configured on
//! this process. `Udp` is refused.

use super::{LbProvider, ListenerView, Realized};
use crate::types::{Balancer, ListenerProtocol, Provider};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

struct Slot {
    bound: SocketAddr,
    listener: Option<TcpListener>,
    task: Option<JoinHandle<()>>,
    cancel: tokio_util::sync::CancellationToken,
    members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>,
    cursor: Arc<AtomicUsize>,
    limit: Arc<AtomicUsize>,
    inflight: Arc<AtomicUsize>,
}

/// HTTP reverse proxy. Serves [`Provider::Relay`].
pub struct RelayL7Provider {
    slots: parking_lot::Mutex<HashMap<Id, Slot>>,
}

impl Default for RelayL7Provider {
    fn default() -> Self {
        Self {
            slots: parking_lot::Mutex::new(HashMap::new()),
        }
    }
}

impl RelayL7Provider {
    pub fn new() -> Self {
        Self::default()
    }

    async fn stop(&self, id: &Id) {
        let slot = self.slots.lock().remove(id);
        if let Some(slot) = slot {
            slot.cancel.cancel();
            if let Some(task) = slot.task {
                let _ = task.await;
            }
        }
    }
}

impl Drop for RelayL7Provider {
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
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast() {
                return false;
            }
            v4.is_loopback() || v4.is_private()
        }
        IpAddr::V6(v6) => {
            if v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            v6.is_loopback() || v6.is_unique_local()
        }
    }
}

fn bind_target(lb: &Balancer) -> ApiResult<SocketAddr> {
    let ip = match lb.spec.vip_address {
        None => IpAddr::V4(Ipv4Addr::LOCALHOST),
        Some(ip) if vip_allowed(ip) => ip,
        Some(ip) => {
            return Err(ApiError::bad_request(format!(
                "http refuses vip {ip}; only a loopback or private address can be bound"
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
    if !view.listener.spec.admin_up {
        return Vec::new();
    }
    let Some((_, members)) = &view.pool else {
        return Vec::new();
    };
    let primary: Vec<SocketAddr> = members
        .iter()
        .filter(|m| !m.spec.backup)
        .map(|m| m.addr())
        .collect();
    if !primary.is_empty() {
        return primary;
    }
    members.iter().map(|m| m.addr()).collect()
}

fn pick(members: &[SocketAddr], cursor: &AtomicUsize) -> Option<SocketAddr> {
    if members.is_empty() {
        return None;
    }
    let n = cursor.fetch_add(1, Ordering::Relaxed);
    Some(members[n % members.len()])
}

async fn accept_loop(
    listener: TcpListener,
    members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>,
    cursor: Arc<AtomicUsize>,
    limit: Arc<AtomicUsize>,
    inflight: Arc<AtomicUsize>,
    http: bool,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        let cap = limit.load(Ordering::Relaxed);
                        let now = inflight.fetch_add(1, Ordering::Relaxed);
                        if cap > 0 && now >= cap {
                            inflight.fetch_sub(1, Ordering::Relaxed);
                            if http {
                                tokio::spawn(async move {
                                    let _ = refuse(stream).await;
                                });
                            }
                            continue;
                        }
                        let members = members.clone();
                        let cursor = cursor.clone();
                        let inflight = inflight.clone();
                        tokio::spawn(async move {
                            if http {
                                proxy_http(stream, peer, members, cursor).await;
                            } else {
                                splice(stream, members, cursor).await;
                            }
                            inflight.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "http accept ended");
                        break;
                    }
                }
            }
        }
    }
}

async fn refuse(mut stream: TcpStream) {
    let body = b"unavailable";
    let msg = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes()).await;
    let _ = stream.write_all(body).await;
}

async fn splice(
    mut inbound: TcpStream,
    members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>,
    cursor: Arc<AtomicUsize>,
) {
    let dest = {
        let guard = members.lock();
        pick(&guard, &cursor)
    };
    let Some(dest) = dest else {
        return;
    };
    let mut outbound = match TcpStream::connect(dest).await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::debug!(%dest, error = %e, "http member is not up");
            return;
        }
    };
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
}

async fn proxy_http(
    mut inbound: TcpStream,
    peer: SocketAddr,
    members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>,
    cursor: Arc<AtomicUsize>,
) {
    let _ = inbound.set_nodelay(true);
    let read = read_headers(&mut inbound).await;
    let Some((head, rest)) = read else {
        return;
    };
    if header_has(&head, "transfer-encoding") {
        let dest = {
            let guard = members.lock();
            pick(&guard, &cursor)
        };
        let Some(dest) = dest else {
            let _ = refuse(inbound).await;
            return;
        };
        let mut outbound = match TcpStream::connect(dest).await {
            Ok(stream) => stream,
            Err(_) => {
                let _ = refuse(inbound).await;
                return;
            }
        };
        let _ = outbound.write_all(&head).await;
        let _ = outbound.write_all(&rest).await;
        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
        return;
    }
    let Some(rewritten) = rewrite_request(&head, peer) else {
        return;
    };
    let dest = {
        let guard = members.lock();
        pick(&guard, &cursor)
    };
    let Some(dest) = dest else {
        let _ = refuse(inbound).await;
        return;
    };
    let mut outbound = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(dest),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        _ => {
            let _ = refuse(inbound).await;
            return;
        }
    };
    let _ = outbound.set_nodelay(true);
    if outbound.write_all(&rewritten).await.is_err() {
        return;
    }
    if !rest.is_empty() && outbound.write_all(&rest).await.is_err() {
        return;
    }
    let need = content_length(&head).unwrap_or(0);
    if need > rest.len() {
        let mut left = need - rest.len();
        let mut buf = [0u8; 8192];
        while left > 0 {
            let n = match inbound.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let take = n.min(left);
            if outbound.write_all(&buf[..take]).await.is_err() {
                return;
            }
            left -= take;
        }
    }
    let _ = tokio::io::copy(&mut outbound, &mut inbound).await;
}

async fn read_headers(stream: &mut TcpStream) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if buf.len() > 65_536 {
            return None;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        let n = match tokio::time::timeout(left, stream.read(&mut tmp)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return None,
            Ok(Ok(n)) => n,
        };
        buf.extend_from_slice(&tmp[..n]);
        if let Some(end) = find_header_end(&buf) {
            let rest = buf.split_off(end);
            return Some((buf, rest));
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn header_has(head: &[u8], name: &str) -> bool {
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    text.lines().skip(1).any(|line| {
        line.split(':')
            .next()
            .is_some_and(|h| h.trim().eq_ignore_ascii_case(name))
    })
}

fn content_length(head: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(head).ok()?;
    for line in text.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

fn rewrite_request(head: &[u8], peer: SocketAddr) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(head).ok()?;
    let (start, headers) = text.split_once("\r\n")?;
    let mut parts = start.split_whitespace();
    let method = parts.next()?;
    let _path = parts.next()?;
    let version = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if !matches!(
        method,
        "GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" | "PATCH"
    ) {
        return None;
    }
    if !version.starts_with("HTTP/1.") {
        return None;
    }
    let mut out = String::new();
    out.push_str(start);
    out.push_str("\r\n");
    let mut forwarded = false;
    for line in headers.split("\r\n") {
        if line.is_empty() {
            break;
        }
        let name = line.split(':').next().unwrap_or("").trim();
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
        ) {
            continue;
        }
        if name.eq_ignore_ascii_case("x-forwarded-for") {
            forwarded = true;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    if !forwarded {
        out.push_str(&format!("X-Forwarded-For: {peer}\r\n"));
    }
    out.push_str("Connection: close\r\n\r\n");
    Some(out.into_bytes())
}

#[async_trait]
impl LbProvider for RelayL7Provider {
    fn name(&self) -> &'static str {
        "http"
    }

    fn serves(&self, provider: Provider) -> bool {
        provider == Provider::Relay
    }

    async fn ensure_balancer(&self, lb: &Balancer) -> ApiResult<Realized> {
        if let Some(bound) = self.slots.lock().get(&lb.meta.id).map(|s| s.bound) {
            return Ok(realized(bound));
        }
        let target = bind_target(lb)?;
        let listener = TcpListener::bind(target)
            .await
            .map_err(|e| ApiError::unavailable(format!("http bind {target}: {e}")))?;
        let bound = listener
            .local_addr()
            .map_err(|e| ApiError::unavailable(format!("http local address: {e}")))?;
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
                cursor: Arc::new(AtomicUsize::new(0)),
                limit: Arc::new(AtomicUsize::new(0)),
                inflight: Arc::new(AtomicUsize::new(0)),
            },
        );
        Ok(realized(bound))
    }

    async fn delete_balancer(&self, id: &Id) -> ApiResult<()> {
        self.stop(id).await;
        Ok(())
    }

    async fn ensure_listener(&self, view: &ListenerView) -> ApiResult<String> {
        match view.listener.spec.protocol {
            ListenerProtocol::Udp => {
                return Err(ApiError::bad_request("http provider does not serve udp"));
            }
            ListenerProtocol::Terminated { .. } => {
                return Err(ApiError::unavailable(
                    "tls termination is not configured",
                ));
            }
            ListenerProtocol::Http | ListenerProtocol::Tcp | ListenerProtocol::Https => {}
        }
        let http = matches!(view.listener.spec.protocol, ListenerProtocol::Http);
        let addrs = member_addrs(view);
        let limit = view.listener.spec.connection_limit as usize;
        let id = view.balancer.meta.id.clone();
        let mut slots = self.slots.lock();
        let slot = slots.get_mut(&id).ok_or_else(|| {
            ApiError::bad_request("http balancer is not bound")
        })?;
        *slot.members.lock() = addrs;
        slot.limit.store(limit, Ordering::Relaxed);
        if slot.task.is_none() {
            let listener = slot
                .listener
                .take()
                .ok_or_else(|| ApiError::internal("http socket is missing"))?;
            let members = slot.members.clone();
            let cursor = slot.cursor.clone();
            let limit = slot.limit.clone();
            let inflight = slot.inflight.clone();
            let cancel = slot.cancel.clone();
            slot.task = Some(tokio::spawn(async move {
                accept_loop(listener, members, cursor, limit, inflight, http, cancel).await;
            }));
        }
        Ok(slot.bound.to_string())
    }

    async fn delete_listener(&self, balancer: &Id, _listener: &Id) -> ApiResult<()> {
        self.stop(balancer).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ListenerView;
    use crate::types::*;
    use pistack_types::error::ErrorCode;
    use pistack_types::{kind, Meta};

    fn balancer(vip: Option<IpAddr>) -> Balancer {
        Balancer {
            meta: Meta::new(kind::BALANCER, "lb", None),
            spec: BalancerSpec {
                vip_network: "net".into(),
                vip_address: vip,
                floating_ip: None,
                provider: Provider::Relay,
                admin_up: true,
            },
            status: BalancerStatus::default(),
        }
    }

    fn view(lb: &Balancer, protocol: ListenerProtocol, members: Vec<SocketAddr>) -> ListenerView {
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
                protocol: PoolProtocol::Http,
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
                    protocol,
                    port: 80,
                    default_pool: Some(pool.meta.id.clone()),
                    connection_limit: 0,
                    admin_up: true,
                    door: None,
                },
                status: ListenerStatus::default(),
            },
            pool: Some((pool, member_rows)),
        }
    }

    #[test]
    fn serves_http_not_l4() {
        let p = RelayL7Provider::new();
        assert_eq!(p.name(), "http");
        assert!(p.serves(Provider::Relay));
        assert!(!p.serves(Provider::Ovn));
    }

    #[tokio::test]
    async fn refuses_public_udp_and_tls() {
        let p = RelayL7Provider::new();
        let err = p
            .ensure_balancer(&balancer(Some(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)))))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        let lb = balancer(None);
        p.ensure_balancer(&lb).await.unwrap();
        let udp = p
            .ensure_listener(&view(&lb, ListenerProtocol::Udp, vec![]))
            .await
            .unwrap_err();
        assert_eq!(udp.code, ErrorCode::BadRequest);
        let tls = p
            .ensure_listener(&view(
                &lb,
                ListenerProtocol::Terminated {
                    cert: Id::new(kind::CERT),
                },
                vec![],
            ))
            .await
            .unwrap_err();
        assert_eq!(tls.code, ErrorCode::Unavailable);
        assert!(tls.message.contains("tls termination"));
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }

    #[tokio::test]
    async fn proxies_get_and_delete_refuses_new_connects() {
        let member = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let member_addr = member.local_addr().unwrap();
        let member_task = tokio::spawn(async move {
            let (mut sock, _) = member.accept().await.unwrap();
            let mut buf = vec![0u8; 2048];
            let n = sock.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            assert!(req.starts_with("GET / HTTP/1.1"), "{req}");
            assert!(req.contains("X-Forwarded-For:"), "{req}");
            let body = b"pong";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.write_all(body).await.unwrap();
        });

        let p = RelayL7Provider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        p.ensure_listener(&view(&lb, ListenerProtocol::Http, vec![member_addr]))
            .await
            .unwrap();

        let mut client = TcpStream::connect(&realized.provider_ref).await.unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: app\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_to_end(&mut buf),
        )
        .await
        .expect("timed out")
        .unwrap();
        assert!(buf.windows(4).any(|w| w == b"pong"), "{buf:?}");
        member_task.await.unwrap();

        p.delete_listener(&lb.meta.id, &Id::new(kinds::LISTENER))
            .await
            .unwrap();
        let err = TcpStream::connect(&realized.provider_ref).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
    }
}
