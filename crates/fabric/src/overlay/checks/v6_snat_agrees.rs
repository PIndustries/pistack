// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::router_namespace::RouterNamespace;

/// IPv6 SNAT is the external address plus every tenant prefix of that family.
/// A public IPv6 external address never reaches this check.
pub(in crate::overlay) fn v6_snat_agrees(ns: &RouterNamespace) -> Result<(), PlanError> {
    let Some(ext) = &ns.external_ip else {
        return Ok(());
    };
    if !ext.contains(':') {
        return Ok(());
    }
    if !ns.snat {
        return Err(PlanError::Invalid(format!(
            "router {} has an ipv6 external address and snat is off",
            ns.id
        )));
    }
    for iface in &ns.interfaces {
        if !iface.cidr.contains(':') {
            return Err(PlanError::Invalid(format!(
                "router {} snat family does not match external ip",
                ns.id
            )));
        }
    }
    for rule in &ns.dnat {
        if !rule.floating.contains(':') {
            return Err(PlanError::Invalid(format!(
                "router {} floating family does not match external ip",
                ns.id
            )));
        }
    }
    Ok(())
}
