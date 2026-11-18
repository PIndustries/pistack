// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{extract::Path, response::Response};
use pistack_types::ApiError;

/// The browser cannot send the bearer token on a WebSocket. The ticket from
/// [`console`] is the only credential, and it is checked before the upgrade.
pub(crate) async fn console_vnc(
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let ticket = crate::console::query_ticket(raw.as_deref())?;
    crate::console::check_ticket(&ticket, &id)?;
    let port = crate::console::display_port(&id).await?;
    Ok(ws.on_upgrade(move |socket| async move {
        if crate::console::take_ticket(&ticket, &id).is_err() {
            return;
        }
        crate::console::splice(socket, port).await;
    }))
}
