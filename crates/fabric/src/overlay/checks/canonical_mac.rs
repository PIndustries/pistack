// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::plan_error::PlanError;

pub(in crate::overlay) fn canonical_mac(raw: &str) -> Result<String, PlanError> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 6 {
        return Err(PlanError::Mac(raw.to_string()));
    }
    let mut out = String::with_capacity(17);
    for (i, part) in parts.iter().enumerate() {
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(PlanError::Mac(raw.to_string()));
        }
        if i > 0 {
            out.push(':');
        }
        out.push_str(&part.to_ascii_lowercase());
    }
    Ok(out)
}
