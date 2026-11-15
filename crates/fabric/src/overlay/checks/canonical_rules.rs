// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::canonical_cidr::canonical_cidr;
use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::sg_rule::SgRule;

pub(in crate::overlay) fn canonical_rules(rules: &[SgRule]) -> Result<Vec<SgRule>, PlanError> {
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        if !matches!(rule.proto.as_str(), "tcp" | "udp" | "icmp" | "any") {
            return Err(PlanError::Invalid(format!(
                "security group proto {} is not tcp, udp, icmp, or any",
                rule.proto
            )));
        }
        if rule.port_min > rule.port_max {
            return Err(PlanError::Invalid(format!(
                "security group port {} > {}",
                rule.port_min, rule.port_max
            )));
        }
        let cidr = canonical_cidr("security group cidr", &rule.cidr)?;
        out.push(SgRule {
            proto: rule.proto.clone(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            cidr,
        });
    }
    Ok(out)
}
