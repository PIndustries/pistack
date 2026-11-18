// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;

use axum::Router;
use pistack_store::Store;
use tokio_util::sync::CancellationToken;

use super::handle::handle;
use super::listen_from_env::listen_from_env;

/// Start the metadata server. A bad address or a taken port is logged and
/// this task returns. It does not stop the rest of forge.
pub async fn serve(store: Store, shutdown: CancellationToken) {
    let Some(addr) = listen_from_env() else {
        return;
    };
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(%addr, error = %e, "forge: metadata listen failed");
            return;
        }
    };
    tracing::info!(%addr, "forge: metadata listening");
    let app = Router::new().fallback(handle).with_state(store);
    let svc = app.into_make_service_with_connect_info::<SocketAddr>();
    let server = axum::serve(listener, svc)
        .with_graceful_shutdown(async move { shutdown.cancelled().await });
    if let Err(e) = server.await {
        tracing::error!(error = %e, "forge: metadata server stopped");
    }
}
