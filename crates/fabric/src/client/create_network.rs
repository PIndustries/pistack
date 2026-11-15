// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use crate::usage::{self, admitted};
use pistack_types::{kind, ApiResult, Id, Meta};
impl Client {
    pub fn create_network(&self, mut meta: Meta, spec: NetworkSpec) -> ApiResult<Network> {
        meta.id = Id::new(kind::NETWORK);
        validate_network(&spec)?;
        self.check_vpc_membership(&meta.project, &spec)?;
        if spec.auto_floating_ip {
            self.only_auto_fip_network(&meta.id)?;
        }
        let net = Network {
            meta,
            spec,
            status: NetworkStatus::default(),
        };
        let project = net.meta.project.clone();
        admitted(&self.store, project.as_ref(), usage::NETWORKS, || {
            self.networks().create(net).map_err(store_err)
        })
    }
}
