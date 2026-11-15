// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::ca::ROLE_AGENT;
use crate::types::Certificate;

use super::managed_cert_role;

/// When the agent certificate of `node` expires: the newest one issued for
/// that name that is not revoked. `None` for a host with no certificate.
pub fn agent_cert_expiry(
    certs: &[Certificate],
    node: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    certs
        .iter()
        .filter(|c| {
            managed_cert_role(c) == Some(ROLE_AGENT)
                && c.spec.common_name == node
                && !c.status.revoked
        })
        .max_by_key(|c| c.status.serial)
        .and_then(|c| c.status.not_after)
}
