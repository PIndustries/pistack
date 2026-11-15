// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};
use std::net::IpAddr;
impl Client {
    pub(in crate::client) fn ensure_machine_port(
        &self,
        fip: &Meta,
        machine: &Id,
        addr: IpAddr,
        machine_name: &str,
    ) -> ApiResult<Id> {
        let ports = self.ports().list(None).map_err(store_err)?;
        if let Some(existing) = ports
            .iter()
            .find(|p| p.meta.project == fip.project && p.spec.fixed_ips.contains(&addr))
        {
            if existing.is_router_port() {
                return Err(ApiError::conflict(format!(
                    "{addr} is held by router port {}",
                    existing.meta.id
                )));
            }
            if existing.spec.device.as_ref().is_some_and(|d| d != machine) {
                return Err(ApiError::conflict(format!(
                    "{addr} is held by port {} on another machine",
                    existing.meta.id
                )));
            }
            return Ok(existing.meta.id.clone());
        }

        let Some(sub) = self.subnet_containing(fip, addr)? else {
            return Err(ApiError::unprocessable(format!(
                "no subnet contains {addr}"
            )));
        };
        let name = if machine_name.is_empty() {
            machine.to_string()
        } else {
            format!("{machine_name}-{addr}")
        };
        let port = self.create_port(
            Meta::new(kind::PORT, name, fip.project.clone()),
            PortSpec {
                network: sub.spec.network.clone(),
                subnet: Some(sub.meta.id.clone()),
                fixed_ips: vec![addr],
                device: Some(machine.clone()),
                ..PortSpec::default()
            },
        )?;
        Ok(port.meta.id)
    }
}
