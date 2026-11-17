// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::Alarm;
use axum::{
    extract::{Extension, Path, Query, State},
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::update_alarm_body::UpdateAlarm;
use super::visible::visible;

pub(crate) async fn update_alarm(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAlarm>,
) -> ApiResult<Json<Alarm>> {
    let project = scope(&caller, &sq)?;
    visible(&caller, client.get_alarm(project.as_ref(), &id)?)?;
    let updated = client.update_alarm_fields(
        project.as_ref(),
        &id,
        body.description,
        body.labels,
        body.spec,
    )?;
    Ok(Json(updated))
}
