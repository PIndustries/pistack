// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::canonical_ip::canonical_ip;
use crate::overlay::types::dnat_rule::DnatRule;
use crate::overlay::types::plan_error::PlanError;

pub(in crate::overlay) fn canonical_dnat(rules: &[DnatRule]) -> Result<Vec<DnatRule>, PlanError> {
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        if rule.id.is_empty() {
            return Err(PlanError::Invalid("dnat rule has an empty id".into()));
        }
        let floating = canonical_ip("floating", &rule.floating)?.to_string();
        let fixed = canonical_ip("fixed", &rule.fixed)?.to_string();
        if out.iter().any(|d: &DnatRule| d.id == rule.id) {
            return Err(PlanError::Invalid(format!("duplicate dnat {}", rule.id)));
        }
        if out.iter().any(|d: &DnatRule| d.floating == floating) {
            return Err(PlanError::Invalid(format!(
                "duplicate floating address {floating}"
            )));
        }
        out.push(DnatRule {
            id: rule.id.clone(),
            floating,
            fixed,
        });
    }
    Ok(out)
}
