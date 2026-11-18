// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use crate::ipmitool::Ipmitool;
use crate::power_control::PowerControl;
use crate::redfish::Redfish;
use crate::secret::Secret;
use crate::tool::tool;

/// The out-of-band kind named on a host spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ipmi,
    Redfish,
}

impl Kind {
    pub fn parse(raw: &str) -> ApiResult<Kind> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ipmi" => Ok(Kind::Ipmi),
            "redfish" => Ok(Kind::Redfish),
            other => Err(ApiError::bad_request(format!(
                "power control kind must be ipmi or redfish, not {other:?}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Ipmi => "ipmi",
            Kind::Redfish => "redfish",
        }
    }

    /// Power control for one chassis, from the kind, address, user and
    /// password source.
    pub fn control(
        self,
        address: impl Into<String>,
        user: impl Into<String>,
        secret: Secret,
    ) -> Box<dyn PowerControl> {
        match self {
            Kind::Ipmi => Box::new(Ipmitool::new(address, user, secret)),
            Kind::Redfish => Box::new(Redfish::new(address, user, secret)),
        }
    }

    /// Check that this transport's tool is present, so a caller can fail
    /// before it does any other work. Same message the command itself
    /// gives when the tool is missing.
    pub fn require_tool(self) -> ApiResult<()> {
        match self {
            Kind::Ipmi => tool("PISTACK_IPMITOOL", "ipmitool").map(|_| ()),
            Kind::Redfish => tool("PISTACK_CURL", "curl").map(|_| ()),
        }
    }
}
