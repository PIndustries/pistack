// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::claim::claim;
use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::plan_provider::PlanProvider;
use crate::overlay::types::provider_attach::ProviderAttach;
use std::collections::BTreeSet;

pub(super) fn canonical_provider(
    spec: &PlanProvider,
    hex: &str,
    names: &mut BTreeSet<String>,
) -> Result<ProviderAttach, PlanError> {
    let physnet = spec.physnet.as_str();
    if physnet.is_empty()
        || physnet.chars().any(char::is_whitespace)
        || physnet.contains('/')
        || physnet == "pistack0"
        || physnet == "wg-fleet"
        || physnet == "simbr0"
        || physnet.starts_with("pi")
    {
        return Err(PlanError::Invalid(format!("refusing physnet {physnet}")));
    }
    if let Some(vlan) = spec.vlan {
        if !(1..=4094).contains(&vlan) {
            return Err(PlanError::Invalid(format!(
                "vlan {vlan} is outside 1..=4094"
            )));
        }
    }
    Ok(ProviderAttach {
        physnet: physnet.to_string(),
        vlan: spec.vlan,
        link: claim(names, format!("pv{hex}"), "pv")?,
    })
}
