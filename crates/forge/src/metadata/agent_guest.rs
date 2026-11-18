// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::extract::State;
use axum::Json;
use pistack_rpc::{MetadataAnswer, MetadataQuery};
use pistack_store::Store;
use pistack_types::ApiResult;

use super::answer_on::answer_on;

pub(crate) async fn agent_guest(
    State(store): State<Store>,
    ext: axum::http::Extensions,
    Json(q): Json<MetadataQuery>,
) -> ApiResult<Json<MetadataAnswer>> {
    pistack_rpc::check_peer_node(&ext, &q.node_name)?;
    let (status, body) = answer_on(&store, &q.node_name, &q.network, q.address, &q.path);
    Ok(Json(MetadataAnswer {
        status: status.as_u16(),
        body,
    }))
}
