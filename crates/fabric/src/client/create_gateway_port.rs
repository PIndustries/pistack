// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::types::*;
use pistack_types::{kind, ApiError, ApiResult, Meta};
impl Client {
    pub(crate) fn create_gateway_port(&self, router: &Router, net: &Network) -> ApiResult<Port> {
        if self.subnets_of_network(&net.meta.id)?.is_empty() {
            return Err(ApiError::bad_request(format!(
                "external network {} has no subnet",
                net.meta.id
            )));
        }
        let mut meta = Meta::new(
            kind::PORT,
            format!("{}-gw", router.meta.id),
            router.meta.project.clone(),
        );
        meta.description = format!("router {} gateway on {}", router.meta.name, net.meta.name);
        let spec = PortSpec {
            network: net.meta.id.clone(),
            subnet: None,
            fixed_ips: Vec::new(),
            mac: String::new(),
            device: Some(router.meta.id.clone()),
            security_groups: Vec::new(),
            port_security: false,
            allowed_address_pairs: Vec::new(),
        };
        self.create_port_as_router(meta, spec)
    }
}
