// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Control-plane acceptance of an overlay network.
//!
//! This backend does not create devices and does not call iptables. An overlay
//! network must already carry a VXLAN VNI in `status.segment_id`. The agent
//! applies [`crate::overlay::OverlayPlan`]. Deleting a record succeeds here.
//! The next plan omits it, and the helper removes only the bridge, VXLAN,
//! namespace, and veth that plan created. A second delete is a success when
//! those devices are already gone.

use super::{FipView, NetBackend, RouterView};
use crate::types::{Network, NetworkKind, Port, PortBinding, SecurityGroup, Subnet};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};

/// Accepts overlay networks whose segment id is a VXLAN VNI.
#[derive(Clone, Debug, Default)]
pub struct OverlayBackend;

impl OverlayBackend {
    pub fn new() -> Self {
        Self
    }
}

fn require_segment(net: &Network) -> ApiResult<()> {
    if !matches!(net.spec.kind, NetworkKind::Overlay) {
        return Ok(());
    }
    match net.status.segment_id {
        Some(vni) if (1..=16_777_215).contains(&vni) => Ok(()),
        _ => Err(ApiError::unavailable("overlay network has no segment")),
    }
}

#[async_trait]
impl NetBackend for OverlayBackend {
    fn name(&self) -> &'static str {
        "overlay"
    }

    async fn ensure_network(&self, net: &Network) -> ApiResult<()> {
        require_segment(net)
    }

    async fn delete_network(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }

    async fn ensure_subnet(&self, net: &Network, _subnet: &Subnet) -> ApiResult<()> {
        require_segment(net)
    }

    async fn delete_subnet(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }

    async fn ensure_port(
        &self,
        net: &Network,
        _port: &Port,
        _groups: &[SecurityGroup],
    ) -> ApiResult<PortBinding> {
        require_segment(net)?;
        Ok(PortBinding {
            vif_type: "overlay".into(),
            vif_details: Default::default(),
        })
    }

    async fn delete_port(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }

    async fn ensure_router(&self, view: &RouterView) -> ApiResult<()> {
        if let Some((net, _)) = &view.gateway {
            require_segment(net)?;
        }
        Ok(())
    }

    async fn delete_router(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }

    async fn apply_security_group(&self, _sg: &SecurityGroup, _members: &[Port]) -> ApiResult<()> {
        Ok(())
    }

    async fn delete_security_group(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }

    async fn associate_fip(&self, _view: &FipView) -> ApiResult<()> {
        Ok(())
    }

    async fn delete_fip(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        NetworkSpec, NetworkStatus, PortSpec, PortStatus, Router, RouterSpec, RouterStatus,
        SubnetSpec, SubnetStatus,
    };
    use pistack_types::{kind, Meta};

    fn net(kind: NetworkKind, segment: Option<u32>) -> Network {
        Network {
            meta: Meta::new(kind::NETWORK, "n", None),
            spec: NetworkSpec {
                kind,
                ..NetworkSpec::default()
            },
            status: NetworkStatus {
                segment_id: segment,
                ..NetworkStatus::default()
            },
        }
    }

    fn port() -> Port {
        Port {
            meta: Meta::new(kind::PORT, "p", None),
            spec: PortSpec::default(),
            status: PortStatus::default(),
        }
    }

    #[tokio::test]
    async fn overlay_requires_a_vni_before_ensure() {
        let backend = OverlayBackend::new();
        assert_eq!(backend.name(), "overlay");

        let missing = net(NetworkKind::Overlay, None);
        let err = backend.ensure_network(&missing).await.unwrap_err();
        assert_eq!(err.message, "overlay network has no segment");

        let zero = net(NetworkKind::Overlay, Some(0));
        assert_eq!(
            backend.ensure_network(&zero).await.unwrap_err().message,
            "overlay network has no segment"
        );
        let huge = net(NetworkKind::Overlay, Some(16_777_216));
        assert_eq!(
            backend.ensure_network(&huge).await.unwrap_err().message,
            "overlay network has no segment"
        );

        let ready = net(NetworkKind::Overlay, Some(1));
        backend.ensure_network(&ready).await.unwrap();
        let max = net(NetworkKind::Overlay, Some(16_777_215));
        backend.ensure_network(&max).await.unwrap();

        let provider = net(
            NetworkKind::Provider {
                vlan: None,
                physnet: "phys".into(),
            },
            None,
        );
        backend.ensure_network(&provider).await.unwrap();

        let sub = Subnet {
            meta: Meta::new(kind::SUBNET, "s", None),
            spec: SubnetSpec {
                network: missing.meta.id.clone(),
                cidr: "10.90.0.0/24".parse().unwrap(),
                gateway: Some("10.90.0.1".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
            status: SubnetStatus::default(),
        };
        assert_eq!(
            backend
                .ensure_subnet(&missing, &sub)
                .await
                .unwrap_err()
                .message,
            "overlay network has no segment"
        );
        backend.ensure_subnet(&ready, &sub).await.unwrap();
        backend.ensure_subnet(&provider, &sub).await.unwrap();

        assert_eq!(
            backend
                .ensure_port(&missing, &port(), &[])
                .await
                .unwrap_err()
                .message,
            "overlay network has no segment"
        );
        let binding = backend.ensure_port(&ready, &port(), &[]).await.unwrap();
        assert_eq!(binding.vif_type, "overlay");
        assert!(binding.vif_details.is_empty());

        let router = Router {
            meta: Meta::new(kind::ROUTER, "r", None),
            spec: RouterSpec::default(),
            status: RouterStatus::default(),
        };
        let blocked = RouterView {
            router: router.clone(),
            gateway: Some((missing, port())),
            gateway_subnet: None,
            interfaces: vec![],
        };
        assert_eq!(
            backend.ensure_router(&blocked).await.unwrap_err().message,
            "overlay network has no segment"
        );
        let open = RouterView {
            router,
            gateway: None,
            gateway_subnet: None,
            interfaces: vec![],
        };
        backend.ensure_router(&open).await.unwrap();
        backend.delete_network(&ready.meta.id).await.unwrap();
    }
}
