// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::ipam;
use crate::types::*;
use crate::usage::{self, admitted, released};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};
impl Client {
    pub fn create_subnet(&self, mut meta: Meta, spec: SubnetSpec) -> ApiResult<Subnet> {
        meta.id = Id::new(kind::SUBNET);
        let net = self.networks().get(&spec.network).map_err(store_err)?;
        check_same_project(&meta, &net.meta, "network")?;
        ipam::validate_subnet(&spec)?;
        for other in self.subnets_of_network(&spec.network)? {
            if overlaps(spec.cidr, other.spec.cidr) {
                return Err(ApiError::conflict(format!(
                    "cidr {} overlaps subnet {} ({})",
                    spec.cidr, other.meta.name, other.spec.cidr
                )));
            }
        }
        let sub = Subnet {
            meta,
            spec,
            status: SubnetStatus::default(),
        };
        self.check_vpc_subnet(&sub)?;
        let vpc = self.vpc_of_network(&net.meta.id);
        if vpc.is_some() {
            self.vpc_joinable(&net, &sub)?;
        }
        let project = sub.meta.project.clone();
        let made = admitted(&self.store, project.as_ref(), usage::SUBNETS, || {
            self.subnets().create(sub.clone()).map_err(store_err)
        });
        match made {
            Ok(s) => {
                // The router interface is part of the subnet's own create.
                // It is taken back if anything below it refuses.
                if let Some(v) = vpc {
                    if let Err(e) = self.vpc_attach(&v, &s) {
                        let _ = self.subnets().delete(&s.meta.id);
                        released(&self.store, project.as_ref(), usage::SUBNETS);
                        return Err(e);
                    }
                    self.count_vpc_subnets(&v.meta.id);
                }
                Ok(s)
            }
            Err(e) => Err(e),
        }
    }
}
