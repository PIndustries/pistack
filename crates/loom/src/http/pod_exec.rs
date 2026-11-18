// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_rpc::proto::{exec_timeout, CommandKind, ExecOutput, ExecRequest};
use pistack_types::{ApiError, ApiResult};

use crate::http::app_state::AppState;
use crate::http::body::Body;
use crate::http::exec_body::ExecBody;
use crate::http::load::load;
use crate::http::scope::Scope;
use crate::types::PodSpec;

/// Run a command in one container of a pod and return its exit code and
/// output. A non-zero exit is still HTTP 200. Stdin and a terminal are a
/// later change; this call does not attach.
pub(crate) async fn pod_exec(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Body(body): Body<ExecBody>,
) -> ApiResult<Json<ExecOutput>> {
    let pod = load::<PodSpec>(&s, &scope, &id)?;
    if body.container.is_empty() {
        return Err(ApiError::bad_request("container name is empty"));
    }
    if body.command.is_empty() {
        return Err(ApiError::bad_request("command is empty"));
    }
    exec_timeout(body.timeout_secs)?;
    if !pod.spec.containers.iter().any(|c| c.name == body.container) {
        return Err(ApiError::not_found(format!(
            "container '{}' is not in this pod",
            body.container
        )));
    }
    let machine = pod
        .status
        .machine
        .clone()
        .ok_or_else(|| ApiError::unavailable("pod exec is not available"))?;
    let host_id = pod
        .status
        .host
        .clone()
        .ok_or_else(|| ApiError::unavailable("pod exec is not available"))?;
    let host = match s.store.collection::<forge::Host>().get(&host_id) {
        Ok(host) => host,
        Err(e) => {
            let api = ApiError::from(e);
            if api.code == pistack_types::error::ErrorCode::NotFound {
                return Err(api);
            }
            return Err(ApiError::unavailable("pod exec is not available"));
        }
    };
    if host.spec.node_name.is_empty() {
        return Err(ApiError::unavailable("pod exec is not available"));
    }
    let queue = s
        .queue
        .clone()
        .ok_or_else(|| ApiError::unavailable("pod exec is not configured"))?;
    let payload = serde_json::to_value(ExecRequest {
        id: machine,
        container: body.container,
        command: body.command,
        env: body.env,
        cwd: body.cwd,
        timeout_secs: body.timeout_secs,
    })?;
    let ticket = queue.enqueue(&host.spec.node_name, CommandKind::Exec, payload)?;
    let value = ticket.wait().await?.into_api_result()?;
    let out = serde_json::from_value(value)
        .map_err(|_| ApiError::unavailable("pod exec result is missing"))?;
    Ok(Json(out))
}
