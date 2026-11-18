// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use pistack_types::ApiResult;

use crate::http::api::Api;
use crate::http::app_state::AppState;
use crate::http::col::col;
use crate::http::delete_mode::DeleteMode;
use crate::http::load::load;
use crate::http::scope::Scope;

pub(crate) async fn delete<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let r = load::<A>(&s, &scope, &id)?;
    match A::DELETE {
        DeleteMode::Direct => {
            col::<A>(&s).delete(&r.meta.id)?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        DeleteMode::Annotate => {
            if delete_requested(&r) {
                return Ok((StatusCode::ACCEPTED, Json(r)).into_response());
            }
            let r = col::<A>(&s).modify(&r.meta.id, |x| {
                x.meta
                    .annotations
                    .insert(DELETE_ANNOTATION.into(), Utc::now().to_rfc3339());
                Ok(())
            })?;
            Ok((StatusCode::ACCEPTED, Json(r)).into_response())
        }
    }
}
