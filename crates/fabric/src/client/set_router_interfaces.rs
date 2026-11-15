// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};
use std::collections::BTreeSet;
impl Client {
    /// Replace the router's interface set. Adds a port holding each new
    /// subnet's gateway address, removes ports for subnets dropped.
    pub fn set_router_interfaces(
        &self,
        id: &Id,
        expected: Option<u64>,
        subnets: &[Id],
    ) -> ApiResult<Router> {
        let router = self.routers().get(id).map_err(store_err)?;
        owned_by_vpc(&router)?;
        if let Some(exp) = expected {
            if exp != router.meta.revision {
                return Err(ApiError::conflict(format!(
                    "resource changed: expected revision {exp}, found {}",
                    router.meta.revision
                )));
            }
        }
        let want: BTreeSet<Id> = subnets.iter().cloned().collect();
        let have: BTreeSet<Id> = router.spec.interfaces.iter().cloned().collect();
        let ports = self.ports_of_device(id)?;

        for sid in want.difference(&have) {
            let sub = self.subnets().get(sid).map_err(store_err)?;
            check_same_project(&router.meta, &sub.meta, "subnet")?;
            let gw = sub.spec.gateway.ok_or_else(|| {
                ApiError::bad_request(format!(
                    "subnet {sid} has no gateway; cannot attach a router"
                ))
            })?;
            // One router per subnet: its gateway address can only be in one place.
            for other in self.routers().list(None).map_err(store_err)? {
                if other.meta.id != router.meta.id && other.spec.interfaces.contains(sid) {
                    return Err(ApiError::conflict(format!(
                        "subnet {sid} is already attached to router {}",
                        other.meta.id
                    )));
                }
            }
            let mut meta = Meta::new(
                kind::PORT,
                format!("{}-{}", router.meta.id, sid),
                router.meta.project.clone(),
            );
            meta.description = format!(
                "router {} interface on subnet {}",
                router.meta.name, sub.meta.name
            );
            let spec = PortSpec {
                network: sub.spec.network.clone(),
                subnet: Some(sid.clone()),
                fixed_ips: vec![gw],
                mac: String::new(),
                device: Some(router.meta.id.clone()),
                security_groups: Vec::new(),
                port_security: false,
                allowed_address_pairs: Vec::new(),
            };
            self.create_port_as_router(meta, spec)?;
        }
        for sid in have.difference(&want) {
            for p in ports.iter().filter(|p| p.spec.subnet.as_ref() == Some(sid)) {
                self.delete_port_unchecked(p)?;
            }
        }
        let mut ordered: Vec<Id> = Vec::new();
        for s in subnets {
            if !ordered.contains(s) {
                ordered.push(s.clone());
            }
        }
        self.routers()
            .modify(id, |r| {
                r.spec.interfaces = ordered.clone();
                Ok(())
            })
            .map_err(store_err)
    }
}
