// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::canonical_ip::canonical_ip;
use crate::overlay::checks::same_service::same_service;
use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::service_backend::ServiceBackend;
use crate::overlay::types::service_rule::ServiceRule;
use std::net::IpAddr;

pub(in crate::overlay) fn canonical_services(
    rules: &[ServiceRule],
) -> Result<Vec<ServiceRule>, PlanError> {
    let mut out: Vec<ServiceRule> = Vec::with_capacity(rules.len());
    for rule in rules {
        if rule.id.is_empty() {
            return Err(PlanError::Invalid("service rule has an empty id".into()));
        }
        let address = match canonical_ip("service", &rule.address)? {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(_) => {
                return Err(PlanError::Invalid(format!(
                    "service {} address {} is not ipv4",
                    rule.id, rule.address
                )))
            }
        };
        if rule.protocol != "tcp" && rule.protocol != "udp" {
            return Err(PlanError::Invalid(format!(
                "service {} protocol {} is not tcp or udp",
                rule.id, rule.protocol
            )));
        }
        if rule.port == 0 {
            return Err(PlanError::Invalid(format!("service {} port is 0", rule.id)));
        }
        let mut backends = Vec::with_capacity(rule.backends.len());
        for b in &rule.backends {
            let IpAddr::V4(ip) = canonical_ip("service backend", &b.address)? else {
                return Err(PlanError::Invalid(format!(
                    "service {} backend {} is not ipv4",
                    rule.id, b.address
                )));
            };
            if b.port == 0 {
                return Err(PlanError::Invalid(format!(
                    "service {} backend port is 0",
                    rule.id
                )));
            }
            let backend = ServiceBackend {
                address: ip.to_string(),
                port: b.port,
            };
            if !backends.contains(&backend) {
                backends.push(backend);
            }
        }
        let rule = ServiceRule {
            id: rule.id.clone(),
            address,
            protocol: rule.protocol.clone(),
            port: rule.port,
            backends,
        };
        if out.iter().any(|r| same_service(r, &rule)) {
            return Err(PlanError::Invalid(format!(
                "service {}:{}/{} is listed twice",
                rule.address, rule.port, rule.protocol
            )));
        }
        out.push(rule);
    }
    Ok(out)
}
