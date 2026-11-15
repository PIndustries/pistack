// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::ipam;
use crate::types::*;
use crate::usage::{self, admitted};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};
impl Client {
    /// `pending` is a subnet about to be created: fabric's own interface for
    /// it is made first, so the caller can take the subnet back whole if the
    /// interface cannot be made.
    pub(crate) fn port_row(
        &self,
        mut meta: Meta,
        mut spec: PortSpec,
        pending: Option<&Subnet>,
    ) -> ApiResult<Port> {
        meta.id = Id::new(kind::PORT);
        validate_pairs(&spec)?;
        let net = self.networks().get(&spec.network).map_err(store_err)?;
        check_same_project(&meta, &net.meta, "network")?;

        // Subnet: explicit, or the network's first. `pending_subnet` is the
        // one a vpc interface is made for a moment before its own row exists.
        let mut subnets = self.subnets_of_network(&spec.network)?;
        if let Some(p) = &pending {
            if !subnets.iter().any(|s| s.meta.id == p.meta.id) {
                subnets.push((*p).clone());
            }
        }
        let subnet = match &spec.subnet {
            Some(sid) => Some(
                subnets
                    .iter()
                    .find(|s| &s.meta.id == sid)
                    .cloned()
                    .ok_or_else(|| {
                        ApiError::bad_request(format!(
                            "subnet {sid} is not on network {}",
                            spec.network
                        ))
                    })?,
            ),
            None => subnets.first().cloned(),
        };
        spec.subnet = subnet.as_ref().map(|s| s.meta.id.clone());

        // Addresses.
        match &subnet {
            Some(sub) => {
                let used = self.used_addresses(sub)?;
                if spec.fixed_ips.is_empty() {
                    spec.fixed_ips.push(ipam::allocate(&sub.spec, &used)?);
                } else {
                    // A router's interface port legitimately holds the gateway.
                    self.validate_fixed_ips(
                        &subnets,
                        &spec.fixed_ips,
                        is_router_device(&spec.device),
                    )?;
                }
            }
            None if !spec.fixed_ips.is_empty() => {
                return Err(ApiError::bad_request(
                    "network has no subnet; fixed_ips must be empty",
                ));
            }
            None => {}
        }

        // MAC.
        let macs = self.macs_on_network(&spec.network)?;
        if spec.mac.is_empty() {
            spec.mac = ipam::generate_mac(&meta.id.uuid, &macs)?;
        } else {
            spec.mac = ipam::format_mac(ipam::parse_mac(&spec.mac)?);
            if macs.contains(&spec.mac) {
                return Err(ApiError::conflict(format!(
                    "mac {} is already in use on this network",
                    spec.mac
                )));
            }
        }

        // Security groups.
        if spec.port_security && spec.security_groups.is_empty() && !is_router_device(&spec.device)
        {
            if let Some(p) = &meta.project {
                spec.security_groups
                    .push(self.default_security_group(p)?.meta.id);
            }
        }
        if !spec.port_security {
            spec.security_groups.clear();
        }
        self.validate_security_groups(&meta, &spec.security_groups)?;

        // Gateway and interface ports belong to the router, not the tenant quota.
        let counted = !is_router_device(&spec.device);
        let port = Port {
            meta,
            spec,
            status: PortStatus::default(),
        };
        if !counted {
            return self.ports().create(port).map_err(store_err);
        }
        let project = port.meta.project.clone();
        admitted(&self.store, project.as_ref(), usage::PORTS, || {
            self.ports().create(port).map_err(store_err)
        })
    }
}
