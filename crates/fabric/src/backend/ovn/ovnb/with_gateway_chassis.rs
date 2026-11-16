// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovn_backend::OvnBackend;

impl OvnBackend {
    /// Chassis that may host router gateway ports, highest priority first.
    pub fn with_gateway_chassis(mut self, chassis: Vec<String>) -> Self {
        self.gateway_chassis = chassis;
        self
    }
}
