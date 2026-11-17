// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Opening one client's socket: bound on an address this forwarder is
//! allowed to hold, connected to the chosen member, with a task carrying
//! that member's replies back to the client.

use super::session::Session;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

pub(super) async fn open_session(
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
