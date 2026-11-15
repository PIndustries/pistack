// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/network`.
//!
//! Plain nouns, one shape for every kind:
//! `GET|POST /{nouns}`, `GET|PUT|DELETE /{nouns}/{name-or-id}`, plus the
//! few verbs that are not a spec edit (router interfaces, security group
//! rules, floating IP association). Handlers only validate the caller's
//! scope and hand off to `Client`; they never write `status`.
//!
//! The vpc has the same shape under `/vpcs`, and one shortcut:
//! `POST /vpcs/{id}/subnets` makes a network and its one subnet together.

use crate::client::Client;
use crate::types::{self, *};
use crate::vpc;
use axum::{
    extract::{Extension, Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json,
};
use pistack_types::{kind, ApiError, ApiResult, Caller, Id, Kind, ListQuery, Meta, Page};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

pub fn router(client: Client) -> axum::Router {
    axum::Router::new()
        .route("/", get(info))
        .route("/vpcs", get(list_vpcs).post(create_vpc))
        .route(
            "/vpcs/{id}",
            get(get_vpc).put(update_vpc).delete(delete_vpc),
        )
        .route(
            "/vpcs/{id}/subnets",
            get(list_vpc_subnets).post(create_vpc_subnet),
        )
        .route(
            "/vpcs/{id}/subnets/{subnet}",
            axum::routing::delete(delete_vpc_subnet),
        )
        .route("/networks", get(list_networks).post(create_network))
        .route(
            "/networks/{id}",
            get(get_network).put(update_network).delete(delete_network),
        )
        .route("/subnets", get(list_subnets).post(create_subnet))
        .route(
            "/subnets/{id}",
            get(get_subnet).put(update_subnet).delete(delete_subnet),
        )
        .route("/ports", get(list_ports).post(create_port))
        .route(
            "/ports/{id}",
            get(get_port).put(update_port).delete(delete_port),
        )
        .route("/routers", get(list_routers).post(create_router))
        .route(
            "/routers/{id}",
            get(get_router).put(update_router).delete(delete_router),
        )
        .route("/routers/{id}/interfaces", put(set_router_interfaces))
        .route("/security-groups", get(list_sgs).post(create_sg))
        .route(
            "/security-groups/{id}",
            get(get_sg).put(update_sg).delete(delete_sg),
        )
        .route("/security-groups/{id}/rules", post(add_rule))
        .route(
            "/security-groups/{id}/rules/{rule}",
            axum::routing::delete(remove_rule),
        )
        .route("/floating-ips", get(list_fips).post(create_fip))
        .route(
            "/floating-ips/{id}",
            get(get_fip).put(update_fip).delete(delete_fip),
        )
        .route("/floating-ips/{id}/associate", post(associate_fip))
        .route("/floating-ips/{id}/disassociate", post(disassociate_fip))
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}

/// The gateway attaches a `Caller`. A request without one is refused.
async fn require_caller(req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        return ApiError::unauthorized("no credentials").into_response();
    }
    next.run(req).await
}

// ---- request / response shapes ---------------------------------------------

#[derive(Deserialize)]
pub struct Create<S> {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Admins may create in any project (or none); others inherit scope.
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: S,
}

#[derive(Deserialize)]
pub struct Update<S> {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<BTreeMap<String, String>>,
    pub spec: S,
}

#[derive(Deserialize, Default)]
pub struct ScopeQuery {
    /// Admin-only: act in this project instead of the token's.
    #[serde(default)]
    pub project: Option<Id>,
    /// Admin-only: list every project.
    #[serde(default)]
    pub all: Option<bool>,
}

/// `GET /networks` and `GET /subnets` beside the usual list parameters.
#[derive(Deserialize, Default)]
pub struct NetFilter {
    /// Only what belongs to this vpc, by id or by name. `none` is what has
    /// no vpc at all.
    #[serde(default)]
    pub vpc: Option<String>,
}

/// The body of `POST /vpcs/{id}/subnets`. The vpc is the one in the path.
#[derive(Deserialize)]
pub struct VpcSubnet {
    pub name: String,
    /// A full cidr, or a prefix length — `24` or `/24` — for the next free
    /// block of that size.
    pub cidr: String,
    #[serde(default)]
    pub gateway: Option<IpAddr>,
    #[serde(default)]
    pub dns_nameservers: Vec<IpAddr>,
    #[serde(default = "default_dhcp")]
    pub enable_dhcp: bool,
}

fn default_dhcp() -> bool {
    true
}

#[derive(Deserialize)]
pub struct Interfaces {
    pub subnets: Vec<Id>,
}

#[derive(Deserialize)]
pub struct Associate {
    /// Existing port. Exactly one of `port` or `machine` is set.
    #[serde(default)]
    pub port: Option<Id>,
    /// Machine whose current IPv4 becomes the fixed address.
    #[serde(default)]
    pub machine: Option<Id>,
}

#[derive(Serialize)]
struct Info {
    module: &'static str,
    backend: &'static str,
    resources: &'static [&'static str],
}

async fn info(State(c): State<Client>) -> Json<Info> {
    Json(Info {
        module: "fabric",
        backend: c.backend().name(),
        resources: KINDS,
    })
}

// ---- scope helpers --------------------------------------------------------

/// Resolve which project a list/get/create acts on.
fn scope(caller: &Caller, q: &ScopeQuery) -> ApiResult<Option<Id>> {
    if caller.is_admin() {
        if q.all == Some(true) {
            return Ok(None);
        }
        return Ok(q.project.clone().or_else(|| caller.project.clone()));
    }
    if q.project.is_some() && q.project != caller.project {
        return Err(ApiError::forbidden("project scope does not match token"));
    }
    caller
        .project
        .clone()
        .map(Some)
        .ok_or_else(|| ApiError::forbidden("token has no project scope"))
}

/// Project for a create: body wins for admins, token wins otherwise.
fn create_project(caller: &Caller, body: &Option<Id>) -> ApiResult<Option<Id>> {
    match (caller.is_admin(), body, &caller.project) {
        (true, Some(p), _) => Ok(Some(p.clone())),
        (true, None, p) => Ok(p.clone()),
        (false, Some(p), Some(mine)) if p == mine => Ok(Some(p.clone())),
        (false, None, Some(mine)) => Ok(Some(mine.clone())),
        (false, _, _) => Err(ApiError::forbidden(
            "cannot create outside the token's project",
        )),
    }
}

/// The caller may see a resource in their project, a cloud-wide one, or
/// anything if admin. Others get not_found, not forbidden.
fn visible<K: Kind>(caller: &Caller, r: K) -> ApiResult<K> {
    let p = &r.meta().project;
    if caller.is_admin() || p.is_none() || *p == caller.project {
        Ok(r)
    } else {
        Err(ApiError::not_found(format!(
            "{} {} not found",
            K::KIND,
            r.meta().id
        )))
    }
}

fn if_match(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    match headers.get(header::IF_MATCH) {
        None => Ok(None),
        Some(v) => {
            let s = v
                .to_str()
                .map_err(|_| ApiError::bad_request("If-Match is not ascii"))?;
            let s = s.trim().trim_matches('"');
            s.parse::<u64>()
                .map(Some)
                .map_err(|_| ApiError::bad_request("If-Match must be a revision number"))
        }
    }
}

fn new_meta<S>(k: &'static str, c: &Create<S>, project: Option<Id>) -> ApiResult<Meta> {
    if c.name.trim().is_empty() || c.name.len() > 128 || c.name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let mut m = Meta::new(k, c.name.trim(), project);
    m.description = c.description.clone();
    m.labels = c.labels.clone();
    m.annotations = c.annotations.clone();
    Ok(m)
}

fn apply_meta<S>(m: &mut Meta, u: &Update<S>) {
    if let Some(n) = &u.name {
        m.name = n.trim().to_owned();
    }
    if let Some(d) = &u.description {
        m.description = d.clone();
    }
    if let Some(l) = &u.labels {
        m.labels = l.clone();
    }
    if let Some(a) = &u.annotations {
        m.annotations = a.clone();
    }
}

/// Filter by name/labels, then page by id (ids are time-ordered).
fn page<K: Kind>(items: Vec<K>, q: &ListQuery) -> Page<K> {
    page_by(items, q, |_| true)
}

/// [`page`] with one more question every item has to answer.
fn page_by<K: Kind>(mut items: Vec<K>, q: &ListQuery, mut keep: impl FnMut(&K) -> bool) -> Page<K> {
    let sel = q.label_selector();
    items.retain(|k| {
        let m = k.meta();
        keep(k)
            && q.name.as_deref().is_none_or(|n| m.name == n)
            && sel.iter().all(|(lk, lv)| m.labels.get(lk) == Some(lv))
    });
    items.sort_by(|a, b| a.meta().id.cmp(&b.meta().id));
    let total = items.len();
    if let Some(after) = &q.after {
        items.retain(|k| k.meta().id.to_string().as_str() > after.as_str());
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let next = if items.len() > limit {
        items.get(limit - 1).map(|k| k.meta().id.to_string())
    } else {
        None
    };
    items.truncate(limit);
    Page { items, next, total }
}

fn created<T: Serialize>(t: T) -> Response {
    (StatusCode::CREATED, Json(t)).into_response()
}

// ---- what only an admin may put in a spec ---------------------------------

/// Checked before a create (`cur` is `None`) or an update.
trait Guarded: Sized {
    fn guard(&self, _caller: &Caller, _cur: Option<&Self>) -> ApiResult<()> {
        Ok(())
    }
}

impl Guarded for VpcSpec {
    /// The blocks are the vpc: a create that names none has nothing to cut
    /// subnets from.
    fn guard(&self, caller: &Caller, cur: Option<&Self>) -> ApiResult<()> {
        let _ = caller;
        if cur.is_none() {
            vpc::cidrs_required(self)?;
        }
        Ok(())
    }
}
impl Guarded for NetworkSpec {
    /// Turning automatic floating IPs on makes machines in every project
    /// reachable from outside, so it is an admin's call. Anyone who may
    /// edit the network can turn it off.
    fn guard(&self, caller: &Caller, cur: Option<&Self>) -> ApiResult<()> {
        let was_on = cur.is_some_and(|c| c.auto_floating_ip);
        if self.auto_floating_ip && !was_on && !caller.is_admin() {
            return Err(ApiError::forbidden(
                "only an admin may turn on auto_floating_ip",
            ));
        }
        Ok(())
    }
}
impl Guarded for SubnetSpec {}
impl Guarded for PortSpec {}
impl Guarded for RouterSpec {}
impl Guarded for SecurityGroupSpec {}
impl Guarded for FloatingIpSpec {}

// ---- one macro per kind for the uniform CRUD ------------------------------

macro_rules! crud {
    (
        $ty:ty, $spec:ty, $kind:expr, $col:ident,
        $list:ident, $create:ident, $get:ident, $update:ident, $delete:ident,
        $c_create:ident, $c_update:ident, $c_delete:ident
    ) => {
        crud!(
            $ty, $spec, $kind, $col, $list, $create, $get, $update, $delete, $c_create, $c_update,
            $c_delete, no_vpc
        );
    };
    (
        $ty:ty, $spec:ty, $kind:expr, $col:ident,
        $list:ident, $create:ident, $get:ident, $update:ident, $delete:ident,
        $c_create:ident, $c_update:ident, $c_delete:ident,
        $net_of:ident
    ) => {
        async fn $list(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Query(q): Query<ListQuery>,
            Query(f): Query<NetFilter>,
        ) -> ApiResult<Json<Page<$ty>>> {
            let project = scope(&caller, &sq)?;
            let want = InWhich::parse(&c, &caller, project.as_ref(), &f.vpc)?;
            let items = c.$col().list(project.as_ref())?;
            Ok(Json(page_by(items, &q, |x| {
                want.holds($net_of(&c, x).as_ref())
            })))
        }

        async fn $create(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Json(body): Json<Create<$spec>>,
        ) -> ApiResult<Response> {
            let project = create_project(&caller, &body.project)?;
            body.spec.guard(&caller, None)?;
            let meta = new_meta($kind, &body, project)?;
            Ok(created(c.$c_create(meta, body.spec)?))
        }

        async fn $get(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
        ) -> ApiResult<Json<$ty>> {
            let project = scope(&caller, &sq)?;
            Ok(Json(visible(
                &caller,
                c.$col().resolve(project.as_ref(), &id)?,
            )?))
        }

        async fn $update(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
            headers: HeaderMap,
            Json(body): Json<Update<$spec>>,
        ) -> ApiResult<Json<$ty>> {
            let project = scope(&caller, &sq)?;
            let cur = visible(&caller, c.$col().resolve(project.as_ref(), &id)?)?;
            let rev = if_match(&headers)?;
            body.spec.guard(&caller, Some(&cur.spec))?;
            Ok(Json(c.$c_update(&cur.meta.id, rev, |r| {
                apply_meta(&mut r.meta, &body);
                r.spec = body.spec.clone();
                Ok(())
            })?))
        }

        async fn $delete(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
        ) -> ApiResult<StatusCode> {
            let project = scope(&caller, &sq)?;
            let cur = visible(&caller, c.$col().resolve(project.as_ref(), &id)?)?;
            c.$c_delete(&cur.meta.id)?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}

crud!(
    Network,
    NetworkSpec,
    kind::NETWORK,
    networks,
    list_networks,
    create_network,
    get_network,
    update_network,
    delete_network,
    create_network,
    update_network,
    delete_network,
    own_vpc
);
crud!(
    Subnet,
    SubnetSpec,
    kind::SUBNET,
    subnets,
    list_subnets,
    create_subnet,
    get_subnet,
    update_subnet,
    delete_subnet,
    create_subnet,
    update_subnet,
    delete_subnet,
    // A subnet is in a vpc through the network it sits on.
    vpc_of_subnet
);
crud!(
    Port,
    PortSpec,
    kind::PORT,
    ports,
    list_ports,
    create_port,
    get_port,
    update_port,
    delete_port,
    create_port,
    update_port,
    delete_port
);
crud!(
    types::Router,
    RouterSpec,
    kind::ROUTER,
    routers,
    list_routers,
    create_router,
    get_router,
    update_router,
    delete_router,
    create_router,
    update_router,
    delete_router
);
crud!(
    SecurityGroup,
    SecurityGroupSpec,
    kind::SECGROUP,
    security_groups,
    list_sgs,
    create_sg,
    get_sg,
    update_sg,
    delete_sg,
    create_security_group,
    update_security_group,
    delete_security_group
);
crud!(
    FloatingIp,
    FloatingIpSpec,
    kind::FLOATING_IP,
    floating_ips,
    list_fips,
    create_fip,
    get_fip,
    update_fip,
    delete_fip,
    create_floating_ip,
    update_floating_ip,
    delete_floating_ip
);

/// The vpc a network names in its spec.
fn own_vpc(c: &Client, n: &Network) -> Option<Id> {
    let _ = c;
    n.spec.vpc.clone()
}

/// The vpc a subnet's network belongs to.
fn vpc_of_subnet(c: &Client, s: &Subnet) -> Option<Id> {
    c.networks().get(&s.spec.network).ok()?.spec.vpc
}

/// Kinds that cannot be in a vpc at all.
fn no_vpc(_: &Client, _: &impl Kind) -> Option<Id> {
    None
}

// ---- vpcs ------------------------------------------------------------------

/// What `?vpc=` asks for: no filter, no vpc, or one named vpc.
enum InWhich {
    Any,
    None,
    One(Id),
}

impl InWhich {
    /// Read `?vpc=`: a vpc's name, its id, or `none`. Admins list across
    /// projects, where a name resolves against no project of its own, so a
    /// name is looked for in the scope first and across the cloud after.
    fn parse(
        c: &Client,
        caller: &Caller,
        project: Option<&Id>,
        want: &Option<String>,
    ) -> ApiResult<InWhich> {
        let Some(w) = want else {
            return Ok(InWhich::Any);
        };
        if w == "none" {
            return Ok(InWhich::None);
        }
        if let Some(id) = Id::parse(kind::VPC, w) {
            return Ok(InWhich::One(visible(caller, c.vpcs().get(&id)?)?.meta.id));
        }
        let vpc = c
            .vpcs()
            .get_by_name(project, w)
            .or_else(|_| c.vpcs().get_by_name(None, w))
            .map_err(|_| ApiError::not_found(format!("no vpc named '{w}'")))?;
        Ok(InWhich::One(visible(caller, vpc)?.meta.id))
    }

    fn holds(&self, of: Option<&Id>) -> bool {
        match self {
            InWhich::Any => true,
            InWhich::None => of.is_none(),
            InWhich::One(id) => of == Some(id),
        }
    }
}

async fn list_vpcs(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Vpc>>> {
    let project = scope(&caller, &sq)?;
    let items = c.vpcs().list(project.as_ref())?;
    Ok(Json(page(items, &q)))
}

async fn create_vpc(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Create<VpcSpec>>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    body.spec.guard(&caller, None)?;
    let meta = new_meta(kind::VPC, &body, project)?;
    Ok(created(c.create_vpc(meta, body.spec)?))
}

async fn get_vpc(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vpc>> {
    let project = scope(&caller, &sq)?;
    Ok(Json(visible(
        &caller,
        c.vpcs().resolve(project.as_ref(), &id)?,
    )?))
}

async fn update_vpc(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Update<VpcSpec>>,
) -> ApiResult<Json<Vpc>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.vpcs().resolve(project.as_ref(), &id)?)?;
    let rev = if_match(&headers)?;
    body.spec.guard(&caller, Some(&cur.spec))?;
    Ok(Json(c.update_vpc(&cur.meta.id, rev, |v| {
        apply_meta(&mut v.meta, &body);
        v.spec = body.spec.clone();
        Ok(())
    })?))
}

async fn delete_vpc(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.vpcs().resolve(project.as_ref(), &id)?)?;
    c.delete_vpc(&cur.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_vpc_subnets(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<Page<Subnet>>> {
    let project = scope(&caller, &sq)?;
    let vpc = visible(&caller, c.vpcs().resolve(project.as_ref(), &id)?)?;
    let items = c.subnets_of_vpc(&vpc.meta.id)?;
    Ok(Json(page(items, &q)))
}

/// The short road: a network and its one subnet, named alike, joined to the
/// vpc. `cidr` is a block or a prefix length to take from the vpc.
async fn create_vpc_subnet(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(mut body): Json<VpcSubnet>,
) -> ApiResult<Response> {
    // The vpc is the one the path names, and what is made lives in its
    // project: a vpc's networks and subnets never live apart from it.
    let asked = create_project(&caller, &sq.project)?;
    let path = visible(&caller, c.vpcs().resolve(asked.as_ref(), &id)?)?;
    let project = match &path.meta.project {
        Some(p) if asked.as_ref().is_some_and(|a| a != p) && !caller.is_admin() => {
            return Err(ApiError::forbidden(
                "the vpc belongs to another project than the token's",
            ));
        }
        Some(p) => Some(p.clone()),
        None => asked,
    };
    let name = body.name.trim().to_owned();
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let cidr = vpc::carve(&path, &c.subnets_of_vpc(&path.meta.id)?, &body.cidr)?;
    let dns = std::mem::take(&mut body.dns_nameservers);
    let gateway = body.gateway;
    let dhcp = body.enable_dhcp;
    let net = c.create_network(
        Meta::new(kind::NETWORK, name.clone(), project.clone()),
        NetworkSpec {
            vpc: Some(path.meta.id.clone()),
            dns_domain: path.spec.dns_domain.clone(),
            ..NetworkSpec::default()
        },
    )?;
    let made = c.create_subnet(
        Meta::new(kind::SUBNET, name, project),
        SubnetSpec {
            network: net.meta.id.clone(),
            cidr,
            gateway,
            allocation_pools: Vec::new(),
            dns_nameservers: dns,
            enable_dhcp: dhcp,
            host_routes: Vec::new(),
        },
    );
    match made {
        Ok(sub) => Ok(created(sub)),
        Err(e) => {
            let _ = c.delete_network(&net.meta.id);
            Err(e)
        }
    }
}

async fn delete_vpc_subnet(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path((id, subnet)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let project = scope(&caller, &sq)?;
    let vpc = visible(&caller, c.vpcs().resolve(project.as_ref(), &id)?)?;
    let sub = visible(&caller, c.subnets().resolve(project.as_ref(), &subnet)?)?;
    let net = c.networks().get(&sub.spec.network)?;
    if net.spec.vpc.as_ref() != Some(&vpc.meta.id) {
        return Err(ApiError::not_found(format!(
            "subnet {subnet} is not in vpc {}",
            vpc.meta.id
        )));
    }
    c.delete_subnet(&sub.meta.id)?;
    let _ = c.delete_network(&net.meta.id);
    Ok(StatusCode::NO_CONTENT)
}

// ---- verbs ----------------------------------------------------------------

async fn set_router_interfaces(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Interfaces>,
) -> ApiResult<Json<types::Router>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.routers().resolve(project.as_ref(), &id)?)?;
    Ok(Json(c.set_router_interfaces(
        &cur.meta.id,
        if_match(&headers)?,
        &body.subnets,
    )?))
}

async fn add_rule(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(rule): Json<Rule>,
) -> ApiResult<Response> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.security_groups().resolve(project.as_ref(), &id)?)?;
    Ok(created(c.add_rule(&cur.meta.id, rule)?))
}

async fn remove_rule(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path((id, rule)): Path<(String, uuid::Uuid)>,
) -> ApiResult<Json<SecurityGroup>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.security_groups().resolve(project.as_ref(), &id)?)?;
    Ok(Json(c.remove_rule(&cur.meta.id, rule)?))
}

async fn associate_fip(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(body): Json<Associate>,
) -> ApiResult<Json<FloatingIp>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.floating_ips().resolve(project.as_ref(), &id)?)?;
    let fip = match (body.port, body.machine) {
        (Some(port), None) => c.associate_floating_ip(&cur.meta.id, Some(port))?,
        (None, Some(machine)) => c.associate_to_machine(&cur.meta.id, &machine)?,
        _ => {
            return Err(ApiError::bad_request(
                "associate needs exactly one of port or machine",
            ))
        }
    };
    Ok(Json(fip))
}

async fn disassociate_fip(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<FloatingIp>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.floating_ips().resolve(project.as_ref(), &id)?)?;
    Ok(Json(c.associate_floating_ip(&cur.meta.id, None)?))
}
