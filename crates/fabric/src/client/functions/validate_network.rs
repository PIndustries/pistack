// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::*;
use pistack_types::{ApiError, ApiResult};
pub(crate) fn validate_network(spec: &NetworkSpec) -> ApiResult<()> {
    if !(68..=9216).contains(&spec.mtu) {
        return Err(ApiError::bad_request(format!(
            "mtu {} out of range 68-9216",
            spec.mtu
        )));
    }
    if let NetworkKind::Provider { vlan, physnet } = &spec.kind {
        if let Some(v) = vlan {
            if !(1..=4094).contains(v) {
                return Err(ApiError::bad_request(format!(
                    "vlan {v} out of range 1-4094"
                )));
            }
        }
        if physnet.is_empty() {
            return Err(ApiError::bad_request("provider network needs a physnet"));
        }
    }
    if spec.auto_floating_ip && !spec.external {
        return Err(ApiError::bad_request(
            "auto_floating_ip needs an external network",
        ));
    }
    Ok(())
}
