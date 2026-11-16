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
//!
//! Members are picked under the pool's algorithm, the same as on the L4
//! forwarder, and the connections this proxy holds count towards
//! `least_conn`.

use super::{LbProvider, ListenerView, Realized};
use crate::picker::{Backend, Picker};
use crate::types::{Algorithm, Balancer, ListenerProtocol, Member, PathMatch, Provider};
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
    picker: Picker,
    conns: Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
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

/// The members to choose between. A backup member is only used when no
/// primary is left; the rest of the health and weight rules are the
/// picker's.
fn live_backends(members: &[Member]) -> Vec<Backend> {
    let primary: Vec<Backend> = members
        .iter()
        .filter(|m| !m.spec.backup)
        .map(Backend::of)
        .collect();
    if !primary.is_empty() {
        return primary;
    }
    members.iter().map(Backend::of).collect()
}

fn table_for(view: &ListenerView) -> Table {
    if !view.listener.spec.admin_up {
        return Table::default();
    }
    let default = match &view.pool {
        Some((_, members)) => live_backends(members),
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
                live_backends(&r.members)
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
    default: Vec<Backend>,
    routes: Vec<RouteTarget>,
}

struct RouteTarget {
    host: Option<String>,
    path: String,
    exact: bool,
    members: Vec<Backend>,
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
    fn choose(
        &self,
        host: Option<&str>,
        path: &str,
        client: Option<IpAddr>,
        picker: &Picker,
        conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>,
    ) -> Choice {
        let take = |members: &[Backend]| match picker.pick(&weighed(members, conns), client) {
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

/// What one HTTP listener hands to every accepted connection.
struct Served {
    members: Arc<parking_lot::Mutex<Table>>,
    picker: Picker,
    conns: Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
    limit: Arc<AtomicUsize>,
    inflight: Arc<AtomicUsize>,
    mode: Arc<parking_lot::Mutex<Serve>>,
}

async fn accept_loop(
    listener: TcpListener,
    served: Arc<Served>,
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
                        let cap = served.limit.load(Ordering::Relaxed);
                        let now = served.inflight.fetch_add(1, Ordering::Relaxed);
                        let serve = served.mode.lock().clone();
                        if cap > 0 && now >= cap {
                            served.inflight.fetch_sub(1, Ordering::Relaxed);
                            if matches!(serve, Serve::Http | Serve::Tls(_)) {
                                tokio::spawn(async move {
                                    let _ = refuse(stream).await;
                                });
                            }
                            continue;
                        }
                        let served = served.clone();
                        tokio::spawn(async move {
                            match serve {
                                Serve::Http => {
                                    proxy_http(stream, peer, &served).await;
                                }
                                Serve::Stream => {
                                    splice(stream, peer, &served).await;
                                }
                                Serve::Tls(cfg) => {
                                    let acceptor = tokio_rustls::TlsAcceptor::from(cfg);
                                    match acceptor.accept(stream).await {
                                        Ok(tls) => {
                                            proxy_http(tls, peer, &served).await;
                                        }
                                        Err(e) => {
                                            tracing::debug!(error = %e, "tls handshake failed");
                                        }
                                    }
                                }
                            }
                            served.inflight.fetch_sub(1, Ordering::Relaxed);
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

/// One backend with the connections this proxy holds on it.
fn weighed(
    backends: &[Backend],
    conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>,
) -> Vec<Backend> {
    let open = conns.lock();
    backends
        .iter()
        .map(|b| Backend {
            conns: open.get(&b.addr).copied().unwrap_or(0),
            ..*b
        })
        .collect()
}

fn opened(conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>, addr: SocketAddr) {
    *conns.lock().entry(addr).or_insert(0) += 1;
}

fn closed(conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>, addr: SocketAddr) {
    let mut open = conns.lock();
    if let Some(n) = open.get_mut(&addr) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            open.remove(&addr);
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

async fn splice(mut inbound: TcpStream, peer: SocketAddr, served: &Served) {
    let dest = {
        let guard = served.members.lock();
        served
            .picker
            .pick(&weighed(&guard.default, &served.conns), Some(peer.ip()))
    };
    let Some(dest) = dest else {
        return;
    };
    opened(&served.conns, dest);
    let outbound = TcpStream::connect(dest).await;
    let mut outbound = match outbound {
        Ok(stream) => stream,
        Err(e) => {
            closed(&served.conns, dest);
            tracing::debug!(%dest, error = %e, "http member is not up");
            return;
        }
    };
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
    closed(&served.conns, dest);
}

async fn proxy_http<S>(mut inbound: S, peer: SocketAddr, served: &Served)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let read = read_headers(&mut inbound).await;
    let Some((head, rest)) = read else {
        return;
    };
    let choice = {
        let (host, path) = route_key(&head);
        served.members.lock().choose(
            host.as_deref(),
            &path,
            Some(peer.ip()),
            &served.picker,
            &served.conns,
        )
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
    opened(&served.conns, dest);
    let guard = Held(served.conns.clone(), dest);
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
    drop(guard);
}

/// One connection counted against a backend until the request holding it
/// is over, whichever way the request ends.
struct Held(
    Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
    SocketAddr,
);

impl Drop for Held {
    fn drop(&mut self) {
        closed(&self.0, self.1);
    }
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
                picker: Picker::new(Algorithm::default()),
                conns: Arc::new(parking_lot::Mutex::new(HashMap::new())),
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
        let algo = view
            .pool
            .as_ref()
            .map(|(pool, _)| pool.spec.algorithm)
            .unwrap_or_default();
        let limit = view.listener.spec.connection_limit as usize;
        let id = view.balancer.meta.id.clone();
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&id)
            .ok_or_else(|| ApiError::bad_request("http balancer is not bound"))?;
        slot.picker.set_algorithm(algo);
        *slot.members.lock() = table;
        slot.limit.store(limit, Ordering::Relaxed);
        *slot.mode.lock() = serve;
        if slot.task.is_none() {
            let listener = slot
                .listener
                .take()
                .ok_or_else(|| ApiError::internal("http socket is missing"))?;
            let served = Arc::new(Served {
                members: slot.members.clone(),
                picker: slot.picker.clone(),
                conns: slot.conns.clone(),
                limit: slot.limit.clone(),
                inflight: slot.inflight.clone(),
                mode: slot.mode.clone(),
            });
            let cancel = slot.cancel.clone();
            slot.task = Some(tokio::spawn(async move {
                accept_loop(listener, served, cancel).await;
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
        let weights = vec![1u16; members.len()];
        algorithm_view(lb, protocol, members, weights, Algorithm::default())
    }

    /// A view whose pool carries `algorithm`, one weight per member.
    fn algorithm_view(
        lb: &Balancer,
        protocol: ListenerProtocol,
        members: Vec<SocketAddr>,
        weights: Vec<u16>,
        algorithm: Algorithm,
    ) -> ListenerView {
        let pool_meta = Meta::new(kinds::POOL, "pool", None);
        let member_rows = members
            .into_iter()
            .zip(weights)
            .map(|(addr, weight)| Member {
                meta: Meta::new(kinds::MEMBER, "m", None),
                spec: MemberSpec {
                    pool: pool_meta.id.clone(),
                    address: addr.ip(),
                    port: addr.port(),
                    weight,
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
            members: vec![Backend::new(SocketAddr::from(([127, 0, 0, 1], port)))],
        }
    }

    fn port_of(choice: Choice) -> u16 {
        match choice {
            Choice::Member(addr) => addr.port(),
            other => panic!("{other:?}"),
        }
    }

    /// No connections counted: nothing is open, so the picker's own
    /// algorithm decides between the members a route holds.
    fn no_conns() -> parking_lot::Mutex<HashMap<SocketAddr, u64>> {
        parking_lot::Mutex::new(HashMap::new())
    }

    #[test]
    fn table_prefers_specific_host_then_longest_path() {
        let picker = Picker::new(Algorithm::RoundRobin);
        let conns = no_conns();
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
        let at = |host: Option<&str>, path: &str| {
            port_of(table.choose(host, path, None, &picker, &conns))
        };
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
        let picker = Picker::new(Algorithm::RoundRobin);
        let conns = no_conns();
        let mut table = Table {
            default: Vec::new(),
            routes: vec![target(Some("app"), "/x", false, 1)],
        };
        assert_eq!(
            table.choose(Some("app"), "/y", None, &picker, &conns),
            Choice::NotFound
        );
        table.default = vec![Backend::new(SocketAddr::from(([127, 0, 0, 1], 9)))];
        assert_eq!(
            port_of(table.choose(Some("app"), "/y", None, &picker, &conns)),
            9
        );
        table.routes[0].members.clear();
        assert_eq!(
            table.choose(Some("app"), "/x/1", None, &picker, &conns),
            Choice::Unavailable
        );
        let empty = Table::default();
        assert_eq!(
            empty.choose(None, "/", None, &picker, &conns),
            Choice::Unavailable
        );
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

    /// A member answering a request with its own name, so a response says
    /// who served it.
    async fn speaker(body: &'static [u8]) -> SocketAddr {
        let member = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = member.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = member.accept().await.unwrap();
                let mut buf = vec![0u8; 2048];
                if sock.read(&mut buf).await.unwrap_or(0) == 0 {
                    continue;
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if sock.write_all(resp.as_bytes()).await.is_err() {
                    continue;
                }
                let _ = sock.write_all(body).await;
            }
        });
        addr
    }

    /// One request through the proxy, answered with the body the member
    /// returned.
    /// One request through the proxy: the status line and, when the member
    /// answers in time, its body.
    async fn ask(vip: &str, path: &str) -> (Vec<u8>, Vec<u8>) {
        let mut client = TcpStream::connect(vip).await.unwrap();
        client
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: app\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = Vec::new();
        let answered = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.read_to_end(&mut buf),
        )
        .await
        .is_ok();
        let head = match buf.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(at) => buf[..at].to_vec(),
            None => buf.clone(),
        };
        assert!(
            head.starts_with(b"HTTP/1.1"),
            "{}",
            String::from_utf8_lossy(&buf)
        );
        let body = match buf.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(at) if answered => buf[at + 4..].to_vec(),
            _ => Vec::new(),
        };
        (head, body)
    }

    /// One request answered with the member's body.
    async fn served(vip: &str, path: &str) -> Vec<u8> {
        let (head, body) = ask(vip, path).await;
        assert!(
            head.starts_with(b"HTTP/1.1 200"),
            "{}",
            String::from_utf8_lossy(&head)
        );
        body
    }

    async fn ask_n(vip: &str, n: usize) -> Vec<Vec<u8>> {
        let mut seen = Vec::new();
        for _ in 0..n {
            seen.push(served(vip, "/").await);
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
    async fn round_robin_honours_weight_over_http() {
        let addrs = vec![speaker(b"one").await, speaker(b"two").await];
        let p = RelayL7Provider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        let view = algorithm_view(
            &lb,
            ListenerProtocol::Http,
            addrs,
            vec![3, 1],
            Algorithm::RoundRobin,
        );
        p.ensure_listener(&view).await.unwrap();
        assert_eq!(
            tally(&ask_n(&realized.provider_ref, 8).await),
            vec![("one".into(), 6), ("two".into(), 2)],
            "weight 3 against weight 1 takes three in four"
        );
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }

    #[tokio::test]
    async fn least_conn_picks_the_least_loaded_member_over_http() {
        // One member is kept busy so the other has the fewest open
        // connections, which is what the algorithm is asked to find. Its
        // answer is slow, so the count is still held when the next request
        // arrives.
        let busy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let busy_addr = busy.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = busy.accept().await.unwrap();
                let mut buf = vec![0u8; 2048];
                if sock.read(&mut buf).await.unwrap_or(0) == 0 {
                    continue;
                }
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let _ = sock.write_all(b"never").await;
            }
        });
        let idle = speaker(b"idle").await;

        let p = RelayL7Provider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        let view = algorithm_view(
            &lb,
            ListenerProtocol::Http,
            vec![busy_addr, idle],
            vec![1, 1],
            Algorithm::LeastConn,
        );
        p.ensure_listener(&view).await.unwrap();

        let mut hold = TcpStream::connect(&realized.provider_ref).await.unwrap();
        hold.write_all(b"GET / HTTP/1.1\r\nHost: app\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(
            served(&realized.provider_ref, "/").await,
            b"idle",
            "the member already holding one is not picked again"
        );
        drop(hold);
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }

    #[tokio::test]
    async fn source_ip_keeps_one_client_on_one_member_over_http() {
        let addrs = vec![
            speaker(b"one").await,
            speaker(b"two").await,
            speaker(b"thr").await,
        ];
        let p = RelayL7Provider::new();
        let lb = balancer(None);
        let realized = p.ensure_balancer(&lb).await.unwrap();
        let view = algorithm_view(
            &lb,
            ListenerProtocol::Http,
            addrs,
            vec![1, 1, 1],
            Algorithm::SourceIp,
        );
        p.ensure_listener(&view).await.unwrap();
        // Every request here arrives from 127.0.0.1, so all of them must land
        // on the one member that address hashes to. Against three members,
        // plain round robin would answer `one` three times in nine.
        let seen = ask_n(&realized.provider_ref, 9).await;
        assert_eq!(
            tally(&seen).len(),
            1,
            "one client address moved between members: {seen:?}"
        );
        p.delete_balancer(&lb.meta.id).await.unwrap();
    }

    #[tokio::test]
    async fn an_unhealthy_member_is_never_picked_over_http() {
        let addrs = [speaker(b"one").await, speaker(b"two").await];
        for algo in [
            Algorithm::RoundRobin,
            Algorithm::LeastConn,
            Algorithm::SourceIp,
        ] {
            let p = RelayL7Provider::new();
            let lb = balancer(None);
            let realized = p.ensure_balancer(&lb).await.unwrap();
            // The second member answers, but is Offline, so the reconciler
            // never hands it to the proxy at all.
            let view = algorithm_view(&lb, ListenerProtocol::Http, vec![addrs[0]], vec![1], algo);
            p.ensure_listener(&view).await.unwrap();
            for body in ask_n(&realized.provider_ref, 4).await {
                assert_eq!(body, b"one", "{algo}: the offline member took traffic");
            }
            p.delete_balancer(&lb.meta.id).await.unwrap();
        }
    }
}
