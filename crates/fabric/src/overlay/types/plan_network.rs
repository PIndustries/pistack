// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::plan_float::PlanFloat;
use crate::overlay::types::plan_port::PlanPort;
use crate::overlay::types::plan_provider::PlanProvider;
use crate::overlay::types::plan_router::PlanRouter;

/// One overlay network to realise.
#[derive(Clone, Debug)]
pub struct PlanNetwork {
    pub id: String,
    pub vni: u32,
    pub mtu: u16,
    pub ports: Vec<PlanPort>,
    pub routers: Vec<PlanRouter>,
    /// When set, this network attaches a physnet and does not get a VXLAN device.
    pub provider: Option<PlanProvider>,
    pub floating: Vec<PlanFloat>,
}
