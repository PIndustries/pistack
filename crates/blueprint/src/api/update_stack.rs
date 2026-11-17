// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Update a stack's metadata or spec.

use super::if_match::if_match;
use super::list_project::list_project;
use super::update_stack_body::UpdateStack;
use super::visible::visible;
use crate::client::Client;
use crate::types::Stack;
use axum::{
    extract::{Extension, Path, State},
    http::HeaderMap,
    Json,
};
use pistack_types::{ApiResult, Caller};

pub(super) async fn update_stack(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateStack>,
) -> ApiResult<Json<Stack>> {
    let expected = if_match(&headers)?;
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_stack(project.as_ref(), &id)?)?;
    Ok(Json(client.update_stack(&cur.meta.id, expected, |s| {
        if let Some(d) = &body.description {
            s.meta.description = d.clone();
        }
        if let Some(l) = &body.labels {
            s.meta.labels = l.clone();
        }
        if let Some(a) = &body.annotations {
            s.meta.annotations = a.clone();
        }
        if let Some(spec) = &body.spec {
            s.spec = spec.clone();
        }
        Ok(())
    })?))
}
