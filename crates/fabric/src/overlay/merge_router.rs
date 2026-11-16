// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::same_service::same_service;
use crate::overlay::types::dnat_rule::DnatRule;
use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::plan_router::PlanRouter;
use crate::overlay::types::router_namespace::RouterNamespace;
use crate::overlay::types::service_rule::ServiceRule;

pub(super) fn merge_router(
    ns: &mut RouterNamespace,
    router: &PlanRouter,
    external_ip: Option<String>,
    dnat: Vec<DnatRule>,
    services: Vec<ServiceRule>,
) -> Result<(), PlanError> {
    if let Some(ip) = external_ip {
        if let Some(have) = &ns.external_ip {
            if have != &ip {
                return Err(PlanError::Invalid(format!(
                    "router {} external ip disagrees",
                    router.id
                )));
            }
        } else {
            ns.external_ip = Some(ip);
        }
    }
    if !router.external_dev.is_empty() && ns.external_dev.is_empty() {
        ns.external_dev.clone_from(&router.external_dev);
    } else if !router.external_dev.is_empty() && ns.external_dev != router.external_dev {
        return Err(PlanError::Invalid(format!(
            "router {} external dev disagrees",
            router.id
        )));
    }
    if ns.metadata != router.metadata {
        return Err(PlanError::Invalid(format!(
            "router {} metadata flag disagrees",
            router.id
        )));
    }
    for rule in dnat {
        if let Some(prev) = ns.dnat.iter().find(|d| d.id == rule.id) {
            if prev != &rule {
                return Err(PlanError::Invalid(format!(
                    "dnat {} disagrees across router attachments",
                    rule.id
                )));
            }
            continue;
        }
        if ns.dnat.iter().any(|d| d.floating == rule.floating) {
            return Err(PlanError::Invalid(format!(
                "duplicate floating address {}",
                rule.floating
            )));
        }
        ns.dnat.push(rule);
    }
    // Each leg lists the backends on its own subnet. One namespace answers
    // for every leg, so it sends to all of them.
    for rule in services {
        if let Some(prev) = ns.services.iter_mut().find(|r| same_service(r, &rule)) {
            for b in rule.backends {
                if !prev.backends.contains(&b) {
                    prev.backends.push(b);
                }
            }
            continue;
        }
        ns.services.push(rule);
    }
    ns.snat = ns.external_ip.is_some();
    Ok(())
}
