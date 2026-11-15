// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::is_hex_suffix::is_hex_suffix;
use crate::overlay::types::plan_error::PlanError;
use std::collections::BTreeSet;

/// Host-NAT bridges are `pistack0` and `pi` plus hex. Overlay names use another prefix.
pub(in crate::overlay) fn claim(
    names: &mut BTreeSet<String>,
    name: String,
    prefix: &str,
) -> Result<String, PlanError> {
    if name == "pistack0"
        || name.starts_with("pi")
        || name.len() > 15
        || !is_hex_suffix(&name, prefix)
    {
        return Err(PlanError::Name(name));
    }
    if !names.insert(name.clone()) {
        return Err(PlanError::Invalid(format!("duplicate device name {name}")));
    }
    Ok(name)
}
