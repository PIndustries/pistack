// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam between the Service controller and whatever makes a cluster IP
//! actually route: L4/L7 balancers via `relay`, or nothing at all in dev mode.

use crate::types::{
    kinds, Endpoint, Ingress, IngressBackend, IntOrName, PathType, Protocol, Service, ServiceKind,
    ServicePort,
};
use async_trait::async_trait;
use pistack_store::{Store, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta, Phase};
use relay::{
    Algorithm, BalancerSpec, Client, ForwardProvider, ListenerProtocol, ListenerSpec, MemberSpec,
    PathMatch, PoolProtocol, PoolSpec, Provider, Providers, RelayL7Provider, Route, MANAGED_LABEL,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::net::TcpStream;

const SERVICE_LABEL: &str = "loom.pistack.io/service";
const INGRESS_LABEL: &str = "loom.pistack.io/ingress";
const VIP_NETWORK: &str = "cluster";
/// Private or loopback address service node ports bind on.
pub const NODE_PORT_ADDRESS_ENV: &str = "PISTACK_NODE_PORT_ADDRESS";

/// `PISTACK_NODE_PORT_ADDRESS` when it is a loopback or private address,
/// otherwise loopback.
fn node_port_address() -> IpAddr {
    let loopback = IpAddr::from([127, 0, 0, 1]);
    let Ok(raw) = std::env::var(NODE_PORT_ADDRESS_ENV) else {
        return loopback;
    };
    match raw.trim().parse::<IpAddr>() {
        Ok(ip) if relay::bind::allowed(ip) => ip,
        _ => {
            tracing::warn!(
                value = %raw,
                "loom: {NODE_PORT_ADDRESS_ENV} must be a loopback or private address; node ports bind on 127.0.0.1"
            );
            loopback
        }
    }
}

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
    node_address: IpAddr,
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

    fn client(&self) -> ApiResult<Client> {
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
        return Err(ApiError::unavailable("service target port must not be 0"));
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

/// A route the realizer wants on an ingress listener, before its pool exists.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RoutePlan {
    host: Option<String>,
    path: String,
    path_match: PathMatch,
    members: Vec<SocketAddr>,
}

/// Default-backend members and one route per rule path, in spec order.
fn ingress_plan(
    client: &Client,
    ingress: &Ingress,
) -> ApiResult<(Vec<SocketAddr>, Vec<RoutePlan>)> {
    let mut routes = Vec::new();
    for rule in &ingress.spec.rules {
        let host = rule
            .host
            .as_deref()
            .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|h| !h.is_empty());
        for path in &rule.paths {
            routes.push(RoutePlan {
                host: host.clone(),
                path: if path.path.is_empty() {
                    "/".into()
                } else {
                    path.path.clone()
                },
                path_match: match path.path_type {
                    PathType::Prefix => PathMatch::Prefix,
                    PathType::Exact => PathMatch::Exact,
                },
                members: one_backend(client, ingress, &path.backend)?,
            });
        }
    }
    let default = match &ingress.spec.default_backend {
        Some(backend) => one_backend(client, ingress, backend)?,
        None => Vec::new(),
    };
    if default.is_empty() && routes.is_empty() {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    Ok((default, routes))
}

fn one_backend(
    client: &Client,
    ingress: &Ingress,
    backend: &IngressBackend,
) -> ApiResult<Vec<SocketAddr>> {
    let services = client.store().collection::<Service>();
    let svc = if let Some(id) = Id::parse(kinds::SERVICE, &backend.service) {
        services
            .get(&id)
            .map_err(|_| ApiError::unavailable("ingress backend is not available"))?
    } else {
        services
            .get_by_name(ingress.meta.project.as_ref(), &backend.service)
            .map_err(|_| ApiError::unavailable("ingress backend is not available"))?
    };
    if !projects_match(&ingress.meta.project, &svc.meta.project) {
        return Err(ApiError::unavailable("ingress backend is not available"));
    }
    let Some(port) = svc
        .spec
        .ports
        .iter()
        .find(|p| port_matches(p, &backend.port))
    else {
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

#[allow(clippy::too_many_arguments)]
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
    bind: Option<SocketAddr>,
    members: &[SocketAddr],
    routes: &[RoutePlan],
) -> ApiResult<String> {
    let lb = match client.balancers().get_by_name(project.as_ref(), name) {
        Ok(lb) => {
            if lb.spec.vip_network != VIP_NETWORK
                || lb.spec.provider != provider
                || lb.spec.vip_address != bind.map(|b| b.ip())
                || lb.spec.vip_port != bind.map(|b| b.port())
            {
                return Err(ApiError::unavailable("listener is not available"));
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
                    vip_address: bind.map(|b| b.ip()),
                    vip_port: bind.map(|b| b.port()),
                    provider,
                    admin_up: true,
                    ..BalancerSpec::default()
                },
            )?
        }
        Err(e) => return Err(e.into()),
    };

    let pools = client.pools_of(&lb.meta.id)?;
    let default_name = format!("{name}-pool");
    let route_prefix = format!("{name}-route-");
    let find_or_create = |pool_name: &str| -> ApiResult<relay::Pool> {
        if let Some(pool) = pools.iter().find(|p| p.meta.name == pool_name) {
            return Ok(pool.clone());
        }
        client.create_pool(
            Meta::new(relay::types::kinds::POOL, pool_name, project.clone()),
            PoolSpec {
                balancer: lb.meta.id.clone(),
                protocol: pool_protocol,
                algorithm: Algorithm::RoundRobin,
                session_persistence: None,
                health_monitor: None,
                admin_up: true,
            },
        )
    };
    let pool = match pools.iter().find(|p| p.meta.name == default_name) {
        Some(pool) => pool.clone(),
        None if routes.is_empty() && pools.len() == 1 => pools[0].clone(),
        None => find_or_create(&default_name)?,
    };
    let mut route_pools = Vec::with_capacity(routes.len());
    let mut want_routes = Vec::with_capacity(routes.len());
    for (i, plan) in routes.iter().enumerate() {
        let route_pool = find_or_create(&format!("{route_prefix}{i}"))?;
        want_routes.push(Route {
            host: plan.host.clone(),
            path: plan.path.clone(),
            path_match: plan.path_match,
            pool: route_pool.meta.id.clone(),
        });
        route_pools.push(route_pool);
    }

    let listeners = client.listeners_of(&lb.meta.id)?;
    let listener = match listeners
        .into_iter()
        .find(|l| l.spec.default_pool.as_ref() == Some(&pool.meta.id))
    {
        Some(l)
            if l.spec.port == port
                && l.spec.protocol == listener_protocol
                && l.spec.routes == want_routes =>
        {
            l
        }
        Some(l) => {
            let protocol = listener_protocol.clone();
            let next_routes = want_routes.clone();
            client.update_listener(&l.meta.id, None, move |row| {
                row.spec.port = port;
                row.spec.protocol = protocol;
                row.spec.routes = next_routes;
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
                routes: want_routes,
            },
        )?,
    };

    // Route pools past the current rule count are no longer referenced.
    for old in &pools {
        let Some(index) = old.meta.name.strip_prefix(&route_prefix) else {
            continue;
        };
        if index.parse::<usize>().is_ok_and(|i| i >= routes.len()) {
            client.delete_pool(&old.meta.id)?;
        }
    }

    let changed = sync_members(client, &project, name, &pool.meta.id, members)?;
    let mut stale_routes = Vec::new();
    for (route_pool, plan) in route_pools.iter().zip(routes) {
        if sync_members(
            client,
            &project,
            &route_pool.meta.name,
            &route_pool.meta.id,
            &plan.members,
        )? {
            stale_routes.push(route_pool.meta.id.clone());
        }
    }

    let provider_handle = client.provider_for(&lb)?;
    let live = provider_handle.ensure_balancer(&lb).await?;
    let lb = client
        .balancers()
        .get(&lb.meta.id)
        .map_err(ApiError::from)?;
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
    for route_pool in &route_pools {
        if !aligned || stale_routes.contains(&route_pool.meta.id) {
            client.touch_pool(&route_pool.meta.id);
            client.reconcile_pool(&route_pool.meta.id).await;
        }
    }

    let lb = client
        .balancers()
        .get(&lb.meta.id)
        .map_err(ApiError::from)?;
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
    // A udp socket cannot be probed with a connect; its bind is the proof.
    if listener.spec.protocol != ListenerProtocol::Udp && !listener_answers(&addr).await {
        return Err(ApiError::unavailable("listener is not available"));
    }
    Ok(addr)
}

/// The keep certificate an ingress's `tls` entries name, if any.
///
/// Each entry's `secret_name` is a keep certificate name (or id) in the
/// ingress's project. One listener presents one certificate, so every
/// entry must name the same one.
fn ingress_cert(client: &Client, ingress: &Ingress) -> ApiResult<Option<Id>> {
    let mut names: Vec<&str> = Vec::new();
    for tls in &ingress.spec.tls {
        let name = tls.secret_name.trim();
        if name.is_empty() {
            return Err(ApiError::unprocessable(
                "ingress tls entry needs a certificate name",
            ));
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let name = match names.as_slice() {
        [] => return Ok(None),
        [one] => *one,
        many => {
            return Err(ApiError::unprocessable(format!(
                "ingress tls names {} certificates ({}); one certificate per ingress is supported",
                many.len(),
                many.join(", ")
            )))
        }
    };
    let project = ingress.meta.project.as_ref();
    let missing = || ApiError::unavailable(format!("ingress certificate '{name}' not found"));
    let cert = match client
        .store()
        .collection::<keep::Certificate>()
        .resolve(project, name)
    {
        Ok(cert) => cert,
        Err(StoreError::NotFound(_)) => return Err(missing()),
        Err(e) => return Err(e.into()),
    };
    if cert.meta.project.as_ref() != project {
        return Err(missing());
    }
    Ok(Some(cert.meta.id))
}

/// Make `pool`'s members exactly `members`. True when anything changed.
fn sync_members(
    client: &Client,
    project: &Option<Id>,
    prefix: &str,
    pool: &Id,
    members: &[SocketAddr],
) -> ApiResult<bool> {
    let have = client.members_of(pool)?;
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
                    format!("{prefix}-{}", member_slug(want)),
                    project.clone(),
                ),
                MemberSpec {
                    pool: pool.clone(),
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
    Ok(changed)
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
        if lb.meta.labels.get(label) == Some(&id.to_owned())
            && !keep.iter().any(|n| n == &lb.meta.name)
        {
            client.delete_balancer(&lb.meta.id).await?;
        }
    }
    Ok(())
}
