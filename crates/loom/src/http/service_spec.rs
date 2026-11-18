// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};
use std::collections::{BTreeMap, BTreeSet};

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;

impl Api for ServiceSpec {
    type Status = ServiceStatus;
    const NOUN: &'static str = "services";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&Service>) -> ApiResult<()> {
        if self.ports.is_empty() {
            return Err(ApiError::bad_request("spec.ports must not be empty"));
        }
        let mut seen = BTreeSet::new();
        let mut node_ports = BTreeMap::new();
        for p in &self.ports {
            if p.port == 0 {
                return Err(ApiError::bad_request("port must be 1-65535"));
            }
            if !seen.insert((p.port, format!("{:?}", p.protocol))) {
                return Err(ApiError::bad_request(format!(
                    "port {} is listed twice",
                    p.port
                )));
            }
            if let Some(node_port) = p.node_port {
                if self.kind == ServiceKind::ClusterIp {
                    return Err(ApiError::bad_request(
                        "node_port needs kind NodePort or LoadBalancer",
                    ));
                }
                if !NODE_PORT_RANGE.contains(&node_port) {
                    return Err(ApiError::bad_request(format!(
                        "node_port {node_port} must be {}-{}",
                        NODE_PORT_RANGE.start(),
                        NODE_PORT_RANGE.end()
                    )));
                }
                // The tcp and udp sides of one port may share a node port.
                if node_ports
                    .insert(node_port, p.port)
                    .is_some_and(|had| had != p.port)
                {
                    return Err(ApiError::bad_request(format!(
                        "node_port {node_port} is listed twice"
                    )));
                }
            }
        }
        if let Some(ip) = &self.cluster_ip {
            ip.parse::<std::net::IpAddr>()
                .map_err(|_| ApiError::bad_request("spec.cluster_ip is not an IP address"))?;
            if let Some(cur) = current {
                if cur.status.cluster_ip.is_some() && cur.status.cluster_ip != self.cluster_ip {
                    return Err(ApiError::unprocessable(
                        "spec.cluster_ip cannot change once allocated",
                    ));
                }
            }
        }
        Ok(())
    }
}
