// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use axum::{extract::State, Json};
use pistack_types::{ApiResult, Page};

use super::agent_certs::agent_certs;
use super::app_state::AppState;
use super::hosts::hosts;
use super::list::List;
use super::paginate::paginate;
use super::scope::Scope;

pub(crate) async fn list_hosts(
    State(s): State<AppState>,
    scope: Scope,
    List(q): List,
) -> ApiResult<Json<Page<Host>>> {
    scope.require_admin()?;
    let mut all = hosts(&s).list(None)?;
    let certs = agent_certs(&s);
    for h in &mut all {
        h.status.cert_expires = keep::agent_cert_expiry(&certs, &h.spec.node_name);
    }
    Ok(Json(paginate(all, &q)))
}
