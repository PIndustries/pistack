// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::ipam;
use crate::types::*;
use crate::usage::{self, admitted};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};
impl Client {
    pub fn create_floating_ip(
        &self,
        mut meta: Meta,
        mut spec: FloatingIpSpec,
    ) -> ApiResult<FloatingIp> {
        meta.id = Id::new(kind::FLOATING_IP);
        let net = self.external_network(&spec.network)?;
        let subnets = self.subnets_of_network(&net.meta.id)?;
        if subnets.is_empty() {
            return Err(ApiError::bad_request(format!(
                "external network {} has no subnet",
                net.meta.id
            )));
        }
        match spec.address {
            Some(a) => self.validate_fixed_ips(&subnets, std::slice::from_ref(&a), false)?,
            None => {
                let mut picked = None;
                for sub in &subnets {
                    let used = self.used_addresses(sub)?;
                    if let Ok(ip) = ipam::allocate(&sub.spec, &used) {
                        picked = Some(ip);
                        break;
                    }
                }
                spec.address = Some(picked.ok_or_else(|| {
                    ApiError::conflict("no free floating ips")
                        .with_detail("network", net.meta.id.to_string())
                })?);
            }
        }
        if let Some(p) = &spec.port {
            self.validate_fip_target(&meta, &meta.id, p)?;
        }
        let project = meta.project.clone();
        admitted(&self.store, project.as_ref(), usage::FLOATING_IPS, || {
            self.floating_ips()
                .create(FloatingIp {
                    meta,
                    spec,
                    status: FloatingIpStatus::default(),
                })
                .map_err(store_err)
        })
    }
}
