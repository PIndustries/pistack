// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Accepting on the listener's socket: one task per connection, bounded by
//! the listener's connection limit, served the way the slot says.

use super::proxy_http::proxy_http;
use super::refuse::refuse;
use super::served::Served;
use super::slot::Serve;
use super::splice::splice;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::net::TcpListener;

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
