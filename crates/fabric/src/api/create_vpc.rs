// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Extension, State},
    response::Response,
    Json,
};
use pistack_types::{kind, ApiResult, Caller};

use super::create::Create;
use super::create_project::create_project;
use super::created::created;
use super::guarded::Guarded;
use super::new_meta::new_meta;
use crate::types::VpcSpec;

pub(crate) async fn create_vpc(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Create<VpcSpec>>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    body.spec.guard(&caller, None)?;
    let meta = new_meta(kind::VPC, &body, project)?;
    Ok(created(c.create_vpc(meta, body.spec)?))
}
