// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::DELETE_ANNOTATION;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::load_machine::load_machine;
use super::machines::machines;
use super::scope::Scope;

pub(crate) async fn delete_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let m = load_machine(&s, &scope, &id)?;
    if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
        return Ok((StatusCode::ACCEPTED, Json(m)).into_response());
    }
    let m = machines(&s).modify(&m.meta.id, |x| {
        x.meta
            .annotations
            .insert(DELETE_ANNOTATION.into(), Utc::now().to_rfc3339());
        Ok(())
    })?;
    crate::usage::note_delete(&s.store, m.meta.project.as_ref());
    Ok((StatusCode::ACCEPTED, Json(m)).into_response())
}
