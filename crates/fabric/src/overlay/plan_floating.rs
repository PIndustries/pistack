// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::canonical_ip::canonical_ip;
use crate::overlay::checks::claim::claim;
use crate::overlay::device_hex::device_hex;
use crate::overlay::types::floating_local::FloatingLocal;
use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::plan_network::PlanNetwork;
use crate::overlay::types::port_endpoint::PortEndpoint;
use std::collections::BTreeSet;

pub(super) fn plan_floating(
    net: &PlanNetwork,
    ports: &[PortEndpoint],
    external_ips: &BTreeSet<String>,
    seen_ids: &mut BTreeSet<String>,
    seen_addrs: &mut BTreeSet<String>,
    names: &mut BTreeSet<String>,
) -> Result<Vec<FloatingLocal>, PlanError> {
    let port_addrs: BTreeSet<&str> = ports.iter().map(|p| p.address.as_str()).collect();
    let mut out = Vec::with_capacity(net.floating.len());
    for float in &net.floating {
        if float.id.is_empty() {
            return Err(PlanError::Invalid("floating id is empty".into()));
        }
        if !seen_ids.insert(float.id.clone()) {
            return Err(PlanError::Invalid(format!(
                "duplicate floating id {}",
                float.id
            )));
        }
        let floating = canonical_ip("floating", &float.floating)?.to_string();
        let fixed = canonical_ip("fixed", &float.fixed)?.to_string();
        if external_ips.contains(&floating) {
            return Err(PlanError::Invalid(format!(
                "floating address {floating} matches a router external ip"
            )));
        }
        if !seen_addrs.insert(floating.clone()) {
            return Err(PlanError::Invalid(format!(
                "duplicate floating address {floating}"
            )));
        }
        if !port_addrs.contains(fixed.as_str()) {
            return Err(PlanError::Invalid(format!(
                "fixed address {fixed} is not a port on {}",
                net.id
            )));
        }
        let hex = device_hex(&float.id)?;
        out.push(FloatingLocal {
            id: float.id.clone(),
            floating,
            fixed,
            netns: claim(names, format!("nf{hex}"), "nf")?,
            veth_bridge: claim(names, format!("e{hex}"), "e")?,
            veth_ns: claim(names, format!("f{hex}"), "f")?,
        });
    }
    Ok(out)
}
