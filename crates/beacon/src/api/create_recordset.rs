// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::{
    extract::{Extension, State},
    response::Response,
    Json,
};
use pistack_types::{kind, ApiResult, Caller};

use super::apex_of::apex_of;
use super::api_state::ApiState;
use super::asked::asked;
use super::create::Create;
use super::create_project::create_project;
use super::created::created;
use super::new_meta::new_meta;

pub(crate) async fn create_recordset(
    State(st): State<ApiState>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Create<RecordSetSpec>>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(kind::RECORD, &body, project)?;
    let apex = apex_of(&st, &body.spec.zone).await;
    let r = st.client.create_recordset(meta, body.spec)?;
    asked(&st, &apex);
    Ok(created(r))
}
