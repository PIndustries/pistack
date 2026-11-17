// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
};
use pistack_types::{ApiResult, Caller};

use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;

pub(crate) async fn delete_alarm(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = scope(&caller, &sq)?;
    let alarm = visible(&caller, client.get_alarm(project.as_ref(), &id)?)?;
    client.delete_alarm(&alarm.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}
