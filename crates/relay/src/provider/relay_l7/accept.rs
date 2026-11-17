// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The accept loop and the per-connection serving: a byte splice for
//! `Tcp`/`Https`, a TLS handshake for `Terminated`, and the HTTP proxy.

use super::http::route_key;
use super::http::{header_has, read_headers, rewrite_request};
use super::slot::Serve;
use super::table::{closed, opened, weighed, Choice, Held};
use crate::picker::Picker;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What one HTTP listener hands to every accepted connection.
pub(super) struct Served {
    pub(super) members: Arc<parking_lot::Mutex<super::table::Table>>,
    pub(super) picker: Picker,
    pub(super) conns: Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
    pub(super) limit: Arc<AtomicUsize>,
    pub(super) inflight: Arc<AtomicUsize>,
    pub(super) mode: Arc<parking_lot::Mutex<Serve>>,
}

impl Served {
    /// The handles a listener's connections serve from, taken off the slot
    /// the moment its accept task starts.
    pub(super) fn for_view(slot: &super::slot::Slot) -> Arc<Self> {
        Arc::new(Served {
            members: slot.members.clone(),
            picker: slot.picker.clone(),
            conns: slot.conns.clone(),
            limit: slot.limit.clone(),
            inflight: slot.inflight.clone(),
            mode: slot.mode.clone(),
        })
    }
}

pub(super) async fn accept_loop(
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
    let need = super::http::content_length(&head).unwrap_or(0);
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
