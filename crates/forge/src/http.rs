// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `compute` API: `/machines`, `/hosts`, `/sizes`.
//!
//! Handlers write `spec` and `meta` only. Anything that must happen on a
//! host (start, stop, reboot, delete) is recorded as desired state and the
//! reconciler carries it out, so every mutating call returns quickly with
//! the resource as stored and the client polls `status.phase`.
//!
//! Project scoping reads `pistack_types::Caller` from request extensions
//! when the gateway's auth layer attached one. With no `Caller` the request
//! is treated as cloud admin in dev mode and rejected otherwise.

use crate::driver::DriverRegistry;
use crate::types::{new_machine, new_size, Host, Machine, MachineSpec, Size, DELETE_ANNOTATION};
use axum::{
    extract::{FromRequest, FromRequestParts, Path, Request, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
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
        .route("/machines/{id}/console", get(console))
        .route("/hosts", get(list_hosts))
        .route("/hosts/{id}", get(get_host))
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
    let size = resolve_size(&s, &body.spec.size)?;
    let mut spec = body.spec;
    // Store the canonical size name so renames of the request form do not
    // change what the scheduler looks up.
    spec.size = size.meta.name.clone();
    spec.restarts = 0;
    let mut m = new_machine(body.name, scope.project.clone(), spec);
    m.meta.description = body.description;
    m.meta.labels = body.labels;
    let m = machines(&s).create(m)?;
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

async fn console(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let m = load_machine(&s, &scope, &id)?;
    Err(ApiError::not_implemented(format!(
        "serial/VNC console for {} is not available yet",
        m.meta.name
    )))
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
}
