// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovn_backend::OvnBackend;

impl OvnBackend {
    pub(in crate::backend::ovn::ovs_net_backend) fn name_body(&self) -> &'static str {
        "ovn"
    }
}
