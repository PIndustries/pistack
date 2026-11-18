// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;

use super::announce::Announce;
use super::discover_error::DiscoverError;
use super::port_unreachable::port_unreachable;
use super::send_datagram::send_datagram;

/// Bind `bind` once, send `mine` to every target, resend at half of `wait`,
/// and return announces whose node_id is not mine.node_id. Keep announces
/// with an empty cluster_id (a peer that is still forming). Skip bad JSON.
pub async fn sweep(
    bind: SocketAddr,
    targets: &[SocketAddr],
    mine: &Announce,
    wait: Duration,
) -> Result<Vec<Announce>, DiscoverError> {
    let sock = UdpSocket::bind(bind)
        .await
        .map_err(|err| DiscoverError::Io(err.to_string()))?;
    let payload = serde_json::to_vec(mine).map_err(|err| DiscoverError::Io(err.to_string()))?;
    for dest in targets {
        send_datagram(&sock, &payload, *dest).await?;
    }

    let start = tokio::time::Instant::now();
    let deadline = start + wait;
    let resend_at = start + wait / 2;
    let mut resent = false;
    let mut heard = Vec::new();
    let mut buf = [0u8; 8192];

    while tokio::time::Instant::now() < deadline {
        let now = tokio::time::Instant::now();
        if !resent && now >= resend_at {
            for dest in targets {
                send_datagram(&sock, &payload, *dest).await?;
            }
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
            Ok(Ok((len, _))) => {
                let Ok(ann) = serde_json::from_slice::<Announce>(&buf[..len]) else {
                    continue;
                };
                if ann.node_id != mine.node_id && !heard.contains(&ann) {
                    heard.push(ann);
                }
            }
            Ok(Err(err)) if port_unreachable(&err) => {}
            Ok(Err(err)) => return Err(DiscoverError::Io(err.to_string())),
        }
    }
    Ok(heard)
}
