// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use pistack_store::Store;

use super::answer::answer;

pub(crate) async fn handle(
    State(store): State<Store>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
) -> Response {
    let (code, body) = answer(&store, peer.ip(), req.uri().path());
    (
        code,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}
