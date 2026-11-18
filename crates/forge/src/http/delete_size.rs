// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use pistack_types::{ApiError, ApiResult};

use super::app_state::AppState;
use super::machines::machines;
use super::scope::Scope;
use super::sizes::sizes;

pub(crate) async fn delete_size(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    scope.require_admin()?;
    let sz = sizes(&s).resolve(None, &id)?;
    let in_use = machines(&s)
        .list(None)?
        .iter()
        .any(|m| m.spec.size == sz.meta.name);
    if in_use {
        return Err(ApiError::conflict(format!(
            "size '{}' is used by existing machines",
            sz.meta.name
        )));
    }
    sizes(&s).delete(&sz.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}
