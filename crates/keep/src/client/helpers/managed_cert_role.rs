// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::ca::{ROLE_AGENT, ROLE_LABEL, ROLE_SERVER};
use crate::types::{CertUsage, Certificate};

use super::sanitize;

/// Turn a CN into something that fits the resource-name rules.
/// `agent` or `server` for a certificate the cloud issued for itself, and
/// `None` for one a caller asked for. Certificates from before the role
/// label are told by having no project and the name `issue_pki` gives.
pub fn managed_cert_role(c: &Certificate) -> Option<&'static str> {
    if c.meta.project.is_some() {
        return None;
    }
    match c.meta.labels.get(ROLE_LABEL).map(String::as_str) {
        Some(ROLE_AGENT) => return Some(ROLE_AGENT),
        Some(ROLE_SERVER) => return Some(ROLE_SERVER),
        Some(_) => return None,
        None => {}
    }
    let named = format!("{}-{}", sanitize(&c.spec.common_name), c.status.serial);
    if c.meta.name != named {
        return None;
    }
    Some(match c.spec.usage {
        CertUsage::Client => ROLE_AGENT,
        _ => ROLE_SERVER,
    })
}
