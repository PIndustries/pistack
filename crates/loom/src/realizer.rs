// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam between the Service controller and whatever makes a cluster IP
//! actually route: L4/L7 balancers via `relay`, or nothing at all in dev mode.

use crate::types::{
    kinds, Endpoint, Ingress, IngressBackend, IntOrName, Protocol, Service, ServiceKind,
    ServicePort,
};
use async_trait::async_trait;
use pistack_store::{Store, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta, Phase};
use relay::{
    Algorithm, BalancerSpec, Client, ForwardProvider, ListenerProtocol, ListenerSpec, MemberSpec,
    PoolProtocol, PoolSpec, Provider, Providers, RelayL7Provider, MANAGED_LABEL,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::net::TcpStream;

const SERVICE_LABEL: &str = "loom.pistack.io/service";
const INGRESS_LABEL: &str = "loom.pistack.io/ingress";
const VIP_NETWORK: &str = "cluster";

#[async_trait]
pub trait ServiceRealizer: Send + Sync + 'static {
    /// Make `service.status.cluster_ip` (and node ports / load balancer, by
    /// kind) forward to `endpoints`. Called whenever the set changes.
    /// Idempotent. May return external addresses for `LoadBalancer`
    /// services; they land in `status.load_balancer`.
    async fn realize(&self, service: &Service, endpoints: &[Endpoint]) -> ApiResult<Vec<String>>;
    /// Tear down whatever `realize` built. Unknown services are not an error.
    async fn remove(&self, service: &Service) -> ApiResult<()>;
}

/// Records nothing, routes nothing. Dev mode and tests.
#[derive(Default, Debug, Clone, Copy)]
pub struct NoopRealizer;

#[async_trait]
impl ServiceRealizer for NoopRealizer {
    async fn realize(&self, _service: &Service, _endpoints: &[Endpoint]) -> ApiResult<Vec<String>> {
        Ok(Vec::new())
    }
    async fn remove(&self, _service: &Service) -> ApiResult<()> {
        Ok(())
    }
}

/// Programs relay listeners for services and ingresses on one pair of
/// providers that live as long as this realizer.
pub struct RelayRealizer {
    store: OnceLock<Store>,
    providers: Providers,
}

impl RelayRealizer {
    pub fn new() -> Self {
        Self {
            store: OnceLock::new(),
            providers: Providers::new(vec![
                Arc::new(ForwardProvider::new()),
                Arc::new(RelayL7Provider::new()),
            ]),
        }
    }

    /// Attach the store opened during module start. Later calls are ignored.
    pub fn bind_store(&self, store: Store) {
        let _ = self.store.set(store);
    }

    fn client(&self) -> ApiResult<Client> {
        let store = self.store.get().cloned().ok_or_else(|| {
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
            .any(|p| p.protocol != Protocol::Tcp || p.port == 0)
        {
            self.remove_service(service).await?;
            return Err(ApiError::unavailable("service forward is tcp only"));
        }
        let client = self.client()?;
        let mut keep = Vec::new();
        let mut addrs = Vec::new();
        for port in &service.spec.ports {
            let name = format!("svc-{}-{}", service.meta.id, port.port);
            keep.push(name.clone());
            let members = endpoint_addrs(endpoints, port)?;
            let bound = ensure_forward(
                &client,
                service.meta.project.clone(),
                &name,
                SERVICE_LABEL,
                &service.meta.id.to_string(),
                Provider::Ovn,
                ListenerProtocol::Tcp,
                PoolProtocol::Tcp,
                port.port,
                &members,
            )
            .await?;
            if service.spec.kind == ServiceKind::LoadBalancer {
                addrs.push(bound);
            }
        }
        delete_unnamed(&client, SERVICE_LABEL, &service.meta.id.to_string(), &keep).await?;
        Ok(addrs)
    }

    pub async fn remove_service(&self, service: &Service) -> ApiResult<()> {
        let client = self.client()?;
        delete_labeled(&client, SERVICE_LABEL, &service.meta.id.to_string()).await
    }

    /// One HTTP listener for every backend that has endpoints. A missing
    /// service, an unknown port, or an empty endpoint set removes the
    /// listener and stays unavailable.
    pub async fn ensure_ingress(&self, ingress: &Ingress) -> ApiResult<Vec<String>> {
        let client = self.client()?;
        let members = match ingress_members(&client, ingress) {
            Ok(members) => members,
            Err(e) => {
                let _ = self.remove_ingress(ingress).await;
                return Err(e);
            }
        };
        let name = format!("ing-{}", ingress.meta.id);
        let bound = ensure_forward(
            &client,
            ingress.meta.project.clone(),
            &name,
            INGRESS_LABEL,
            &ingress.meta.id.to_string(),
            Provider::Relay,
            ListenerProtocol::Http,
            PoolProtocol::Http,
            80,
            &members,
        )
        .await?;
        Ok(vec![bound])
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
}

fn endpoint_addrs(endpoints: &[Endpoint], port: &ServicePort) -> ApiResult<Vec<SocketAddr>> {
    let dest = port.target_port.unwrap_or(port.port);
    if dest == 0 {
        return Err(ApiError::unavailable("service forward is tcp only"));
    }
    let mut out = Vec::new();
    for ep in endpoints {
        let ip: IpAddr = match ep.ip.parse() {
            Ok(ip) => ip,
            Err(_) => {
                return Err(ApiError::unavailable("service endpoint is not available"));
            }
        };
        let addr = SocketAddr::new(ip, dest);
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    Ok(out)
}

fn ingress_members(client: &Client, ingress: &Ingress) -> ApiResult<Vec<SocketAddr>> {
    let backends = ingress_backends(ingress);
    if backends.is_empty() {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    let mut out = Vec::new();
    for backend in backends {
        let addrs = one_backend(client, ingress, backend)?;
        for addr in addrs {
            if !out.contains(&addr) {
                out.push(addr);
            }
        }
    }
    if out.is_empty() {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    Ok(out)
}

fn ingress_backends(ingress: &Ingress) -> Vec<&IngressBackend> {
    let mut out = Vec::new();
    if let Some(backend) = &ingress.spec.default_backend {
        out.push(backend);
    }
    for rule in &ingress.spec.rules {
        for path in &rule.paths {
            out.push(&path.backend);
        }
    }
    out
}

fn one_backend(
    client: &Client,
    ingress: &Ingress,
    backend: &IngressBackend,
) -> ApiResult<Vec<SocketAddr>> {
    let services = client.store().collection::<Service>();
    let svc = if let Some(id) = Id::parse(kinds::SERVICE, &backend.service) {
        services.get(&id).map_err(|_| {
            ApiError::unavailable("ingress backend is not available")
        })?
    } else {
        services
            .get_by_name(ingress.meta.project.as_ref(), &backend.service)
            .map_err(|_| ApiError::unavailable("ingress backend is not available"))?
    };
    if !projects_match(&ingress.meta.project, &svc.meta.project) {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    let Some(port) = svc.spec.ports.iter().find(|p| port_matches(p, &backend.port)) else {
        return Err(ApiError::unavailable("ingress backend is not available"));
    };
    if port.protocol != Protocol::Tcp {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    let addrs = endpoint_addrs(&svc.status.endpoints, port)?;
    if addrs.is_empty() {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    Ok(addrs)
}

fn port_matches(port: &ServicePort, want: &IntOrName) -> bool {
    match want {
        IntOrName::Int(n) => port.port == *n,
        IntOrName::Name(name) => !name.is_empty() && port.name == *name,
    }
}

fn projects_match(a: &Option<Id>, b: &Option<Id>) -> bool {
    match (a, b) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

async fn ensure_forward(
    client: &Client,
    project: Option<Id>,
    name: &str,
    owner_label: &str,
    owner_id: &str,
    provider: Provider,
    listener_protocol: ListenerProtocol,
    pool_protocol: PoolProtocol,
    port: u16,
    members: &[SocketAddr],
) -> ApiResult<String> {
    let lb = match client.balancers().get_by_name(project.as_ref(), name) {
        Ok(lb) => {
            if lb.spec.vip_network != VIP_NETWORK || lb.spec.provider != provider {
                return Err(ApiError::unavailable(
                    "listener is not available",
                ));
            }
            lb
        }
        Err(StoreError::NotFound(_)) => {
            let mut meta = Meta::new(kind::BALANCER, name, project.clone());
            meta.labels.insert(MANAGED_LABEL.into(), "loom".into());
            meta.labels.insert(owner_label.into(), owner_id.to_owned());
            client.create_balancer(
                meta,
                BalancerSpec {
                    vip_network: VIP_NETWORK.into(),
                    provider,
                    admin_up: true,
                    ..BalancerSpec::default()
                },
            )?
        }
        Err(e) => return Err(e.into()),
    };

    let pool = match client.pools_of(&lb.meta.id)?.into_iter().next() {
        Some(pool) => pool,
        None => client.create_pool(
            Meta::new(relay::types::kinds::POOL, format!("{name}-pool"), project.clone()),
            PoolSpec {
                balancer: lb.meta.id.clone(),
                protocol: pool_protocol,
                algorithm: Algorithm::RoundRobin,
                session_persistence: None,
                health_monitor: None,
                admin_up: true,
            },
        )?,
    };

    let listeners = client.listeners_of(&lb.meta.id)?;
    let listener = match listeners
        .into_iter()
        .find(|l| l.spec.default_pool.as_ref() == Some(&pool.meta.id))
    {
        Some(l) if l.spec.port == port && l.spec.protocol == listener_protocol => l,
        Some(l) => {
            let protocol = listener_protocol.clone();
            client.update_listener(&l.meta.id, None, move |row| {
                row.spec.port = port;
                row.spec.protocol = protocol;
                Ok(())
            })?
        }
        None => client.create_listener(
            Meta::new(
                relay::types::kinds::LISTENER,
                format!("{name}-lsn"),
                project.clone(),
            ),
            ListenerSpec {
                balancer: lb.meta.id.clone(),
                protocol: listener_protocol,
                port,
                default_pool: Some(pool.meta.id.clone()),
                connection_limit: 0,
                admin_up: true,
                door: None,
            },
        )?,
    };

    let have = client.members_of(&pool.meta.id)?;
    let mut changed = false;
    for member in &have {
        if !members.contains(&member.addr()) {
            client.delete_member(&member.meta.id)?;
            changed = true;
        }
    }
    for want in members {
        if !have.iter().any(|member| member.addr() == *want) {
            client.create_member(
                Meta::new(
                    relay::types::kinds::MEMBER,
                    format!("{name}-{}", member_slug(want)),
                    project.clone(),
                ),
                MemberSpec {
                    pool: pool.meta.id.clone(),
                    address: want.ip(),
                    port: want.port(),
                    weight: 1,
                    backup: false,
                    admin_up: true,
                },
            )?;
            changed = true;
        }
    }

    let provider_handle = client.provider_for(&lb)?;
    let live = provider_handle.ensure_balancer(&lb).await?;
    let lb = client.balancers().get(&lb.meta.id).map_err(ApiError::from)?;
    let listener = client
        .listeners()
        .get(&listener.meta.id)
        .map_err(ApiError::from)?;
    let aligned = lb.status.phase == Phase::Ready
        && lb.status.provider_ref.as_deref() == Some(live.provider_ref.as_str())
        && listener.status.phase == Phase::Ready
        && listener.status.provider_ref.as_deref() == Some(live.provider_ref.as_str());
    if !aligned {
        client.update_balancer(&lb.meta.id, None, |_| Ok(()))?;
        client.reconcile_balancer(&lb.meta.id).await;
    }
    if changed || !aligned {
        client.touch_pool(&pool.meta.id);
        client.reconcile_pool(&pool.meta.id).await;
    }

    let lb = client.balancers().get(&lb.meta.id).map_err(ApiError::from)?;
    let listener = client
        .listeners()
        .get(&listener.meta.id)
        .map_err(ApiError::from)?;
    if listener.status.phase != Phase::Ready {
        let msg = if !listener.status.message.is_empty() {
            listener.status.message
        } else if lb.status.phase != Phase::Ready && !lb.status.message.is_empty() {
            lb.status.message
        } else {
            "listener is not available".into()
        };
        return Err(ApiError::unavailable(msg));
    }
    let addr = listener
        .status
        .provider_ref
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::unavailable("listener is not available"))?;
    if !listener_answers(&addr).await {
        return Err(ApiError::unavailable("listener is not available"));
    }
    Ok(addr)
}

fn member_slug(addr: &SocketAddr) -> String {
    format!("{}-{}", addr.ip(), addr.port())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

async fn listener_answers(addr: &str) -> bool {
    let Ok(sock) = addr.parse::<SocketAddr>() else {
        return false;
    };
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_millis(200), TcpStream::connect(sock)).await {
            Ok(Ok(stream)) => {
                drop(stream);
                return true;
            }
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    false
}

async fn delete_labeled(client: &Client, label: &str, id: &str) -> ApiResult<()> {
    delete_unnamed(client, label, id, &[]).await
}

async fn delete_unnamed(client: &Client, label: &str, id: &str, keep: &[String]) -> ApiResult<()> {
    let lbs = client.balancers().list(None).map_err(ApiError::from)?;
    for lb in lbs {
        if lb.meta.labels.get(label) == Some(&id.to_owned()) && !keep.iter().any(|n| n == &lb.meta.name)
        {
            client.delete_balancer(&lb.meta.id).await?;
        }
    }
    Ok(())
}
