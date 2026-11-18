// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{kinds, Endpoint, Ingress, Protocol, Service, ServiceKind};
use async_trait::async_trait;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use relay::{
    Client, ForwardProvider, ListenerProtocol, PoolProtocol, Provider, Providers, RelayL7Provider,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};

use crate::realizer::delete_labeled::delete_labeled;
use crate::realizer::delete_unnamed::delete_unnamed;
use crate::realizer::endpoint_addrs::endpoint_addrs;
use crate::realizer::ensure_forward::ensure_forward;
use crate::realizer::held::Held;
use crate::realizer::ingress_cert::ingress_cert;
use crate::realizer::ingress_label::INGRESS_LABEL;
use crate::realizer::ingress_plan::ingress_plan;
use crate::realizer::node_port_address::node_port_address;
use crate::realizer::service_label::SERVICE_LABEL;
use crate::realizer::service_realizer::ServiceRealizer;

/// Programs relay listeners for services and ingresses on one pair of
/// providers that live as long as this realizer.
pub struct RelayRealizer {
    pub(crate) store: OnceLock<Store>,
    pub(crate) providers: Providers,
    pub(crate) node_address: IpAddr,
    /// Listeners this process opened or is opening.
    pub(crate) held: Held,
}

impl RelayRealizer {
    pub fn new() -> Self {
        Self {
            store: OnceLock::new(),
            providers: Providers::new(vec![
                Arc::new(ForwardProvider::new()),
                Arc::new(RelayL7Provider::new()),
            ]),
            node_address: node_port_address(),
            held: Held::default(),
        }
    }

    /// Bind node ports on `ip` instead of the address from the environment.
    pub fn with_node_address(mut self, ip: IpAddr) -> Self {
        self.node_address = ip;
        self
    }

    /// Address node ports bind on.
    pub fn node_address(&self) -> IpAddr {
        self.node_address
    }

    /// Attach the store opened during module start. Later calls are ignored.
    pub fn bind_store(&self, store: Store) {
        let _ = self.store.set(store);
    }

    pub(crate) fn client(&self) -> ApiResult<Client> {
        let store =
            self.store.get().cloned().ok_or_else(|| {
                ApiError::unavailable("relay realizer is not attached to the store")
            })?;
        Ok(Client::new(store, self.providers.clone()))
    }

    pub async fn ensure_service(
        &self,
        service: &Service,
        endpoints: &[Endpoint],
    ) -> ApiResult<Vec<String>> {
        if service.spec.ports.is_empty() {
            self.remove_service(service).await?;
            return Ok(Vec::new());
        }
        if service
            .spec
            .ports
            .iter()
            .any(|p| p.protocol == Protocol::Sctp || p.port == 0)
        {
            self.remove_service(service).await?;
            return Err(ApiError::unavailable("service forward is tcp and udp only"));
        }
        let client = self.client()?;
        let mut keep = Vec::new();
        let mut addrs = Vec::new();
        for port in &service.spec.ports {
            let udp = port.protocol == Protocol::Udp;
            let (listener_protocol, pool_protocol) = if udp {
                (ListenerProtocol::Udp, PoolProtocol::Udp)
            } else {
                (ListenerProtocol::Tcp, PoolProtocol::Tcp)
            };
            let base = if udp {
                format!("svc-{}-{}-udp", service.meta.id, port.port)
            } else {
                format!("svc-{}-{}", service.meta.id, port.port)
            };
            let name = base.clone();
            keep.push(name.clone());
            let members = endpoint_addrs(endpoints, port)?;
            let bound = ensure_forward(
                &self.held,
                &client,
                service.meta.project.clone(),
                &name,
                SERVICE_LABEL,
                &service.meta.id.to_string(),
                Provider::Forward,
                listener_protocol.clone(),
                pool_protocol,
                port.port,
                None,
                &members,
                &[],
            )
            .await?;
            if service.spec.kind == ServiceKind::LoadBalancer {
                addrs.push(bound);
            }
            if service.spec.kind == ServiceKind::ClusterIp {
                continue;
            }
            let Some(&node_port) = service.status.node_ports.get(&port.port) else {
                continue;
            };
            let name = format!("{base}-node-{node_port}");
            keep.push(name.clone());
            ensure_forward(
                &self.held,
                &client,
                service.meta.project.clone(),
                &name,
                SERVICE_LABEL,
                &service.meta.id.to_string(),
                Provider::Forward,
                listener_protocol,
                pool_protocol,
                node_port,
                Some(SocketAddr::new(self.node_address, node_port)),
                &members,
                &[],
            )
            .await?;
        }
        delete_unnamed(&client, SERVICE_LABEL, &service.meta.id.to_string(), &keep).await?;
        Ok(addrs)
    }

    pub async fn remove_service(&self, service: &Service) -> ApiResult<()> {
        let client = self.client()?;
        delete_labeled(&client, SERVICE_LABEL, &service.meta.id.to_string()).await
    }

    /// One HTTP listener per ingress. The default backend is the
    /// listener's default pool, and each rule path is its own route and
    /// pool, so a request reaches only the service its host and path name.
    /// A missing service, an unknown port, or an empty endpoint set removes
    /// the listener and stays unavailable.
    pub async fn ensure_ingress(&self, ingress: &Ingress) -> ApiResult<Vec<String>> {
        let client = self.client()?;
        let (members, routes) = match ingress_plan(&client, ingress) {
            Ok(plan) => plan,
            Err(e) => {
                let _ = self.remove_ingress(ingress).await;
                return Err(e);
            }
        };
        let name = format!("ing-{}", ingress.meta.id);
        let id = ingress.meta.id.to_string();
        let bound = ensure_forward(
            &self.held,
            &client,
            ingress.meta.project.clone(),
            &name,
            INGRESS_LABEL,
            &id,
            Provider::Relay,
            ListenerProtocol::Http,
            PoolProtocol::Http,
            80,
            None,
            &members,
            &routes,
        )
        .await?;
        // HTTPS rides a second balancer: relay keeps one socket per balancer.
        let only_http = std::slice::from_ref(&name);
        let cert = match ingress_cert(&client, ingress) {
            Ok(Some(cert)) => cert,
            Ok(None) => {
                delete_unnamed(&client, INGRESS_LABEL, &id, only_http).await?;
                return Ok(vec![bound]);
            }
            Err(e) => {
                let _ = delete_unnamed(&client, INGRESS_LABEL, &id, only_http).await;
                return Err(e);
            }
        };
        let tls = ensure_forward(
            &self.held,
            &client,
            ingress.meta.project.clone(),
            &format!("{name}-tls"),
            INGRESS_LABEL,
            &id,
            Provider::Relay,
            ListenerProtocol::Terminated { cert },
            PoolProtocol::Http,
            443,
            None,
            &members,
            &routes,
        )
        .await?;
        Ok(vec![bound, tls])
    }

    /// Close every listener this process opened for owners of `label`.
    /// That includes listeners whose pass failed after the bind, and a
    /// pass still binding closes what it opened when it finishes.
    pub(crate) async fn release_label(&self, label: &'static str) {
        let ids = self.held.release(label);
        if ids.is_empty() {
            return;
        }
        for id in &ids {
            self.providers.stop_everywhere(id).await;
        }
        tracing::info!(
            count = ids.len(),
            owner = label,
            "loom: closed listeners after losing the lead"
        );
    }

    /// Close every service listener this process holds open.
    pub async fn release_services(&self) {
        self.release_label(SERVICE_LABEL).await;
    }

    /// Close every ingress listener this process holds open.
    pub async fn release_ingresses(&self) {
        self.release_label(INGRESS_LABEL).await;
    }

    pub async fn remove_ingress(&self, ingress: &Ingress) -> ApiResult<()> {
        let client = self.client()?;
        delete_labeled(&client, INGRESS_LABEL, &ingress.meta.id.to_string()).await
    }

    /// Drop listeners whose ingress record is gone.
    pub async fn gc_ingresses(&self) -> ApiResult<()> {
        let client = self.client()?;
        let ingresses = client.store().collection::<Ingress>();
        let lbs = client.balancers().list(None).map_err(ApiError::from)?;
        for lb in lbs {
            let Some(raw) = lb.meta.labels.get(INGRESS_LABEL) else {
                continue;
            };
            let Some(id) = Id::parse(kinds::INGRESS, raw) else {
                continue;
            };
            match ingresses.try_get(&id) {
                Ok(None) => {
                    client.delete_balancer(&lb.meta.id).await?;
                }
                Ok(Some(_)) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "loom: ingress lookup failed during gc");
                }
            }
        }
        Ok(())
    }
}

impl Default for RelayRealizer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ServiceRealizer for RelayRealizer {
    async fn realize(&self, service: &Service, endpoints: &[Endpoint]) -> ApiResult<Vec<String>> {
        self.ensure_service(service, endpoints).await
    }
    async fn remove(&self, service: &Service) -> ApiResult<()> {
        self.remove_service(service).await
    }
    async fn release(&self) {
        self.release_services().await;
    }
}
