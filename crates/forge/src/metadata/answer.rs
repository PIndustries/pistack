// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

use axum::http::StatusCode;
use pistack_store::Store;

use super::lookup::lookup;
use super::respond::respond;

/// Body for one metadata path. Unknown peers and unknown paths are 404.
/// The request path is not logged. Secret payloads are not logged.
pub fn answer(store: &Store, peer: IpAddr, path: &str) -> (StatusCode, Vec<u8>) {
    match lookup(store, peer, None) {
        Some(guest) => respond(store, guest, path),
        None => (StatusCode::NOT_FOUND, Vec::new()),
    }
}
