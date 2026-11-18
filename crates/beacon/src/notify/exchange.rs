// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hickory_proto::op::{Message, OpCode, ResponseCode};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::ack_timeout::ACK_TIMEOUT;

/// One UDP exchange: send this NOTIFY, await the one answer. The NOTIFY is put
/// on the wire exactly as encoded — its own id, its opcode — so a plain
/// `UdpSocket` is enough and nothing rewrites the message on the way out.
pub(crate) async fn exchange(bytes: &[u8], to: SocketAddr) -> anyhow::Result<ResponseCode> {
    // Bound to the family the peer is in: macOS refuses a v4 destination from
    // an AF_INET6 socket, so one dual-stack socket for both is not available.
    let local = match to.ip() {
        IpAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        IpAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
    };
    let sock = tokio::net::UdpSocket::bind(local).await?;
    let mut buf = [0u8; 4096];
    let id = u16::from_be_bytes([bytes[0], bytes[1]]);
    {
        sock.send_to(bytes, to).await?;
        loop {
            let (n, from) = tokio::time::timeout(ACK_TIMEOUT, sock.recv_from(&mut buf))
                .await
                .map_err(|_| anyhow::anyhow!("{to} did not answer inside {ACK_TIMEOUT:?}"))??;
            if from != to {
                continue;
            }
            let msg = Message::from_vec(&buf[..n])?;
            if msg.id != id {
                continue;
            }
            if msg.metadata.op_code != OpCode::Notify {
                anyhow::bail!("{to} answered a {:?}", msg.metadata.op_code);
            }
            return Ok(msg.metadata.response_code);
        }
    }
}
