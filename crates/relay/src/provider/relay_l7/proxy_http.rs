// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The HTTP proxy for one request: read the head, choose a member through
//! the table, rewrite the request onto it, and pump the body and the reply.

use super::choice::Choice;
use super::content_length::content_length;
use super::header_has::header_has;
use super::held::Held;
use super::not_found::not_found;
use super::opened::opened;
use super::read_headers::read_headers;
use super::refuse::refuse;
use super::rewrite_request::rewrite_request;
use super::route_key::route_key;
use super::served::Served;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(super) async fn proxy_http<S>(mut inbound: S, peer: SocketAddr, served: &Served)
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
