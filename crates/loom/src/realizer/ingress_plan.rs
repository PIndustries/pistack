// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Ingress, PathType};
use pistack_types::{ApiError, ApiResult};
use relay::{Client, PathMatch};
use std::net::SocketAddr;

use crate::realizer::one_backend::one_backend;
use crate::realizer::route_plan::RoutePlan;

/// Default-backend members and one route per rule path, in spec order.
pub(crate) fn ingress_plan(
    client: &Client,
    ingress: &Ingress,
) -> ApiResult<(Vec<SocketAddr>, Vec<RoutePlan>)> {
    let mut routes = Vec::new();
    for rule in &ingress.spec.rules {
        let host = rule
            .host
            .as_deref()
            .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|h| !h.is_empty());
        for path in &rule.paths {
            routes.push(RoutePlan {
                host: host.clone(),
                path: if path.path.is_empty() {
                    "/".into()
                } else {
                    path.path.clone()
                },
                path_match: match path.path_type {
                    PathType::Prefix => PathMatch::Prefix,
                    PathType::Exact => PathMatch::Exact,
                },
                members: one_backend(client, ingress, &path.backend)?,
            });
        }
    }
    let default = match &ingress.spec.default_backend {
        Some(backend) => one_backend(client, ingress, backend)?,
        None => Vec::new(),
    };
    if default.is_empty() && routes.is_empty() {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    Ok((default, routes))
}
