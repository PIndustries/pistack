// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

use super::announce::Announce;
use super::discover_error::DiscoverError;
use super::port_unreachable::port_unreachable;
use super::send_datagram::send_datagram;

/// Bind `ip:port`, send one announce to `dest`, and return announces heard
/// on this socket within `wait`, skipping `self_id`.
pub async fn collect_announcements(
    ip: IpAddr,
    port: u16,
    dest: SocketAddr,
    mine: &Announce,
    wait: Duration,
    self_id: &str,
) -> Result<Vec<Announce>, DiscoverError> {
    let sock = UdpSocket::bind(SocketAddr::new(ip, port))
        .await
        .map_err(|e| DiscoverError::Io(e.to_string()))?;
    let payload = serde_json::to_vec(mine).map_err(|e| DiscoverError::Io(e.to_string()))?;
    // Immediate announce, then again at the halfway mark so a peer that binds
    // a moment later still hears it.
    send_datagram(&sock, &payload, dest).await?;

    let start = tokio::time::Instant::now();
    let deadline = start + wait;
    let resend_at = start + wait / 2;
    let mut resent = false;
    let mut heard = Vec::new();
    let mut buf = [0u8; 8192];

    while tokio::time::Instant::now() < deadline {
        let now = tokio::time::Instant::now();
        if !resent && now >= resend_at {
            send_datagram(&sock, &payload, dest).await?;
            resent = true;
            continue;
        }
        let wake = if resent { deadline } else { resend_at };
        let pause = wake.saturating_duration_since(now);
        if pause.is_zero() {
            continue;
        }
        match tokio::time::timeout(pause, sock.recv_from(&mut buf)).await {
            Err(_) => {}
            Ok(Ok((n, _))) => {
                let Ok(ann) = serde_json::from_slice::<Announce>(&buf[..n]) else {
                    continue;
                };
                if ann.node_id != self_id && !heard.contains(&ann) {
                    heard.push(ann);
                }
            }
            Ok(Err(e)) if port_unreachable(&e) => {}
            Ok(Err(e)) => return Err(DiscoverError::Io(e.to_string())),
        }
    }
    Ok(heard)
}
