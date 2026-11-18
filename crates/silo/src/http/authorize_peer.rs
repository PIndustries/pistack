// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::http::HeaderMap;
use pistack_types::{ApiError, ApiResult};

/// `required` is the value of `PISTACK_REQUIRE_JOIN`. Only `0` lets a
/// caller through without the peer key. A join token is not enough here:
/// a server that is still joining has no object records to ask about.
pub(crate) fn authorize_peer(
    store: &pistack_store::Store,
    headers: &HeaderMap,
    required: Option<&str>,
) -> ApiResult<()> {
    if required.map(str::trim) == Some("0") {
        return Ok(());
    }
    match headers.get(keep::peer::PEER_HEADER) {
        Some(offered) if keep::peer::is_member(store, offered.as_bytes()) => Ok(()),
        Some(_) => Err(ApiError::unauthorized("not a member of this cluster")),
        None => Err(ApiError::unauthorized(
            "object bytes are served only to a member of this cluster",
        )),
    }
}
