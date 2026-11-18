// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Network quotas: `networks`, `vpcs`, `subnets`, `routers`,
//! `security_groups`, `ports` and `floating_ips` per project. No project, or
//! no limit for the resource, means unlimited. A missing project row is the
//! same: callers address ids that are not gate projects. A disabled project
//! rejects a create.
//!
//! Usage is the number of rows the project has, counted at each admission
//! under gate's admission lock and copied to the project for reading.
//! What fabric makes on its own is not counted: the gateway and interface
//! ports of a router, the router a vpc owns, and the project's `default`
//! security group. There is no separate DHCP port.

mod admitted;
mod count;
mod mirror;
mod released;
#[cfg(test)]
mod tests;
pub(crate) use admitted::admitted;
pub use gate::quota::{FLOATING_IPS, NETWORKS, PORTS, ROUTERS, SECURITY_GROUPS, SUBNETS, VPCS};
pub(crate) use mirror::mirror;
pub(crate) use released::released;
