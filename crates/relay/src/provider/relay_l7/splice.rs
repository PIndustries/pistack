// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The byte copy for a `Tcp` or `Https` listener: pick a member, dial it,
//! copy both ways.

use super::closed::closed;
use super::opened::opened;
use super::served::Served;
use super::weighed::weighed;
use tokio::net::TcpStream;

pub(super) async fn splice(mut inbound: TcpStream, peer: std::net::SocketAddr, served: &Served) {
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
