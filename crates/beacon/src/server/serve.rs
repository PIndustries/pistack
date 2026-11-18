// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use hickory_server::Server;
use pistack_types::module::CancellationToken;
use pistack_types::Event;
use std::net::{IpAddr, SocketAddr};
use tokio::sync::{broadcast, mpsc};

use super::handler::Handler;
use super::secondary_cache::SecondaryCache;
use super::tcp_timeout::TCP_TIMEOUT;

/// Bind UDP and TCP on each address in `listens` and serve until `shutdown`
/// fires. Returns the bound addresses (useful when an entry has port 0).
/// The handler is invalidated on every zone/record event from `events`.
/// `refresh`, when given, is where an accepted NOTIFY leaves its zone.
pub async fn serve(
    client: Client,
    listens: &[SocketAddr],
    events: broadcast::Sender<Event>,
    shutdown: CancellationToken,
    secondaries: SecondaryCache,
    refresh: Option<mpsc::UnboundedSender<(String, Option<IpAddr>)>>,
) -> anyhow::Result<Vec<SocketAddr>> {
    if listens.is_empty() {
        anyhow::bail!("beacon: no DNS listen addresses");
    }
    let mut handler = Handler::new(client, secondaries);
    if let Some(tx) = refresh {
        handler = handler.with_refresh(tx);
    }
    handler.watch(events.subscribe(), shutdown.clone());

    let mut server = Server::new(handler);
    let mut bounds = Vec::with_capacity(listens.len());
    for listen in listens {
        let udp = tokio::net::UdpSocket::bind(listen).await?;
        let bound = udp.local_addr()?;
        let tcp = tokio::net::TcpListener::bind(bound).await?;
        server.register_socket(udp);
        server.register_listener(tcp, TCP_TIMEOUT, 4096);
        tracing::info!(addr = %bound, "beacon: authoritative dns listening (udp+tcp)");
        bounds.push(bound);
    }

    tokio::spawn(async move {
        shutdown.cancelled().await;
        if let Err(e) = server.shutdown_gracefully().await {
            tracing::warn!(error = %e, "beacon: dns server shutdown");
        }
        tracing::debug!("beacon dns server stopped");
    });
    Ok(bounds)
}
