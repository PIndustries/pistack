// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{kinds, Ingress, IngressBackend, Protocol, Service};
use pistack_types::{ApiError, ApiResult, Id};
use relay::Client;
use std::net::SocketAddr;

use crate::realizer::endpoint_addrs::endpoint_addrs;
use crate::realizer::port_matches::port_matches;
use crate::realizer::projects_match::projects_match;

pub(crate) fn one_backend(
    client: &Client,
    ingress: &Ingress,
    backend: &IngressBackend,
) -> ApiResult<Vec<SocketAddr>> {
    let services = client.store().collection::<Service>();
    let svc = if let Some(id) = Id::parse(kinds::SERVICE, &backend.service) {
        services
            .get(&id)
            .map_err(|_| ApiError::unavailable("ingress backend is not available"))?
    } else {
        services
            .get_by_name(ingress.meta.project.as_ref(), &backend.service)
            .map_err(|_| ApiError::unavailable("ingress backend is not available"))?
    };
    if !projects_match(&ingress.meta.project, &svc.meta.project) {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    let Some(port) = svc
        .spec
        .ports
        .iter()
        .find(|p| port_matches(p, &backend.port))
    else {
        return Err(ApiError::unavailable("ingress backend is not available"));
    };
    if port.protocol != Protocol::Tcp {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    let addrs = endpoint_addrs(&svc.status.endpoints, port)?;
    if addrs.is_empty() {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    Ok(addrs)
}
