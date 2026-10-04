// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! UDP datagram copy. One socket per listener, on the balancer's bind
//! address. A client stays on one up member until that member leaves the
//! pool. An empty member list does not become a running forwarder.

use crate::provider::ListenerView;
use crate::types::{Member, OperatingStatus};
use pistack_types::{ApiError, ApiResult};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// A bound UDP forwarder. Drop cancels the copy task.
pub struct UdpForward {
    bound: SocketAddr,
    members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl UdpForward {
    /// Bind `ip` on an ephemeral port and copy datagrams to `members`.
    /// `members` must be non-empty. `ip` must be loopback or private.
    pub async fn bind(ip: IpAddr, members: Vec<SocketAddr>) -> ApiResult<Self> {
        if members.is_empty() {
            return Err(ApiError::unavailable("udp pool has no up member"));
        }
        if !crate::bind::allowed(ip) {
            return Err(ApiError::bad_request(format!(
                "udp refuses {ip}; only a loopback or private address can be bound"
            )));
        }
        let socket = UdpSocket::bind(SocketAddr::new(ip, 0))
            .await
            .map_err(|e| ApiError::unavailable(format!("udp bind {ip}: {e}")))?;
        let bound = socket
            .local_addr()
            .map_err(|e| ApiError::unavailable(format!("udp local address: {e}")))?;
        if !crate::bind::allowed(bound.ip()) {
            return Err(ApiError::unavailable(format!(
                "udp bound {bound}, which is not a loopback or private address"
            )));
        }
        let members = Arc::new(parking_lot::Mutex::new(members));
        let cancel = CancellationToken::new();
        let cursor = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(run(
            Arc::new(socket),
            members.clone(),
            cursor,
            cancel.clone(),
            ip,
        ));
        Ok(Self {
            bound,
            members,
            cancel,
            task: Some(task),
        })
    }

    pub fn bound(&self) -> SocketAddr {
        self.bound
    }

    pub fn set_members(&self, members: Vec<SocketAddr>) {
        *self.members.lock() = members;
    }

    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for UdpForward {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Up members that should take datagrams. Backup members are used only when
/// no primary is up. Administratively down, zero-weight, offline, and
/// draining members are left out.
pub fn targets(view: &ListenerView) -> Vec<SocketAddr> {
    if !view.listener.spec.admin_up {
        return Vec::new();
    }
    let Some((_, members)) = &view.pool else {
        return Vec::new();
    };
    let primary: Vec<SocketAddr> = members
        .iter()
        .filter(|m| is_up(m) && !m.spec.backup)
        .map(|m| m.addr())
        .collect();
    if !primary.is_empty() {
        return primary;
    }
    members
        .iter()
        .filter(|m| is_up(m))
        .map(|m| m.addr())
        .collect()
}

fn is_up(member: &Member) -> bool {
    member.spec.admin_up
        && member.spec.weight > 0
        && matches!(
            member.status.operating_status,
            OperatingStatus::Online | OperatingStatus::NoMonitor
        )
}

struct Session {
    dest: SocketAddr,
    sock: Arc<UdpSocket>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

async fn run(
    listen: Arc<UdpSocket>,
    members: Arc<parking_lot::Mutex<Vec<SocketAddr>>>,
    cursor: Arc<AtomicUsize>,
    cancel: CancellationToken,
    bind_ip: IpAddr,
) {
    let mut buf = vec![0u8; 65535];
    let mut sessions: HashMap<SocketAddr, Session> = HashMap::new();
    loop {
        let (n, peer) = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            recvd = listen.recv_from(&mut buf) => match recvd {
                Ok((n, peer)) if n > 0 => (n, peer),
                _ => break,
            },
        };
        let list = members.lock().clone();
        if list.is_empty() {
            continue;
        }
        let pinned = sessions.get(&peer).map(|session| session.dest);
        let dest = match pinned {
            Some(dest) if list.contains(&dest) => dest,
            _ => pick(&list, &cursor),
        };
        let fresh = sessions.get(&peer).map(|session| session.dest) != Some(dest);
        if fresh {
            let Some(session) = open_session(bind_ip, dest, peer, listen.clone(), cancel.clone()).await
            else {
                continue;
            };
            sessions.insert(peer, session);
        }
        let sent = sessions
            .get(&peer)
            .unwrap()
            .sock
            .send(&buf[..n])
            .await
            .is_ok();
        if !sent {
            sessions.remove(&peer);
        }
    }
}

fn pick(members: &[SocketAddr], cursor: &AtomicUsize) -> SocketAddr {
    let n = cursor.fetch_add(1, Ordering::Relaxed);
    members[n % members.len()]
}

async fn open_session(
    bind_ip: IpAddr,
    dest: SocketAddr,
    peer: SocketAddr,
    listen: Arc<UdpSocket>,
    parent: CancellationToken,
) -> Option<Session> {
    if !crate::bind::allowed(bind_ip) {
        return None;
    }
    let sock = UdpSocket::bind(SocketAddr::new(bind_ip, 0)).await.ok()?;
    if let Ok(local) = sock.local_addr() {
        if !crate::bind::allowed(local.ip()) {
            return None;
        }
    }
    sock.connect(dest).await.ok()?;
    let sock = Arc::new(sock);
    let cancel = parent.child_token();
    let reply = sock.clone();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let n = tokio::select! {
                biased;
                _ = task_cancel.cancelled() => break,
                recvd = reply.recv(&mut buf) => match recvd {
                    Ok(n) if n > 0 => n,
                    _ => break,
                },
            };
            if listen.send_to(&buf[..n], peer).await.is_err() {
                break;
            }
        }
    });
    Some(Session {
        dest,
        sock,
        cancel,
        task,
    })
}
