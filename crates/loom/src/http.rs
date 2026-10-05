// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `loom` API: plain-noun CRUD over every resource plus a few verbs.
//!
//! Handlers write `spec` and `meta` only; controllers own `status`.
//! Deleting a resource that has children or a machine behind it sets
//! `DELETE_ANNOTATION` and returns `202 Accepted`; the controller tears it
//! down and removes it. ConfigMaps and Ingresses delete immediately (`204`).
//!
//! Project scoping reads `pistack_types::Caller` from request extensions
//! when the gateway's auth layer attached one. With no `Caller` the request
//! is treated as cloud admin in dev mode and rejected otherwise.

use crate::controllers::deployment::{owned_sets, revision_of};
use crate::cron::Schedule;
use crate::selector::template_hash;
use crate::types::*;
use axum::{
    extract::{FromRequest, FromRequestParts, Path, Request, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use pistack_rpc::proto::CommandKind;
use pistack_rpc::CommandQueue;
use pistack_store::{Collection, Store};
use pistack_types::{ApiError, ApiResult, Caller, Id, Kind, ListQuery, Page, Resource, Spec};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    /// Treat unauthenticated requests as cloud admin.
    pub dev_admin: bool,
    /// `None` until the server supplies the command queue.
    pub queue: Option<CommandQueue>,
}

pub fn router(state: AppState) -> Router {
    let r = Router::new().route("/", get(index));
    let r = crud::<PodSpec>(r)
        .route("/pods/{id}/logs", get(pod_logs))
        .route("/pods/{id}/sandbox", get(pod_sandbox));
    let r = crud::<ReplicaSetSpec>(r);
    let r = crud::<DeploymentSpec>(r)
        .route("/deployments/{id}/scale", post(scale_deployment))
        .route("/deployments/{id}/rollback", post(rollback_deployment));
    let r = crud::<ServiceSpec>(r);
    let r = crud::<ConfigMapSpec>(r);
    let r = crud::<JobSpec>(r);
    let r = crud::<CronJobSpec>(r);
    let r = crud::<IngressSpec>(r);
    r.with_state(state)
}

fn crud<A: Api>(r: Router<AppState>) -> Router<AppState> {
    r.route(&format!("/{}", A::NOUN), get(list::<A>).post(create::<A>))
        .route(
            &format!("/{}/{{id}}", A::NOUN),
            get(get_one::<A>).put(update::<A>).delete(delete::<A>),
        )
}

// ---- the per-resource contract -------------------------------------------------

/// How DELETE behaves for a kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMode {
    /// Remove from the store now.
    Direct,
    /// Set `DELETE_ANNOTATION`; a controller finishes the job.
    Annotate,
}

/// What the generic handlers need from each spec type.
pub trait Api: Spec + Serialize + DeserializeOwned {
    type Status: crate::controllers::Status;
    /// URL noun, plural: `pods`.
    const NOUN: &'static str;
    const DELETE: DeleteMode;
    /// Normalise and validate a spec about to be stored. `current` is the
    /// stored resource on update.
    fn validate(&mut self, current: Option<&Resource<Self, Self::Status>>) -> ApiResult<()>;
}

fn validate_pod_spec(spec: &PodSpec) -> ApiResult<()> {
    if spec.containers.is_empty() {
        return Err(ApiError::bad_request("spec.containers must not be empty"));
    }
    let mut names = BTreeSet::new();
    for c in &spec.containers {
        validate_name(&c.name)
            .map_err(|e| ApiError::bad_request(format!("container name: {}", e.message)))?;
        if !names.insert(c.name.as_str()) {
            return Err(ApiError::bad_request(format!(
                "container name '{}' is used twice",
                c.name
            )));
        }
        if c.image.trim().is_empty() {
            return Err(ApiError::bad_request(format!(
                "container '{}' needs an image",
                c.name
            )));
        }
        for vm in &c.volume_mounts {
            if !spec.volumes.iter().any(|v| v.name == vm.name) {
                return Err(ApiError::bad_request(format!(
                    "container '{}' mounts unknown volume '{}'",
                    c.name, vm.name
                )));
            }
        }
    }
    let mut vols = BTreeSet::new();
    for v in &spec.volumes {
        if !vols.insert(v.name.as_str()) {
            return Err(ApiError::bad_request(format!(
                "volume name '{}' is used twice",
                v.name
            )));
        }
    }
    Ok(())
}

/// Default an empty selector to the template labels and require it to match.
fn validate_selector(selector: &mut Selector, template: &PodTemplate) -> ApiResult<()> {
    if selector.is_empty() {
        if template.labels.is_empty() {
            return Err(ApiError::bad_request(
                "spec.selector is empty and spec.template.labels has nothing to default it from",
            ));
        }
        *selector = Selector::from_labels(&template.labels);
    }
    if !selector.matches(&template.labels) {
        return Err(ApiError::bad_request(
            "spec.selector does not match spec.template.labels",
        ));
    }
    Ok(())
}

impl Api for PodSpec {
    type Status = PodStatus;
    const NOUN: &'static str = "pods";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&Pod>) -> ApiResult<()> {
        validate_pod_spec(self)?;
        if let Some(cur) = current {
            if cur.status.machine.is_some() && *self != cur.spec {
                return Err(ApiError::unprocessable(
                    "pod spec is immutable once its machine exists; only labels and annotations may change",
                ));
            }
        }
        Ok(())
    }
}

impl Api for ReplicaSetSpec {
    type Status = ReplicaSetStatus;
    const NOUN: &'static str = "replicasets";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, _current: Option<&ReplicaSet>) -> ApiResult<()> {
        validate_pod_spec(&self.template.spec)?;
        validate_selector(&mut self.selector, &self.template)
    }
}

impl Api for DeploymentSpec {
    type Status = DeploymentStatus;
    const NOUN: &'static str = "deployments";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&Deployment>) -> ApiResult<()> {
        validate_pod_spec(&self.template.spec)?;
        if self.template.labels.contains_key(POD_TEMPLATE_HASH) {
            return Err(ApiError::bad_request(format!(
                "spec.template.labels may not set '{POD_TEMPLATE_HASH}'"
            )));
        }
        validate_selector(&mut self.selector, &self.template)?;
        if let Some(cur) = current {
            if self.selector != cur.spec.selector {
                return Err(ApiError::unprocessable("spec.selector is immutable"));
            }
        }
        if let DeploymentStrategy::RollingUpdate {
            max_surge,
            max_unavailable,
        } = &self.strategy
        {
            for (name, v) in [
                ("max_surge", max_surge),
                ("max_unavailable", max_unavailable),
            ] {
                if let IntOrPercent::Percent(p) = v {
                    let ok = p
                        .trim()
                        .strip_suffix('%')
                        .and_then(|n| n.trim().parse::<u32>().ok())
                        .is_some_and(|n| n <= 100);
                    if !ok {
                        return Err(ApiError::bad_request(format!(
                            "strategy.{name} must be an integer or '<0-100>%'"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

impl Api for ServiceSpec {
    type Status = ServiceStatus;
    const NOUN: &'static str = "services";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&Service>) -> ApiResult<()> {
        if self.ports.is_empty() {
            return Err(ApiError::bad_request("spec.ports must not be empty"));
        }
        let mut seen = BTreeSet::new();
        let mut node_ports = BTreeSet::new();
        for p in &self.ports {
            if p.port == 0 {
                return Err(ApiError::bad_request("port must be 1-65535"));
            }
            if !seen.insert((p.port, format!("{:?}", p.protocol))) {
                return Err(ApiError::bad_request(format!(
                    "port {} is listed twice",
                    p.port
                )));
            }
            if let Some(node_port) = p.node_port {
                if self.kind == ServiceKind::ClusterIp {
                    return Err(ApiError::bad_request(
                        "node_port needs kind NodePort or LoadBalancer",
                    ));
                }
                if !NODE_PORT_RANGE.contains(&node_port) {
                    return Err(ApiError::bad_request(format!(
                        "node_port {node_port} must be {}-{}",
                        NODE_PORT_RANGE.start(),
                        NODE_PORT_RANGE.end()
                    )));
                }
                if !node_ports.insert(node_port) {
                    return Err(ApiError::bad_request(format!(
                        "node_port {node_port} is listed twice"
                    )));
                }
            }
        }
        if let Some(ip) = &self.cluster_ip {
            ip.parse::<std::net::IpAddr>()
                .map_err(|_| ApiError::bad_request("spec.cluster_ip is not an IP address"))?;
            if let Some(cur) = current {
                if cur.status.cluster_ip.is_some() && cur.status.cluster_ip != self.cluster_ip {
                    return Err(ApiError::unprocessable(
                        "spec.cluster_ip cannot change once allocated",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Api for ConfigMapSpec {
    type Status = ConfigMapStatus;
    const NOUN: &'static str = "configmaps";
    const DELETE: DeleteMode = DeleteMode::Direct;
    fn validate(&mut self, current: Option<&ConfigMap>) -> ApiResult<()> {
        if let Some(cur) = current {
            if cur.spec.immutable
                && (self.data != cur.spec.data || self.binary_data != cur.spec.binary_data)
            {
                return Err(ApiError::unprocessable("configmap is immutable"));
            }
        }
        Ok(())
    }
}

impl Api for JobSpec {
    type Status = JobStatus;
    const NOUN: &'static str = "jobs";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&Job>) -> ApiResult<()> {
        validate_pod_spec(&self.template.spec)?;
        if self.template.spec.restart_policy == RestartPolicy::Always {
            return Err(ApiError::bad_request(
                "job pods must use restart_policy OnFailure or Never",
            ));
        }
        if self.completions == 0 || self.parallelism == 0 {
            return Err(ApiError::bad_request(
                "completions and parallelism must be > 0",
            ));
        }
        if let Some(cur) = current {
            if self.template != cur.spec.template {
                return Err(ApiError::unprocessable("job template is immutable"));
            }
        }
        Ok(())
    }
}

impl Api for CronJobSpec {
    type Status = CronJobStatus;
    const NOUN: &'static str = "cronjobs";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, _current: Option<&CronJob>) -> ApiResult<()> {
        Schedule::parse(&self.schedule).map_err(|e| ApiError::bad_request(e.to_string()))?;
        self.job_template.validate(None)
    }
}

impl Api for IngressSpec {
    type Status = IngressStatus;
    const NOUN: &'static str = "ingresses";
    const DELETE: DeleteMode = DeleteMode::Direct;
    fn validate(&mut self, _current: Option<&Ingress>) -> ApiResult<()> {
        if self.rules.is_empty() && self.default_backend.is_none() {
            return Err(ApiError::bad_request(
                "spec.rules or spec.default_backend is required",
            ));
        }
        for r in &self.rules {
            for p in &r.paths {
                if !p.path.starts_with('/') {
                    return Err(ApiError::bad_request(format!(
                        "path '{}' must start with '/'",
                        p.path
                    )));
                }
                if p.backend.service.trim().is_empty() {
                    return Err(ApiError::bad_request("backend.service is required"));
                }
            }
        }
        Ok(())
    }
}

// ---- scope -----------------------------------------------------------------------

/// Who is asking and what they may see.
#[derive(Clone, Debug)]
pub struct Scope {
    pub project: Option<Id>,
    pub admin: bool,
    pub caller: Option<Caller>,
}

impl Scope {
    fn filter(&self) -> Option<&Id> {
        self.project.as_ref()
    }
    fn can_see(&self, project: Option<&Id>) -> bool {
        self.admin || (self.project.is_some() && project == self.project.as_ref())
    }
}

impl FromRequestParts<AppState> for Scope {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<Caller>() {
            Some(c) => Ok(Scope {
                project: c.project.clone(),
                admin: c.is_admin(),
                caller: Some(c.clone()),
            }),
            None if state.dev_admin => Ok(Scope {
                project: None,
                admin: true,
                caller: None,
            }),
            None => Err(ApiError::unauthorized("no credentials")),
        }
    }
}

/// `Json<T>` whose rejection is an `ApiError`.
pub struct Body<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Body<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(v) = Json::<T>::from_request(req, state)
            .await
            .map_err(|e| ApiError::bad_request(e.body_text()))?;
        Ok(Body(v))
    }
}

/// `Query<ListQuery>` whose rejection is an `ApiError`.
pub struct List(pub ListQuery);

impl<S: Send + Sync> FromRequestParts<S> for List {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(List(parse_query(parts.uri.query().unwrap_or(""))?))
    }
}

fn parse_query(q: &str) -> ApiResult<ListQuery> {
    let mut out = ListQuery::default();
    for pair in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = percent_decode(v);
        match k {
            "limit" => {
                out.limit = Some(
                    v.parse()
                        .map_err(|_| ApiError::bad_request("limit must be a number"))?,
                )
            }
            "after" => out.after = Some(v),
            "labels" => out.labels = Some(v),
            "name" => out.name = Some(v),
            _ => {}
        }
    }
    Ok(out)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len()
                && s.is_char_boundary(i + 1)
                && s.is_char_boundary(i + 3) =>
            {
                match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Sort by id, apply name/label filters, cut a page.
pub fn paginate<K: Kind>(mut items: Vec<K>, q: &ListQuery) -> Page<K> {
    let sel = q.label_selector();
    items.retain(|k| {
        let m = k.meta();
        q.name.as_deref().is_none_or(|n| m.name == n)
            && sel.iter().all(|(lk, lv)| m.labels.get(lk) == Some(lv))
    });
    items.sort_by(|a, b| a.meta().id.cmp(&b.meta().id));
    let total = items.len();
    if let Some(after) = q.after.as_deref() {
        items.retain(|k| k.meta().id.to_string().as_str() > after);
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let next = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|k| k.meta().id.to_string())
    } else {
        None
    };
    Page { items, next, total }
}

fn if_match(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    match headers.get(header::IF_MATCH) {
        None => Ok(None),
        Some(v) => {
            let s = v
                .to_str()
                .map_err(|_| ApiError::bad_request("If-Match is not ASCII"))?;
            s.trim()
                .trim_matches('"')
                .parse::<u64>()
                .map(Some)
                .map_err(|_| ApiError::bad_request("If-Match must be a revision number"))
        }
    }
}

pub fn validate_name(name: &str) -> ApiResult<()> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && name.starts_with(|c: char| c.is_ascii_alphanumeric());
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "name must be 1-63 chars of [a-z0-9.-] and start with a letter or digit",
        ))
    }
}

// ---- generic handlers ------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "S: DeserializeOwned"))]
pub struct Create<S> {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    pub spec: S,
}

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "S: DeserializeOwned"))]
pub struct Update<S> {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub spec: Option<S>,
}

type Res<A> = Resource<A, <A as Api>::Status>;

fn col<A: Api>(s: &AppState) -> Collection<Res<A>> {
    s.store.collection::<Res<A>>()
}

fn load<A: Api>(s: &AppState, scope: &Scope, id: &str) -> ApiResult<Res<A>> {
    let r = col::<A>(s).resolve(scope.filter(), id)?;
    if !scope.can_see(r.meta.project.as_ref()) {
        return Err(ApiError::not_found(format!("{} '{id}' not found", A::KIND)));
    }
    Ok(r)
}

async fn index() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "module": "loom",
        "resources": [
            PodSpec::NOUN, ReplicaSetSpec::NOUN, DeploymentSpec::NOUN, ServiceSpec::NOUN,
            ConfigMapSpec::NOUN, JobSpec::NOUN, CronJobSpec::NOUN, IngressSpec::NOUN,
        ],
    }))
}

async fn list<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    List(q): List,
) -> ApiResult<Json<Page<Res<A>>>> {
    let items = col::<A>(&s).list(scope.filter())?;
    Ok(Json(paginate(items, &q)))
}

async fn create<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    Body(body): Body<Create<A>>,
) -> ApiResult<Response> {
    validate_name(&body.name)?;
    let mut spec = body.spec;
    spec.validate(None)?;
    let mut r: Res<A> = Resource::new(body.name, scope.project.clone(), spec);
    r.meta.description = body.description;
    r.meta.labels = body.labels;
    r.meta.annotations = body.annotations;
    r.meta.annotations.remove(DELETE_ANNOTATION);
    let r = col::<A>(&s).create(r)?;
    Ok((StatusCode::CREATED, Json(r)).into_response())
}

async fn get_one<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Res<A>>> {
    Ok(Json(load::<A>(&s, &scope, &id)?))
}

async fn update<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(body): Body<Update<A>>,
) -> ApiResult<Json<Res<A>>> {
    let expected = if_match(&headers)?;
    let mut r = load::<A>(&s, &scope, &id)?;
    if delete_requested(&r) {
        return Err(ApiError::conflict("resource is being deleted"));
    }
    if let Some(d) = body.description {
        r.meta.description = d;
    }
    if let Some(l) = body.labels {
        r.meta.labels = l;
    }
    if let Some(a) = body.annotations {
        let keep = r.meta.annotations.get(DELETE_ANNOTATION).cloned();
        r.meta.annotations = a;
        r.meta.annotations.remove(DELETE_ANNOTATION);
        if let Some(v) = keep {
            r.meta.annotations.insert(DELETE_ANNOTATION.into(), v);
        }
    }
    if let Some(mut spec) = body.spec {
        spec.validate(Some(&r))?;
        r.spec = spec;
    }
    Ok(Json(col::<A>(&s).update(r, expected)?))
}

async fn delete<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let r = load::<A>(&s, &scope, &id)?;
    match A::DELETE {
        DeleteMode::Direct => {
            col::<A>(&s).delete(&r.meta.id)?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        DeleteMode::Annotate => {
            if delete_requested(&r) {
                return Ok((StatusCode::ACCEPTED, Json(r)).into_response());
            }
            let r = col::<A>(&s).modify(&r.meta.id, |x| {
                x.meta
                    .annotations
                    .insert(DELETE_ANNOTATION.into(), Utc::now().to_rfc3339());
                Ok(())
            })?;
            Ok((StatusCode::ACCEPTED, Json(r)).into_response())
        }
    }
}

// ---- verbs -----------------------------------------------------------------------

async fn pod_logs(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let pod = load::<PodSpec>(&s, &scope, &id)?;
    let machine = pod
        .status
        .machine
        .clone()
        .ok_or_else(|| ApiError::unavailable("pod logs are not available"))?;
    let host_id = pod
        .status
        .host
        .clone()
        .ok_or_else(|| ApiError::unavailable("pod logs are not available"))?;
    let host = s
        .store
        .collection::<forge::Host>()
        .get(&host_id)
        .map_err(|_| ApiError::unavailable("pod logs are not available"))?;
    if host.spec.node_name.is_empty() {
        return Err(ApiError::unavailable("pod logs are not available"));
    }
    let queue = s
        .queue
        .clone()
        .ok_or_else(|| ApiError::unavailable("pod logs are not configured"))?;
    let ticket = queue.enqueue(
        &host.spec.node_name,
        CommandKind::ReadLogs,
        serde_json::json!({"id": machine.to_string()}),
    )?;
    let value = ticket.wait().await?.into_api_result()?;
    let text = match value.get("text").and_then(|v| v.as_str()) {
        Some(text) => text.to_owned(),
        None => return Err(ApiError::unavailable("pod log text is missing")),
    };
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response())
}

/// The document the pod's machine carries to anvil; handy for debugging.
async fn pod_sandbox(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::controllers::pod::PodSandbox>> {
    let p = load::<PodSpec>(&s, &scope, &id)?;
    Ok(Json(crate::controllers::pod::PodSandbox::from_pod(&p)))
}

#[derive(Debug, Deserialize)]
pub struct ScaleBody {
    pub replicas: u32,
}

async fn scale_deployment(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Body(body): Body<ScaleBody>,
) -> ApiResult<Json<Deployment>> {
    let d = load::<DeploymentSpec>(&s, &scope, &id)?;
    if delete_requested(&d) {
        return Err(ApiError::conflict("deployment is being deleted"));
    }
    let d = col::<DeploymentSpec>(&s).modify(&d.meta.id, |x| {
        x.spec.replicas = body.replicas;
        Ok(())
    })?;
    Ok(Json(d))
}

#[derive(Debug, Deserialize, Default)]
pub struct RollbackBody {
    /// Revision to roll back to; the previous one when omitted.
    #[serde(default)]
    pub revision: Option<u64>,
}

async fn rollback_deployment(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    body: Result<Body<RollbackBody>, ApiError>,
) -> ApiResult<Json<Deployment>> {
    // An empty body means "the previous revision".
    let body = body.map(|Body(b)| b).unwrap_or_default();
    let d = load::<DeploymentSpec>(&s, &scope, &id)?;
    if delete_requested(&d) {
        return Err(ApiError::conflict("deployment is being deleted"));
    }
    let sets = owned_sets(
        &s.store
            .collection::<ReplicaSet>()
            .list(d.meta.project.as_ref())?,
        &d,
    );
    let current_hash = template_hash(&d.spec.template);
    let target = match body.revision {
        Some(rev) => sets
            .iter()
            .find(|rs| revision_of(rs) == rev)
            .ok_or_else(|| ApiError::not_found(format!("revision {rev} not found")))?,
        None => sets
            .iter()
            .rev()
            .find(|rs| rs.meta.labels.get(POD_TEMPLATE_HASH) != Some(&current_hash))
            .ok_or_else(|| ApiError::unprocessable("no previous revision to roll back to"))?,
    };
    if target.meta.labels.get(POD_TEMPLATE_HASH) == Some(&current_hash) {
        return Err(ApiError::unprocessable(
            "that revision is already the current template",
        ));
    }
    let mut template = target.spec.template.clone();
    template.labels.remove(POD_TEMPLATE_HASH);
    let d = col::<DeploymentSpec>(&s).modify(&d.meta.id, |x| {
        x.spec.template = template.clone();
        Ok(())
    })?;
    Ok(Json(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_node_port_validation() {
        let spec = |kind: ServiceKind, ports: &[(u16, Option<u16>)]| ServiceSpec {
            ports: ports
                .iter()
                .map(|(port, node_port)| ServicePort {
                    port: *port,
                    node_port: *node_port,
                    ..ServicePort::default()
                })
                .collect(),
            kind,
            ..ServiceSpec::default()
        };
        assert!(spec(ServiceKind::NodePort, &[(80, Some(30080))])
            .validate(None)
            .is_ok());
        assert!(spec(ServiceKind::LoadBalancer, &[(80, None)])
            .validate(None)
            .is_ok());
        assert!(spec(ServiceKind::ClusterIp, &[(80, Some(30080))])
            .validate(None)
            .is_err());
        assert!(spec(ServiceKind::NodePort, &[(80, Some(8080))])
            .validate(None)
            .is_err());
        assert!(spec(
            ServiceKind::NodePort,
            &[(80, Some(30080)), (81, Some(30080))]
        )
        .validate(None)
        .is_err());
    }

    #[test]
    fn query_parses() {
        let q = parse_query("limit=5&labels=app%3Dweb&name=a+b").unwrap();
        assert_eq!(q.limit, Some(5));
        assert_eq!(q.labels.as_deref(), Some("app=web"));
        assert_eq!(q.name.as_deref(), Some("a b"));
        assert!(parse_query("limit=x").is_err());
    }

    #[test]
    fn selector_defaults_and_must_match() {
        let mut t = PodTemplate::default();
        t.labels.insert("app".into(), "web".into());
        let mut sel = Selector::default();
        validate_selector(&mut sel, &t).unwrap();
        assert_eq!(sel.match_labels.get("app").unwrap(), "web");
        let mut bad = Selector::from_labels(&[("app".to_string(), "db".to_string())].into());
        assert!(validate_selector(&mut bad, &t).is_err());
        assert!(validate_selector(&mut Selector::default(), &PodTemplate::default()).is_err());
    }

    #[test]
    fn pod_spec_validation() {
        assert!(validate_pod_spec(&PodSpec::default()).is_err());
        let mut spec = PodSpec {
            containers: vec![Container {
                name: "web".into(),
                image: "nginx".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(validate_pod_spec(&spec).is_ok());
        spec.containers[0].volume_mounts.push(VolumeMount {
            name: "data".into(),
            mount_path: "/data".into(),
            ..Default::default()
        });
        assert!(validate_pod_spec(&spec).is_err());
        spec.volumes.push(PodVolume {
            name: "data".into(),
            source: VolumeSource::EmptyDir,
        });
        assert!(validate_pod_spec(&spec).is_ok());
    }
}
