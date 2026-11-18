// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

/// `pistack-{id}`. Same character rule as anvil, without a forge dependency
/// on the host agent.
pub fn domain_name(id: &str) -> Result<String, ApiError> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(ApiError::bad_request(
            "machine id cannot be used as a domain name",
        ));
    }
    Ok(format!("pistack-{id}"))
}
