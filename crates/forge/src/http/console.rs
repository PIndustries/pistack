// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::console_ticket::ConsoleTicket;
use super::load_machine::load_machine;
use super::scope::Scope;

pub(crate) async fn console(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<ConsoleTicket>> {
    let m = load_machine(&s, &scope, &id)?;
    let ticket = crate::console::mint_ticket(&m.meta.id.to_string());
    Ok(Json(ConsoleTicket {
        kind: "vnc",
        websocket: format!(
            "/compute/machines/{}/console/vnc?ticket={ticket}",
            m.meta.id
        ),
    }))
}
