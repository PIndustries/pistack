// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{FromRequestParts, State},
    http::request::Parts,
    Json,
};
use pistack_types::{ApiError, ApiResult, ListQuery, Page};

use crate::http::api::Api;
use crate::http::app_state::AppState;
use crate::http::col::col;
use crate::http::paginate::paginate;
use crate::http::parse_query::parse_query;
use crate::http::res::Res;
use crate::http::scope::Scope;

/// `Query<ListQuery>` whose rejection is an `ApiError`.
pub struct List(pub ListQuery);

impl<S: Send + Sync> FromRequestParts<S> for List {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(List(parse_query(parts.uri.query().unwrap_or(""))?))
    }
}

pub(crate) async fn list<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    List(q): List,
) -> ApiResult<Json<Page<Res<A>>>> {
    let items = col::<A>(&s).list(scope.filter())?;
    Ok(Json(paginate(items, &q)))
}
