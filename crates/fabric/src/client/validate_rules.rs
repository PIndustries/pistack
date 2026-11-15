// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use ipnet::IpNet;
use pistack_types::{ApiError, ApiResult, Id, Meta};
use std::collections::BTreeSet;
impl Client {
    pub(in crate::client) fn validate_rules(
        &self,
        meta: &Meta,
        rules: &[Rule],
        self_id: Option<&Id>,
    ) -> ApiResult<()> {
        let mut ids = BTreeSet::new();
        for r in rules {
            if !ids.insert(r.id) {
                return Err(ApiError::bad_request(format!("duplicate rule id {}", r.id)));
            }
            if let Some(pr) = r.port_range {
                if pr.min > pr.max {
                    return Err(ApiError::bad_request(format!(
                        "port range {}-{} is inverted",
                        pr.min, pr.max
                    )));
                }
                if !matches!(r.protocol, Some(Protocol::Tcp) | Some(Protocol::Udp)) {
                    return Err(ApiError::bad_request(
                        "port_range requires protocol tcp or udp",
                    ));
                }
            }
            match &r.remote {
                Remote::Any => {}
                Remote::Cidr(c) => {
                    let v4 = matches!(c, IpNet::V4(_));
                    if v4 != (r.ethertype == EtherType::IPv4) {
                        return Err(ApiError::bad_request(format!(
                            "remote cidr {c} does not match ethertype"
                        )));
                    }
                }
                Remote::Group(g) => {
                    if Some(g) != self_id {
                        let other = self.security_groups().get(g).map_err(store_err)?;
                        check_same_project(meta, &other.meta, "remote security group")?;
                    }
                }
            }
        }
        Ok(())
    }
}
