// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::keypairs::keypairs;
use super::load_keypair::load_keypair;
use super::scope::Scope;

/// Forget the pair. A machine made with it keeps the public key it was given
/// at create, so nothing about a running machine changes.
pub(crate) async fn delete_keypair(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let kp = load_keypair(&s, &scope, &id)?;
    keypairs(&s).delete(&kp.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}
