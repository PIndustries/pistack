// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::port_spec::PortSpec;
use super::port_status::PortStatus;

resource!(Port, PortSpec, PortStatus, kind::PORT);

impl Port {
    /// True for ports fabric itself created for a router.
    pub fn is_router_port(&self) -> bool {
        self.spec
            .device
            .as_ref()
            .is_some_and(|d| d.kind == kind::ROUTER)
    }
}
