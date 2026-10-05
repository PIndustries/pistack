// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! In-process HTTP reverse proxy. One TCP socket per balancer, and a UDP
//! socket when the listener is UDP.
//!
//! `ensure_balancer` binds `127.0.0.1:0` unless `spec.vip_address` is
//! loopback or private. Unspecified, public, multicast, broadcast,
//! link-local, and 100.64/10 addresses are refused. An `Http` listener
//! picks a route by host and path (falling back to the default pool, and
//! answering 404 when neither applies), rewrites the request onto that
//! pool's next member and copies the response back.
//! `Tcp` and `Https` are byte copies. `Terminated` presents the certificate
//! `keep` already stored for that listener and stays unavailable when it
//! has none. This proxy does not generate a certificate. UDP copies
//! datagrams to an up member and is not ready when that list is empty.

use super::{LbProvider, ListenerView, Realized};
use crate::types::{Balancer, ListenerProtocol, Member, PathMatch, Provider};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

#[derive(Clone)]
enum Serve {
    Http,
    Stream,
    Tls(Arc<rustls::ServerConfig>),
}

struct Slot {
    bound: SocketAddr,
    listener: Option<TcpListener>,
    task: Option<JoinHandle<()>>,
    cancel: tokio_util::sync::CancellationToken,
    members: Arc<parking_lot::Mutex<Table>>,
    cursor: Arc<AtomicUsize>,
    limit: Arc<AtomicUsize>,
    inflight: Arc<AtomicUsize>,
    mode: Arc<parking_lot::Mutex<Serve>>,
    udp: Option<crate::udp::UdpForward>,
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
            let slot = slots
                .get(&id)
                .ok_or_else(|| ApiError::bad_request("http balancer is not bound"))?;
            if let Some(udp) = &slot.udp {
                udp.set_members(targets);
                return Ok(udp.bound().to_string());
            }
            slot.bound.ip()
        };
        let udp = crate::udp::UdpForward::bind(SocketAddr::new(ip, 0), targets).await?;
        let bound = udp.bound().to_string();
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&id)
            .ok_or_else(|| ApiError::bad_request("http balancer is not bound"))?;
        if let Some(existing) = &slot.udp {
            existing.set_members(crate::udp::targets(view));
            drop(udp);
            return Ok(existing.bound().to_string());
        }
        slot.udp = Some(udp);
        Ok(bound)
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
    crate::bind::allowed(ip)
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
    Ok(SocketAddr::new(ip, lb.spec.vip_port.unwrap_or(0)))
}

fn realized(bound: SocketAddr) -> Realized {
    Realized {
        vip: bound.ip(),
        provider_ref: bound.to_string(),
    }
}

fn live_addrs(members: &[Member]) -> Vec<SocketAddr> {
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

fn table_for(view: &ListenerView) -> Table {
    if !view.listener.spec.admin_up {
        return Table::default();
    }
    let default = match &view.pool {
        Some((_, members)) => live_addrs(members),
        None => Vec::new(),
    };
    let routes = view
        .routes
        .iter()
        .map(|r| RouteTarget {
            host: r.route.host.as_deref().map(normalize_host),
            path: r.route.path.clone(),
            exact: r.route.path_match == PathMatch::Exact,
            members: if r.pool.spec.admin_up {
                live_addrs(&r.members)
            } else {
                Vec::new()
            },
        })
        .collect();
    Table { default, routes }
}

/// Where requests on one listener go: routes first, then the default pool.
#[derive(Default)]
struct Table {
    default: Vec<SocketAddr>,
    routes: Vec<RouteTarget>,
}

struct RouteTarget {
    host: Option<String>,
    path: String,
    exact: bool,
    members: Vec<SocketAddr>,
}

#[derive(Debug, PartialEq, Eq)]
enum Choice {
    Member(SocketAddr),
    /// A route or the default pool applies but has no member up.
    Unavailable,
    /// Routes exist, none matches, and there is no default member.
    NotFound,
}

impl Table {
    fn choose(&self, host: Option<&str>, path: &str, cursor: &AtomicUsize) -> Choice {
        let take = |members: &[SocketAddr]| match pick(members, cursor) {
            Some(addr) => Choice::Member(addr),
            None => Choice::Unavailable,
        };
        let mut best: Option<((u8, usize, bool), &RouteTarget)> = None;
        for route in &self.routes {
            let Some(host_rank) = host_rank(route.host.as_deref(), host) else {
                continue;
            };
            let Some(len) = path_len(&route.path, route.exact, path) else {
                continue;
            };
            let rank = (host_rank, len, route.exact);
            if best.as_ref().is_none_or(|(have, _)| rank > *have) {
                best = Some((rank, route));
            }
        }
        match best {
            Some((_, route)) => take(&route.members),
            None if self.routes.is_empty() || !self.default.is_empty() => take(&self.default),
            None => Choice::NotFound,
        }
    }
}

/// 2 for an exact host, 1 for a wildcard, 0 for a route with no host.
fn host_rank(route: Option<&str>, request: Option<&str>) -> Option<u8> {
    let Some(want) = route else {
        return Some(0);
    };
    let have = request?;
    if let Some(suffix) = want.strip_prefix('*') {
        let label = have.strip_suffix(suffix)?;
        return (!label.is_empty() && !label.contains('.')).then_some(1);
    }
    (want == have).then_some(2)
}

/// Length of the matched route path, or `None` when it does not match.
fn path_len(route: &str, exact: bool, request: &str) -> Option<usize> {
    if exact {
        return (route == request).then_some(route.len());
    }
    let base = route.trim_end_matches('/');
    if base.is_empty() {
        return Some(0);
    }
    let matched = request == base
        || request
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'));
    matched.then_some(base.len())
}

/// Lower-case, without a port or a trailing dot.
fn normalize_host(raw: &str) -> String {
    let raw = raw.trim();
    let host = if let Some(rest) = raw.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        raw.rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
            .map_or(raw, |(host, _)| host)
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// The request's host and path. The host comes from an absolute request
/// target when there is one, otherwise from the `Host` header. The path
/// drops any query or fragment.
fn route_key(head: &[u8]) -> (Option<String>, String) {
    let Ok(text) = std::str::from_utf8(head) else {
        return (None, "/".into());
    };
    let mut lines = text.split("\r\n");
    let target = lines
        .next()
        .and_then(|start| start.split_whitespace().nth(1))
        .unwrap_or("/");
    let mut host = None;
    let mut path = target;
    for scheme in ["http://", "https://"] {
        if let Some(rest) = target.strip_prefix(scheme) {
            let (authority, tail) = match rest.find('/') {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, "/"),
            };
            host = Some(normalize_host(authority));
            path = tail;
        }
    }
    if host.is_none() {
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("host") {
                    host = Some(normalize_host(value));
                    break;
                }
            }
        }
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    (host.filter(|h| !h.is_empty()), path.to_owned())
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
    members: Arc<parking_lot::Mutex<Table>>,
    cursor: Arc<AtomicUsize>,
    limit: Arc<AtomicUsize>,
    inflight: Arc<AtomicUsize>,
    mode: Arc<parking_lot::Mutex<Serve>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        let _ = stream.set_nodelay(true);
                        let cap = limit.load(Ordering::Relaxed);
                        let now = inflight.fetch_add(1, Ordering::Relaxed);
                        let serve = mode.lock().clone();
                        if cap > 0 && now >= cap {
                            inflight.fetch_sub(1, Ordering::Relaxed);
                            if matches!(serve, Serve::Http | Serve::Tls(_)) {
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
                            match serve {
                                Serve::Http => proxy_http(stream, peer, members, cursor).await,
                                Serve::Stream => splice(stream, members, cursor).await,
                                Serve::Tls(cfg) => {
                                    let acceptor = tokio_rustls::TlsAcceptor::from(cfg);
                                    match acceptor.accept(stream).await {
                                        Ok(tls) => {
                                            proxy_http(tls, peer, members, cursor).await;
                                        }
                                        Err(e) => {
                                            tracing::debug!(error = %e, "tls handshake failed");
                                        }
                                    }
                                }
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

async fn refuse<S>(mut stream: S)
where
    S: tokio::io::AsyncWrite + Unpin,
{
    let body = b"unavailable";
    let msg = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes()).await;
    let _ = stream.write_all(body).await;
}

async fn not_found<S>(mut stream: S)
where
    S: tokio::io::AsyncWrite + Unpin,
{
    let body = b"no route";
    let msg = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes()).await;
    let _ = stream.write_all(body).await;
}

async fn splice(
    mut inbound: TcpStream,
    members: Arc<parking_lot::Mutex<Table>>,
    cursor: Arc<AtomicUsize>,
) {
    let dest = {
        let guard = members.lock();
        pick(&guard.default, &cursor)
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

async fn proxy_http<S>(
    mut inbound: S,
    peer: SocketAddr,
    members: Arc<parking_lot::Mutex<Table>>,
    cursor: Arc<AtomicUsize>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let read = read_headers(&mut inbound).await;
    let Some((head, rest)) = read else {
        return;
    };
    let choice = {
        let (host, path) = route_key(&head);
        members.lock().choose(host.as_deref(), &path, &cursor)
    };
    let dest = match choice {
        Choice::Member(dest) => dest,
        Choice::Unavailable => {
            let _ = refuse(inbound).await;
            return;
        }
        Choice::NotFound => {
            let _ = not_found(inbound).await;
            return;
        }
    };
    if header_has(&head, "transfer-encoding") {
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
    let mut outbound =
        match tokio::time::timeout(std::time::Duration::from_secs(5), TcpStream::connect(dest))
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
    let _ = tokio::io::AsyncWriteExt::shutdown(&mut inbound).await;
}

async fn read_headers<R>(stream: &mut R) -> Option<(Vec<u8>, Vec<u8>)>
where
    R: tokio::io::AsyncRead + Unpin,
{
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
                members: Arc::new(parking_lot::Mutex::new(Table::default())),
                cursor: Arc::new(AtomicUsize::new(0)),
                limit: Arc::new(AtomicUsize::new(0)),
                inflight: Arc::new(AtomicUsize::new(0)),
                mode: Arc::new(parking_lot::Mutex::new(Serve::Http)),
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
        let serve = match &view.listener.spec.protocol {
            ListenerProtocol::Udp => unreachable!("udp returned above"),
            ListenerProtocol::Terminated { .. } => {
                let material = view
                    .tls
                    .as_ref()
                    .ok_or_else(|| ApiError::unavailable("no certificate for this listener"))?;
                Serve::Tls(crate::tls::server_config(material)?)
            }
            ListenerProtocol::Http => Serve::Http,
            ListenerProtocol::Tcp | ListenerProtocol::Https => Serve::Stream,
        };
        let table = table_for(view);
        let limit = view.listener.spec.connection_limit as usize;
        let id = view.balancer.meta.id.clone();
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&id)
            .ok_or_else(|| ApiError::bad_request("http balancer is not bound"))?;
        *slot.members.lock() = table;
        slot.limit.store(limit, Ordering::Relaxed);
        *slot.mode.lock() = serve;
        if slot.task.is_none() {
            let listener = slot
                .listener
                .take()
                .ok_or_else(|| ApiError::internal("http socket is missing"))?;
            let members = slot.members.clone();
            let cursor = slot.cursor.clone();
            let limit = slot.limit.clone();
            let inflight = slot.inflight.clone();
            let mode = slot.mode.clone();
            let cancel = slot.cancel.clone();
            slot.task = Some(tokio::spawn(async move {
                accept_loop(listener, members, cursor, limit, inflight, mode, cancel).await;
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
                vip_port: None,
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
                    routes: Vec::new(),
                },
                status: ListenerStatus::default(),
            },
            pool: Some((pool, member_rows)),
            tls: None,
            routes: Vec::new(),
        }
    }

    #[test]
    fn serves_http_not_l4() {
        let p = RelayL7Provider::new();
        assert_eq!(p.name(), "http");
        assert!(p.serves(Provider::Relay));
        assert!(!p.serves(Provider::Forward));
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
        assert_eq!(udp.code, ErrorCode::Unavailable);
        assert!(udp.message.contains("no up member"));
        for ip in [
            IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
            IpAddr::V4(Ipv4Addr::BROADCAST),
        ] {
            let err = p.ensure_balancer(&balancer(Some(ip))).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::BadRequest, "{ip}");
        }
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
        assert!(tls.message.contains("no certificate"));
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
        let err = TcpStream::connect(&realized.provider_ref)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
    }

    fn target(host: Option<&str>, path: &str, exact: bool, port: u16) -> RouteTarget {
        RouteTarget {
            host: host.map(str::to_owned),
            path: path.into(),
            exact,
            members: vec![SocketAddr::from(([127, 0, 0, 1], port))],
        }
    }

    fn port_of(choice: Choice) -> u16 {
        match choice {
            Choice::Member(addr) => addr.port(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn table_prefers_specific_host_then_longest_path() {
        let cursor = AtomicUsize::new(0);
        let table = Table {
            default: Vec::new(),
            routes: vec![
                target(None, "/", false, 1),
                target(Some("*.example.com"), "/", false, 2),
                target(Some("app.example.com"), "/", false, 3),
                target(Some("app.example.com"), "/api/", false, 4),
                target(Some("app.example.com"), "/api", true, 5),
            ],
        };
        let at = |host: Option<&str>, path: &str| port_of(table.choose(host, path, &cursor));
        assert_eq!(at(Some("app.example.com"), "/"), 3);
        assert_eq!(at(Some("app.example.com"), "/api/v1"), 4);
        assert_eq!(at(Some("app.example.com"), "/api"), 5);
        assert_eq!(at(Some("app.example.com"), "/apix"), 3);
        assert_eq!(at(Some("www.example.com"), "/api"), 2);
        assert_eq!(at(Some("a.b.example.com"), "/"), 1);
        assert_eq!(at(None, "/"), 1);
    }

    #[test]
    fn table_falls_back_then_404s() {
        let cursor = AtomicUsize::new(0);
        let mut table = Table {
            default: Vec::new(),
            routes: vec![target(Some("app"), "/x", false, 1)],
        };
        assert_eq!(table.choose(Some("app"), "/y", &cursor), Choice::NotFound);
        table.default = vec![SocketAddr::from(([127, 0, 0, 1], 9))];
        assert_eq!(port_of(table.choose(Some("app"), "/y", &cursor)), 9);
        table.routes[0].members.clear();
        assert_eq!(
            table.choose(Some("app"), "/x/1", &cursor),
            Choice::Unavailable
        );
        let empty = Table::default();
        assert_eq!(empty.choose(None, "/", &cursor), Choice::Unavailable);
    }

    #[test]
    fn route_key_reads_host_and_path() {
        let key = |raw: &str| route_key(raw.as_bytes());
        assert_eq!(
            key("GET /a/b?q=1 HTTP/1.1\r\nHost: App.Example.com:8080\r\n\r\n"),
            (Some("app.example.com".into()), "/a/b".into())
        );
        assert_eq!(
            key("GET http://Edge.Local./x#f HTTP/1.1\r\nHost: other\r\n\r\n"),
            (Some("edge.local".into()), "/x".into())
        );
        assert_eq!(
            key("GET ?q HTTP/1.1\r\nhost: [::1]:80\r\n\r\n"),
            (Some("::1".into()), "/".into())
        );
        assert_eq!(key("GET / HTTP/1.0\r\n\r\n"), (None, "/".into()));
    }
}
