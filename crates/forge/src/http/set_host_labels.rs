// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Host, SYSTEM_LABEL_PREFIX};
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;
use serde::Deserialize;
use std::collections::BTreeMap;

use super::app_state::AppState;
use super::body::Body;
use super::hosts::hosts;
use super::scope::Scope;
use super::validate_label::validate_label;

#[derive(Debug, Deserialize)]
pub struct SetHostLabels {
    pub labels: BTreeMap<String, String>,
}

/// Replace the labels an operator owns. Labels PiStack wrote itself, the
/// ones under `pistack.io/`, stay as they are.
pub(crate) async fn set_host_labels(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Body(body): Body<SetHostLabels>,
) -> ApiResult<Json<Host>> {
    scope.require_admin()?;
    for (k, v) in &body.labels {
        validate_label(k, v)?;
    }
    let host = hosts(&s).resolve(None, &id)?;
    let host = hosts(&s).modify(&host.meta.id, |h| {
        h.meta
            .labels
            .retain(|k, _| k.starts_with(SYSTEM_LABEL_PREFIX));
        h.meta.labels.extend(body.labels.clone());
        Ok(())
    })?;
    Ok(Json(host))
}
