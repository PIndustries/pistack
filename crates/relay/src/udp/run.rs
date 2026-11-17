// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Reading the listener's datagrams and routing them: a client keeps the
//! member it was first sent to while that member is still offered, and a
//! client whose member left is sent to the next one in turn.

use super::open_session::open_session;
use super::pick::pick;
use super::session::Session;
use std::collections::HashMap;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

pub(super) async fn run(
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
            let Some(session) =
                open_session(bind_ip, dest, peer, listen.clone(), cancel.clone()).await
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
