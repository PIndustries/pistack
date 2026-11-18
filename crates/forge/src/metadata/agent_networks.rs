// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::extract::{Path, State};
use axum::Json;
use pistack_rpc::MetadataNetwork;
use pistack_store::Store;
use pistack_types::ApiResult;

use super::host_networks::host_networks;

pub(crate) async fn agent_networks(
    State(store): State<Store>,
    Path(node): Path<String>,
    ext: axum::http::Extensions,
) -> ApiResult<Json<Vec<MetadataNetwork>>> {
    pistack_rpc::check_peer_node(&ext, &node)?;
    if !crate::ovn::enabled() {
        return Ok(Json(Vec::new()));
    }
    Ok(Json(host_networks(&store, &node)?))
}
