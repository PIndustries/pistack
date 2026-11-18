// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;
use tokio::net::UdpSocket;

use super::discover_error::DiscoverError;
use super::port_unreachable::port_unreachable;

/// ICMP port-unreachable stays queued on the socket and the next send drops
/// its datagram. Retry once after consuming that error.
pub(crate) async fn send_datagram(
    sock: &UdpSocket,
    payload: &[u8],
    dest: SocketAddr,
) -> Result<(), DiscoverError> {
    match sock.send_to(payload, dest).await {
        Ok(_) => Ok(()),
        Err(e) if port_unreachable(&e) => match sock.send_to(payload, dest).await {
            Ok(_) => Ok(()),
            Err(e) if port_unreachable(&e) => Ok(()),
            Err(e) => Err(DiscoverError::Io(e.to_string())),
        },
        Err(e) => Err(DiscoverError::Io(e.to_string())),
    }
}
