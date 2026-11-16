// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::canonical_external_ip::canonical_external_ip;
use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::plan_network::PlanNetwork;
use std::collections::BTreeSet;

pub(super) fn external_ips(networks: &[PlanNetwork]) -> Result<BTreeSet<String>, PlanError> {
    let mut out = BTreeSet::new();
    for net in networks {
        for router in &net.routers {
            if let Some(ip) = canonical_external_ip(&router.external_ip)? {
                out.insert(ip);
            }
        }
    }
    Ok(out)
}
