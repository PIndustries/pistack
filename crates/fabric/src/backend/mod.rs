// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam between the network model and whatever realises it.
//!
//! `fabric` owns the resources and decides *what* should exist; a
//! `NetBackend` makes it so. Every call is idempotent ("ensure"), takes the
//! fully resolved picture (a router with its gateway and interface ports
//! already looked up) and returns only what the model cannot know by
//! itself (a port binding). The reconciler is the only caller.
//!
//! Two implementations ship: [`MockBackend`] for tests and single-node
//! development, and [`OvnBackend`], today a stub that documents the OVN
//! northbound mapping for every operation.

use crate::types::{FloatingIp, Network, Port, PortBinding, Router, SecurityGroup, Subnet};
use async_trait::async_trait;
use pistack_types::{ApiResult, Id};

pub mod mock;
pub mod ovn;

pub use mock::{Call, MockBackend};
pub use ovn::OvnBackend;

/// A router with everything the backend needs to plumb it.
#[derive(Clone, Debug)]
pub struct RouterView {
    pub router: Router,
    /// External network and the router's gateway port on it.
    pub gateway: Option<(Network, Port)>,
    /// One entry per `spec.interfaces` subnet: the subnet and the router's
    /// interface port on it (holding the subnet gateway address).
    pub interfaces: Vec<(Subnet, Port)>,
}

/// A floating IP with the far end resolved.
#[derive(Clone, Debug)]
pub struct FipView {
    pub fip: FloatingIp,
    /// Port and the fixed address being NATed; `None` means disassociate.
    pub target: Option<(Port, std::net::IpAddr)>,
    /// Router carrying the NAT, once one is found.
    pub router: Option<Router>,
}

#[async_trait]
pub trait NetBackend: Send + Sync + 'static {
    /// Human name, for logs and `GET /network/`.
    fn name(&self) -> &'static str;

    async fn ensure_network(&self, net: &Network) -> ApiResult<()>;
    async fn delete_network(&self, id: &Id) -> ApiResult<()>;

    async fn ensure_subnet(&self, net: &Network, subnet: &Subnet) -> ApiResult<()>;
    async fn delete_subnet(&self, id: &Id) -> ApiResult<()>;

    /// `groups` are the resolved security groups the port is a member of.
    async fn ensure_port(
        &self,
        net: &Network,
        port: &Port,
        groups: &[SecurityGroup],
    ) -> ApiResult<PortBinding>;
    async fn delete_port(&self, id: &Id) -> ApiResult<()>;

    async fn ensure_router(&self, view: &RouterView) -> ApiResult<()>;
    async fn delete_router(&self, id: &Id) -> ApiResult<()>;

    /// `members` are the ports currently in the group.
    async fn apply_security_group(&self, sg: &SecurityGroup, members: &[Port]) -> ApiResult<()>;
    async fn delete_security_group(&self, id: &Id) -> ApiResult<()>;

    /// Associate (or, with `target: None`, disassociate) a floating IP.
    async fn associate_fip(&self, view: &FipView) -> ApiResult<()>;
    async fn delete_fip(&self, id: &Id) -> ApiResult<()>;
}
