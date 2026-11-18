// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

use axum::http::StatusCode;
use pistack_store::Store;
use pistack_types::{kind, Id};

use super::host_named::host_named;
use super::lookup::lookup;
use super::respond::respond;

/// [`answer`] for a request that host `node` passed on from `network`.
/// The guest is the machine that holds `peer` on that network, and it must
/// be placed on that host: a host cannot read for a guest it does not run,
/// and the same address on another network is another guest.
pub fn answer_on(
    store: &Store,
    node: &str,
    network: &str,
    peer: IpAddr,
    path: &str,
) -> (StatusCode, Vec<u8>) {
    let not_found = (StatusCode::NOT_FOUND, Vec::new());
    let Some(network) = Id::parse(kind::NETWORK, network) else {
        return not_found;
    };
    let Ok(Some(host)) = host_named(store, node) else {
        return not_found;
    };
    match lookup(store, peer, Some((&network, &host))) {
        Some(guest) => respond(store, guest, path),
        None => not_found,
    }
}
