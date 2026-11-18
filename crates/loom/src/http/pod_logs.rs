// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
};
use pistack_rpc::proto::CommandKind;
use pistack_types::{ApiError, ApiResult};

use crate::http::app_state::AppState;
use crate::http::load::load;
use crate::http::scope::Scope;
use crate::types::PodSpec;

pub(crate) async fn pod_logs(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let pod = load::<PodSpec>(&s, &scope, &id)?;
    let machine = pod
        .status
        .machine
        .clone()
        .ok_or_else(|| ApiError::unavailable("pod logs are not available"))?;
    let host_id = pod
        .status
        .host
        .clone()
        .ok_or_else(|| ApiError::unavailable("pod logs are not available"))?;
    let host = s
        .store
        .collection::<forge::Host>()
        .get(&host_id)
        .map_err(|_| ApiError::unavailable("pod logs are not available"))?;
    if host.spec.node_name.is_empty() {
        return Err(ApiError::unavailable("pod logs are not available"));
    }
    let queue = s
        .queue
        .clone()
        .ok_or_else(|| ApiError::unavailable("pod logs are not configured"))?;
    let ticket = queue.enqueue(
        &host.spec.node_name,
        CommandKind::ReadLogs,
        serde_json::json!({"id": machine.to_string()}),
    )?;
    let value = ticket.wait().await?.into_api_result()?;
    let text = match value.get("text").and_then(|v| v.as_str()) {
        Some(text) => text.to_owned(),
        None => return Err(ApiError::unavailable("pod log text is missing")),
    };
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response())
}
