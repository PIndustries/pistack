// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::backend::{FipView, RouterView};
use crate::client::Client;
use crate::types::*;
use pistack_types::{kind, ApiResult, Id, Phase};

use super::*;
use crate::backend::NetBackend;
use async_trait::async_trait;
use pistack_store::Store;
use pistack_types::Meta;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Holds the first floating-IP call so a spec write can land underneath it.
struct Gate {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    held: AtomicBool,
    calls: parking_lot::Mutex<Vec<Option<Id>>>,
}

#[async_trait]
impl NetBackend for Gate {
    fn name(&self) -> &'static str {
        "gate"
    }
    async fn ensure_network(&self, _: &Network) -> ApiResult<()> {
        Ok(())
    }
    async fn delete_network(&self, _: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn ensure_subnet(&self, _: &Network, _: &Subnet) -> ApiResult<()> {
        Ok(())
    }
    async fn delete_subnet(&self, _: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn ensure_port(
        &self,
        _: &Network,
        _: &Port,
        _: &[SecurityGroup],
    ) -> ApiResult<PortBinding> {
        Ok(PortBinding::default())
    }
    async fn delete_port(&self, _: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn ensure_router(&self, _: &RouterView) -> ApiResult<()> {
        Ok(())
    }
    async fn delete_router(&self, _: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn apply_security_group(&self, _: &SecurityGroup, _: &[Port]) -> ApiResult<()> {
        Ok(())
    }
    async fn delete_security_group(&self, _: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn associate_fip(&self, view: &FipView) -> ApiResult<()> {
        self.calls.lock().push(view.fip.spec.port.clone());
        if !self.held.swap(true, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
    async fn delete_fip(&self, _: &Id) -> ApiResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn spec_change_during_reconcile_stays_stale() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let gate = Arc::new(Gate {
        entered: entered.clone(),
        release: release.clone(),
        held: AtomicBool::new(false),
        calls: parking_lot::Mutex::new(Vec::new()),
    });
    let store = Store::in_memory().unwrap();
    let client = Client::new(store, gate.clone());
    client.ensure_default_template().unwrap();
    let project = Id::new(kind::PROJECT);
    let ext = NetworkSpec {
        external: true,
        ..Default::default()
    };
    let net = client
        .create_network(
            Meta::new(kind::NETWORK, "fleet", Some(project.clone())),
            ext,
        )
        .unwrap();
    let sub = client
        .create_subnet(
            Meta::new(kind::SUBNET, "pool", Some(project.clone())),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "10.66.66.80/28".parse().unwrap(),
                gateway: Some("10.66.66.81".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: false,
                host_routes: vec![],
            },
        )
        .unwrap();
    let guest = client
        .create_network(
            Meta::new(kind::NETWORK, "guest", Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    client
        .create_subnet(
            Meta::new(kind::SUBNET, "guest", Some(project.clone())),
            SubnetSpec {
                network: guest.meta.id.clone(),
                cidr: "10.88.0.0/24".parse().unwrap(),
                gateway: Some("10.88.0.1".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();
    let port = client
        .create_port(
            Meta::new(kind::PORT, "demo", Some(project.clone())),
            PortSpec {
                network: guest.meta.id,
                fixed_ips: vec!["10.88.0.187".parse().unwrap()],
                ..PortSpec::default()
            },
        )
        .unwrap();
    let fip = client
        .create_floating_ip(
            Meta::new(kind::FLOATING_IP, "demo", Some(project)),
            FloatingIpSpec {
                network: net.meta.id,
                address: None,
                port: None,
            },
        )
        .unwrap();
    let _ = sub;

    let waiting = entered.notified();
    tokio::pin!(waiting);
    let id = fip.meta.id.clone();
    let worker = client.clone();
    let task = tokio::spawn(async move { worker.reconcile_floating_ip(&id).await });
    waiting.await;
    client
        .associate_floating_ip(&fip.meta.id, Some(port.meta.id.clone()))
        .unwrap();
    release.notify_one();
    let _ = task.await.unwrap();

    let mid = client.floating_ips().get(&fip.meta.id).unwrap();
    assert!(
        mid.stale(),
        "the in-flight pass must not mark the new port observed"
    );
    assert_eq!(mid.spec.port, Some(port.meta.id.clone()));

    client.reconcile_floating_ip(&fip.meta.id).await;
    let done = client.floating_ips().get(&fip.meta.id).unwrap();
    assert_eq!(done.status.phase, Phase::Ready);
    assert_eq!(done.status.fixed_ip, Some("10.88.0.187".parse().unwrap()));
    assert!(!done.stale());
    let calls = gate.calls.lock().clone();
    assert_eq!(calls, vec![None, Some(port.meta.id)]);
}
