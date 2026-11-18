// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use relay::PathMatch;
use std::net::SocketAddr;

/// A route the realizer wants on an ingress listener, before its pool exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoutePlan {
    pub(crate) host: Option<String>,
    pub(crate) path: String,
    pub(crate) path_match: PathMatch,
    pub(crate) members: Vec<SocketAddr>,
}
