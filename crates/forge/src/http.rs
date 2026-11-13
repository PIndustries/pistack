// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `compute` API: `/machines`, `/hosts`, `/sizes`.
//!
//! Handlers write `spec` and `meta` only. Anything that must happen on a
//! host (start, stop, reboot, delete) is recorded as desired state and the
//! reconciler carries it out, so every mutating call returns quickly with
//! the resource as stored and the client polls `status.phase`.
//! Live migration is the exception: it enqueues on the source node and only
//! then records the destination on `status`, so port binding can follow.
//!
//! Project scoping reads `pistack_types::Caller` from request extensions
//! when the gateway's auth layer attached one. With no `Caller` the request
//! is treated as cloud admin in dev mode and rejected otherwise.

use crate::driver::DriverRegistry;
use crate::types::{
    new_machine, new_size, Host, Machine, MachineSpec, Size, DELETE_ANNOTATION,
    SYSTEM_LABEL_PREFIX, ZONE_LABEL,
};
use axum::{
    extract::{FromRequest, FromRequestParts, Path, Request, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::Utc;
use pistack_rpc::CommandQueue;
use pistack_store::{Collection, Store};
use pistack_types::{ApiError, ApiResult, Caller, Id, Kind, ListQuery, Page, Phase};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub drivers: Arc<DriverRegistry>,
    /// Treat unauthenticated requests as cloud admin.
    pub dev_admin: bool,
    /// Agent transport. Absent in dev mode; live migration is refused then.
    pub queue: Option<CommandQueue>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/machines", get(list_machines).post(create_machine))
        .route(
            "/machines/{id}",
            get(get_machine).put(update_machine).delete(delete_machine),
        )
        .route("/machines/{id}/start", post(start_machine))
        .route("/machines/{id}/stop", post(stop_machine))
        .route("/machines/{id}/reboot", post(reboot_machine))
        .route("/machines/{id}/migrate", post(migrate_machine))
        .route("/machines/{id}/console", get(console))
        .route("/machines/{id}/console/vnc", get(console_vnc))
        .route("/hosts", get(list_hosts))
        .route("/hosts/{id}", get(get_host))
        .route("/hosts/{id}/labels", put(set_host_labels))
        .route("/hosts/{id}/disable", post(disable_host))
        .route("/hosts/{id}/enable", post(enable_host))
        .route("/sizes", get(list_sizes).post(create_size))
        .route(
            "/sizes/{id}",
            get(get_size).put(update_size).delete(delete_size),
        )
        .with_state(state)
}

// ---- scope ------------------------------------------------------------------

/// Who is asking and what they may see.
#[derive(Clone, Debug)]
pub struct Scope {
    pub project: Option<Id>,
    pub admin: bool,
    pub caller: Option<Caller>,
}

impl Scope {
    /// Project filter for list calls: admins without a project see all.
    fn filter(&self) -> Option<&Id> {
        self.project.as_ref()
    }
    fn can_see(&self, project: Option<&Id>) -> bool {
        self.admin || (self.project.is_some() && project == self.project.as_ref())
    }
    fn require_admin(&self) -> ApiResult<()> {
        if self.admin {
            Ok(())
        } else {
            Err(ApiError::forbidden("admin role required"))
        }
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
        let q = parts.uri.query().unwrap_or("");
        let v: ListQuery = serde_urlencoded_lite(q)?;
        Ok(List(v))
    }
}

/// Parse `limit`, `after`, `labels`, `name` from a query string.
fn serde_urlencoded_lite(q: &str) -> ApiResult<ListQuery> {
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
                let hex = &s[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
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
fn paginate<K: Kind>(mut items: Vec<K>, q: &ListQuery) -> Page<K> {
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
            let s = s.trim().trim_matches('"');
            s.parse::<u64>()
                .map(Some)
                .map_err(|_| ApiError::bad_request("If-Match must be a revision number"))
        }
    }
}

fn validate_name(name: &str) -> ApiResult<()> {
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

fn created<T: Serialize>(v: T) -> Response {
    (StatusCode::CREATED, Json(v)).into_response()
}

// ---- index -------------------------------------------------------------------

async fn index() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "module": "forge",
        "resources": ["machines", "hosts", "sizes"],
    }))
}

// ---- machines ----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateMachine {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub spec: MachineSpec,
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateMachine {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub spec: Option<MachineSpec>,
}

fn machines(s: &AppState) -> Collection<Machine> {
    s.store.collection::<Machine>()
}
fn sizes(s: &AppState) -> Collection<Size> {
    s.store.collection::<Size>()
}
fn hosts(s: &AppState) -> Collection<Host> {
    s.store.collection::<Host>()
}

fn load_machine(s: &AppState, scope: &Scope, id: &str) -> ApiResult<Machine> {
    let m = machines(s).resolve(scope.filter(), id)?;
    if !scope.can_see(m.meta.project.as_ref()) {
        return Err(ApiError::not_found(format!("mach '{id}' not found")));
    }
    Ok(m)
}

fn resolve_size(s: &AppState, name_or_id: &str) -> ApiResult<Size> {
    sizes(s)
        .resolve(None, name_or_id)
        .map_err(|_| ApiError::unprocessable(format!("size '{name_or_id}' not found")))
}

/// No host reads `spec.volumes`: a machine that named volumes started
/// without them. Until a start can attach them, say so instead of taking
/// the request. Attaching to a running machine goes through the volume API.
fn refuse_volumes(volumes: &[String]) -> ApiResult<()> {
    if volumes.is_empty() {
        return Ok(());
    }
    Err(ApiError::unprocessable(
        "spec.volumes is not supported: a machine does not attach volumes when it starts. \
         Create the machine without them, then attach each one with \
         `pi volume attach <volume> --machine <machine>`",
    ))
}

/// A zone and a `zone` host selector say the same thing. Two different
/// values can match no host, so refuse them here.
fn check_zone(spec: &MachineSpec) -> ApiResult<()> {
    let Some(zone) = &spec.zone else {
        return Ok(());
    };
    if zone.trim().is_empty() {
        return Err(ApiError::bad_request("spec.zone must not be empty"));
    }
    match spec.host_selector.get(ZONE_LABEL) {
        Some(sel) if sel != zone => Err(ApiError::unprocessable(format!(
            "spec.zone is '{zone}' but spec.host_selector asks for zone '{sel}'"
        ))),
        _ => Ok(()),
    }
}

async fn list_machines(
    State(s): State<AppState>,
    scope: Scope,
    List(q): List,
) -> ApiResult<Json<Page<Machine>>> {
    let items = machines(&s).list(scope.filter())?;
    Ok(Json(paginate(items, &q)))
}

async fn create_machine(
    State(s): State<AppState>,
    scope: Scope,
    Body(body): Body<CreateMachine>,
) -> ApiResult<Response> {
    validate_name(&body.name)?;
    if body.spec.image.trim().is_empty() {
        return Err(ApiError::bad_request("spec.image is required"));
    }
    if body.spec.size.trim().is_empty() {
        return Err(ApiError::bad_request("spec.size is required"));
    }
    refuse_volumes(&body.spec.volumes)?;
    check_zone(&body.spec)?;
    let size = resolve_size(&s, &body.spec.size)?;
    let mut spec = body.spec;
    // Store the canonical size name so renames of the request form do not
    // change what the scheduler looks up.
    spec.size = size.meta.name.clone();
    spec.restarts = 0;
    crate::usage::admit_create(&s.store, scope.project.as_ref())?;
    let mut m = new_machine(body.name, scope.project.clone(), spec);
    m.meta.description = body.description;
    m.meta.labels = body.labels;
    let m = machines(&s).create(m)?;
    crate::usage::note_create(&s.store, m.meta.project.as_ref());
    Ok(created(m))
}

async fn get_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Machine>> {
    Ok(Json(load_machine(&s, &scope, &id)?))
}

async fn update_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(body): Body<UpdateMachine>,
) -> ApiResult<Json<Machine>> {
    let expected = if_match(&headers)?;
    let mut m = load_machine(&s, &scope, &id)?;
    if let Some(d) = body.description {
        m.meta.description = d;
    }
    if let Some(l) = body.labels {
        m.meta.labels = l;
    }
    if let Some(mut spec) = body.spec {
        if m.status.phase != Phase::Pending
            && (spec.kind != m.spec.kind || spec.image != m.spec.image || spec.size != m.spec.size)
        {
            return Err(ApiError::unprocessable(
                "kind, image and size can only change while the machine is pending",
            ));
        }
        // A machine stored with volumes before they were refused keeps them.
        if spec.volumes != m.spec.volumes {
            refuse_volumes(&spec.volumes)?;
        }
        check_zone(&spec)?;
        let size = resolve_size(&s, &spec.size)?;
        spec.size = size.meta.name.clone();
        // Reboot requests are made through the action endpoint, never by PUT.
        spec.restarts = m.spec.restarts;
        m.spec = spec;
    }
    Ok(Json(machines(&s).update(m, expected)?))
}

async fn delete_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let m = load_machine(&s, &scope, &id)?;
    if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
        return Ok((StatusCode::ACCEPTED, Json(m)).into_response());
    }
    let m = machines(&s).modify(&m.meta.id, |x| {
        x.meta
            .annotations
            .insert(DELETE_ANNOTATION.into(), Utc::now().to_rfc3339());
        Ok(())
    })?;
    crate::usage::note_delete(&s.store, m.meta.project.as_ref());
    Ok((StatusCode::ACCEPTED, Json(m)).into_response())
}

async fn power_action(
    s: &AppState,
    scope: &Scope,
    id: &str,
    f: impl Fn(&mut MachineSpec),
) -> ApiResult<Json<Machine>> {
    let m = load_machine(s, scope, id)?;
    if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
        return Err(ApiError::conflict("machine is being deleted"));
    }
    let m = machines(s).modify(&m.meta.id, |x| {
        f(&mut x.spec);
        Ok(())
    })?;
    Ok(Json(m))
}

async fn start_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Machine>> {
    power_action(&s, &scope, &id, |sp| {
        sp.power = crate::types::DesiredPower::On
    })
    .await
}

async fn stop_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Machine>> {
    power_action(&s, &scope, &id, |sp| {
        sp.power = crate::types::DesiredPower::Off
    })
    .await
}

async fn reboot_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Machine>> {
    power_action(&s, &scope, &id, |sp| {
        sp.power = crate::types::DesiredPower::On;
        sp.restarts += 1;
    })
    .await
}

#[derive(Debug, Deserialize)]
struct MigrateBody {
    host: String,
}

async fn migrate_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Body(body): Body<MigrateBody>,
) -> ApiResult<Json<Machine>> {
    let m = load_machine(&s, &scope, &id)?;
    if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
        return Err(ApiError::conflict("machine is being deleted"));
    }
    let dest = hosts(&s).resolve(None, &body.host)?;
    let queue = s
        .queue
        .clone()
        .ok_or_else(|| ApiError::unavailable("live migration needs the agent transport"))?;
    // Host placement is updated in `begin_migration` after the command is
    // queued, not here.
    let m = crate::agent_driver::begin_migration(&s.store, &queue, &m, &dest)?;
    Ok(Json(m))
}

#[derive(Serialize)]
struct ConsoleTicket {
    #[serde(rename = "type")]
    kind: &'static str,
    websocket: String,
}

async fn console(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<ConsoleTicket>> {
    let m = load_machine(&s, &scope, &id)?;
    let ticket = crate::console::mint_ticket(&m.meta.id.to_string());
    Ok(Json(ConsoleTicket {
        kind: "vnc",
        websocket: format!(
            "/compute/machines/{}/console/vnc?ticket={ticket}",
            m.meta.id
        ),
    }))
}

/// The browser cannot send the bearer token on a WebSocket. The ticket from
/// [`console`] is the only credential, and it is checked before the upgrade.
async fn console_vnc(
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let ticket = crate::console::query_ticket(raw.as_deref())?;
    crate::console::check_ticket(&ticket, &id)?;
    let port = crate::console::display_port(&id).await?;
    Ok(ws.on_upgrade(move |socket| async move {
        if crate::console::take_ticket(&ticket, &id).is_err() {
            return;
        }
        crate::console::splice(socket, port).await;
    }))
}

// ---- hosts -------------------------------------------------------------------

async fn list_hosts(
    State(s): State<AppState>,
    scope: Scope,
    List(q): List,
) -> ApiResult<Json<Page<Host>>> {
    scope.require_admin()?;
    Ok(Json(paginate(hosts(&s).list(None)?, &q)))
}

async fn get_host(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Host>> {
    scope.require_admin()?;
    Ok(Json(hosts(&s).resolve(None, &id)?))
}

#[derive(Debug, Deserialize)]
pub struct SetHostLabels {
    pub labels: BTreeMap<String, String>,
}

fn validate_label(key: &str, value: &str) -> ApiResult<()> {
    if key.starts_with(SYSTEM_LABEL_PREFIX) {
        return Err(ApiError::unprocessable(format!(
            "label '{key}' is set by PiStack and cannot be written"
        )));
    }
    let word = |s: &str| {
        !s.is_empty()
            && s.len() <= 63
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
            && s.starts_with(|c: char| c.is_ascii_alphanumeric())
    };
    if !word(key) || !word(value) {
        return Err(ApiError::bad_request(format!(
            "label '{key}={value}': key and value must be 1-63 chars of [A-Za-z0-9._/-] and start with a letter or digit"
        )));
    }
    Ok(())
}

/// Replace the labels an operator owns. Labels PiStack wrote itself, the
/// ones under `pistack.io/`, stay as they are.
async fn set_host_labels(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Body(body): Body<SetHostLabels>,
) -> ApiResult<Json<Host>> {
    scope.require_admin()?;
    for (k, v) in &body.labels {
        validate_label(k, v)?;
    }
    let host = hosts(&s).resolve(None, &id)?;
    let host = hosts(&s).modify(&host.meta.id, |h| {
        h.meta
            .labels
            .retain(|k, _| k.starts_with(SYSTEM_LABEL_PREFIX));
        h.meta.labels.extend(body.labels.clone());
        Ok(())
    })?;
    Ok(Json(host))
}

#[derive(Debug, Deserialize, Default)]
pub struct DisableHost {
    #[serde(default)]
    pub reason: Option<String>,
}

/// The JSON body of an action that can also be posted with no body.
fn optional_body<T: DeserializeOwned + Default>(bytes: &[u8]) -> ApiResult<T> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes).map_err(|e| ApiError::bad_request(e.to_string()))
}

fn clean_reason(reason: Option<String>) -> ApiResult<Option<String>> {
    let reason = reason
        .map(|r| r.trim().to_owned())
        .filter(|r| !r.is_empty());
    if reason.as_ref().is_some_and(|r| r.chars().count() > 256) {
        return Err(ApiError::bad_request(
            "reason must be at most 256 characters",
        ));
    }
    Ok(reason)
}

/// Stop placing machines on a host. Calling it again changes the reason
/// when one is given and is otherwise a no-op.
fn set_disabled(s: &AppState, id: &Id, reason: Option<String>) -> ApiResult<Host> {
    Ok(hosts(s).modify(id, |h| {
        h.spec.disabled = true;
        if let Some(r) = &reason {
            h.spec.disabled_reason = r.clone();
        }
        Ok(())
    })?)
}

async fn disable_host(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Host>> {
    scope.require_admin()?;
    let body: DisableHost = optional_body(&body)?;
    let reason = clean_reason(body.reason)?;
    let host = hosts(&s).resolve(None, &id)?;
    Ok(Json(set_disabled(&s, &host.meta.id, reason)?))
}

async fn enable_host(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Host>> {
    scope.require_admin()?;
    let host = hosts(&s).resolve(None, &id)?;
    let host = hosts(&s).modify(&host.meta.id, |h| {
        h.spec.disabled = false;
        h.spec.disabled_reason.clear();
        Ok(())
    })?;
    Ok(Json(host))
}

// ---- sizes -------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateSize {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub resources: pistack_types::Resources,
}

async fn list_sizes(
    State(s): State<AppState>,
    _scope: Scope,
    List(q): List,
) -> ApiResult<Json<Page<Size>>> {
    Ok(Json(paginate(sizes(&s).list(None)?, &q)))
}

async fn get_size(
    State(s): State<AppState>,
    _scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Size>> {
    Ok(Json(sizes(&s).resolve(None, &id)?))
}

fn validate_resources(r: &pistack_types::Resources) -> ApiResult<()> {
    if r.vcpus == 0 || r.memory_mib == 0 {
        return Err(ApiError::bad_request("vcpus and memory_mib must be > 0"));
    }
    Ok(())
}

async fn create_size(
    State(s): State<AppState>,
    scope: Scope,
    Body(body): Body<CreateSize>,
) -> ApiResult<Response> {
    scope.require_admin()?;
    validate_name(&body.name)?;
    validate_resources(&body.resources)?;
    let mut sz = new_size(body.name, body.resources);
    sz.meta.description = body.description;
    Ok(created(sizes(&s).create(sz)?))
}

async fn update_size(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(body): Body<CreateSize>,
) -> ApiResult<Json<Size>> {
    scope.require_admin()?;
    let expected = if_match(&headers)?;
    validate_name(&body.name)?;
    validate_resources(&body.resources)?;
    let mut sz = sizes(&s).resolve(None, &id)?;
    sz.meta.name = body.name;
    sz.meta.description = body.description;
    sz.spec.resources = body.resources;
    Ok(Json(sizes(&s).update(sz, expected)?))
}

async fn delete_size(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    scope.require_admin()?;
    let sz = sizes(&s).resolve(None, &id)?;
    let in_use = machines(&s)
        .list(None)?
        .iter()
        .any(|m| m.spec.size == sz.meta.name);
    if in_use {
        return Err(ApiError::conflict(format!(
            "size '{}' is used by existing machines",
            sz.meta.name
        )));
    }
    sizes(&s).delete(&sz.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parses() {
        let q = serde_urlencoded_lite("limit=5&labels=env%3Dprod%2Ctier%3Dweb&name=a+b").unwrap();
        assert_eq!(q.limit, Some(5));
        assert_eq!(q.labels.as_deref(), Some("env=prod,tier=web"));
        assert_eq!(q.name.as_deref(), Some("a b"));
        assert!(serde_urlencoded_lite("limit=x").is_err());
    }

    #[test]
    fn pagination_cuts_and_links() {
        let items: Vec<Size> = (0..5)
            .map(|i| {
                new_size(
                    format!("s{i}"),
                    pistack_types::Resources {
                        vcpus: 1,
                        memory_mib: 1,
                        disk_gib: 0,
                        gpus: 0,
                    },
                )
            })
            .collect();
        let q = ListQuery {
            limit: Some(2),
            ..Default::default()
        };
        let p1 = paginate(items.clone(), &q);
        assert_eq!(p1.items.len(), 2);
        assert_eq!(p1.total, 5);
        let next = p1.next.clone().unwrap();
        let p2 = paginate(
            items.clone(),
            &ListQuery {
                limit: Some(2),
                after: Some(next),
                ..Default::default()
            },
        );
        assert_eq!(p2.items.len(), 2);
        assert!(p1
            .items
            .iter()
            .all(|a| p2.items.iter().all(|b| a.meta.id != b.meta.id)));
        let p3 = paginate(
            items,
            &ListQuery {
                limit: Some(2),
                after: p2.next,
                ..Default::default()
            },
        );
        assert_eq!(p3.items.len(), 1);
        assert!(p3.next.is_none());
    }

    #[test]
    fn names_validated() {
        assert!(validate_name("web-1").is_ok());
        assert!(validate_name("Web").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("-x").is_err());
    }

    #[tokio::test]
    async fn migrate_route_refuses_without_underlay_and_enqueues() {
        use crate::types::new_host;
        use axum::body::Body as HttpBody;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use pistack_rpc::proto::{CommandKind, MigrateRequest};
        use pistack_types::Resources;
        use tower::ServiceExt;

        let store = Store::in_memory().unwrap();
        let queue = CommandQueue::default();
        let cap = Resources {
            vcpus: 2,
            memory_mib: 2048,
            disk_gib: 20,
            gpus: 0,
        };
        let labels = BTreeMap::new();
        let src = store
            .collection::<Host>()
            .create(new_host(
                "src",
                "node-src",
                cap,
                vec!["kvm".into()],
                labels.clone(),
            ))
            .unwrap();
        let mut bare = new_host("bare", "node-bare", cap, vec!["kvm".into()], labels.clone());
        bare.spec.underlay_ip = None;
        store.collection::<Host>().create(bare).unwrap();
        let mut dest = new_host("dest", "node-dest", cap, vec!["kvm".into()], labels);
        dest.spec.underlay_ip = Some("10.1.0.2".into());
        let dest = store.collection::<Host>().create(dest).unwrap();
        let mut m = new_machine(
            "web",
            None,
            MachineSpec {
                size: "s.1x1".into(),
                image: "ubuntu".into(),
                networks: vec!["tenant".into()],
                ..Default::default()
            },
        );
        m.status.host = Some(src.meta.id.clone());
        m.status.phase = Phase::Ready;
        let m = store.collection::<Machine>().create(m).unwrap();
        let id = m.meta.id.to_string();
        let app = router(AppState {
            store: store.clone(),
            drivers: Arc::new(DriverRegistry::new()),
            dev_admin: true,
            queue: Some(queue.clone()),
        });

        async fn post(app: &axum::Router, path: &str, host: &str) -> (StatusCode, Vec<u8>) {
            let req = Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(HttpBody::from(
                    serde_json::to_vec(&serde_json::json!({ "host": host })).unwrap(),
                ))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = resp
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec();
            (status, bytes)
        }

        let (st, bytes) = post(&app, &format!("/machines/{id}/migrate"), "bare").await;
        assert_ne!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        let still = store.collection::<Machine>().get(&m.meta.id).unwrap();
        assert_eq!(still.status.host.as_ref(), Some(&src.meta.id));
        assert_eq!(queue.pending_count(), 0);

        let (st, bytes) = post(&app, &format!("/machines/{id}/migrate"), "dest").await;
        assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        let body: Machine = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.status.phase, Phase::Updating);
        assert_eq!(body.status.message, format!("migrating {}", dest.meta.id));
        assert_eq!(body.status.host.as_ref(), Some(&src.meta.id));
        let cmds = queue
            .next("node-src", std::time::Duration::from_secs(1), 4)
            .await;
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].kind, CommandKind::MigrateMachine);
        let req: MigrateRequest = serde_json::from_value(cmds[0].payload.clone()).unwrap();
        assert_eq!(req.dest, "qemu+ssh://10.1.0.2/system");
    }
}
