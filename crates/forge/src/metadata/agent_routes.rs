// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::routing::{get, post};
use axum::Router;
use pistack_store::Store;

use super::agent_guest::agent_guest;
use super::agent_networks::agent_networks;

/// Routes for host agents, to sit beside the rest of `/rpc` behind the
/// same checks.
///
/// * `GET /metadata/networks/{node}`: the networks with a guest on that
///   host, empty unless the networks are OVN.
/// * `POST /metadata/guest`: one guest's metadata request, passed on by
///   the host it runs on.
pub fn agent_routes(store: Store) -> Router {
    Router::new()
        .route("/metadata/networks/{node}", get(agent_networks))
        .route("/metadata/guest", post(agent_guest))
        .with_state(store)
}
