// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::Router;

use super::router_with::router_with;

/// Native + S3 routes with bearer auth only (no SigV4). Used by unit tests
/// that inject a [`Caller`] via request extensions.
pub fn router(client: Client) -> Router {
    router_with(client, None, DEFAULT_REGION.to_owned())
}
