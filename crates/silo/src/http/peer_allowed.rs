// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::http::HeaderMap;
use pistack_types::ApiResult;

use super::authorize_peer::authorize_peer;

pub(crate) fn peer_allowed(c: &Client, headers: &HeaderMap) -> ApiResult<()> {
    let required = std::env::var(pistack_rpc::REQUIRE_JOIN_ENV).ok();
    authorize_peer(c.store(), headers, required.as_deref())
}
