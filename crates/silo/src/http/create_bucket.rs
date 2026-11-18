// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{
    extract::{Extension, State},
    http::StatusCode,
    Json,
};
use pistack_types::{ApiResult, Caller, Id, Meta, Spec};
use std::collections::BTreeMap;

use super::caller::caller;
use super::create_project::create_project;

#[derive(serde::Deserialize)]
pub(crate) struct CreateBucket {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) labels: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) annotations: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) project: Option<Id>,
    #[serde(default)]
    pub(crate) spec: BucketSpec,
}

pub(crate) async fn create_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Json(body): Json<CreateBucket>,
) -> ApiResult<(StatusCode, Json<Bucket>)> {
    let who = caller(ext)?;
    let project = create_project(&who, &body.project)?;
    let mut meta = Meta::new(BucketSpec::KIND, body.name, project);
    meta.description = body.description;
    meta.labels = body.labels;
    meta.annotations = body.annotations;
    let bucket = c.create_bucket(meta, body.spec)?;
    Ok((StatusCode::CREATED, Json(bucket)))
}
