// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP handlers, mounted by the gateway at `/identity`.
//!
//! ```text
//! POST   /tokens                 login with password or API key -> token + caller
//! GET    /tokens/self            the caller behind the bearer token
//! GET    /projects               (admin)            POST /projects (admin)
//! GET    /projects/{id}          (member/reader of it, or admin)
//! PUT    /projects/{id}          (admin)            DELETE /projects/{id} (admin)
//! GET    /projects/{id}/quotas   limit and usage per resource (member/reader of it, or admin)
//! PUT    /projects/{id}/quotas   set or clear single limits (admin)
//! GET|POST /principals, GET|PUT|DELETE /principals/{id}   (admin; GET self allowed)
//! GET    /signing-keys           ids and dates of the token signing keys (admin)
//! POST   /signing-keys/rotate    sign with a new key; the old one verifies for a while (admin)
//! POST   /signing-keys/retire    drop every key but the current one now (admin)
//! GET|POST /roles, GET|PUT|DELETE /roles/{id}             (read: any caller; write: admin)
//! GET|POST /bindings, GET|DELETE /bindings/{id}           (admin)
//! GET|POST /keys, GET|DELETE /keys/{id}                   (own keys, or any as admin)
//! GET    /policy                 the RBAC document
//! GET    /audit                  the audit log, filtered and paged (admin)
//! ```
//!
//! `{id}` accepts a name or an id. `PUT` honours `If-Match: <revision>`.

use axum::{
    extract::{FromRequest, FromRequestParts, Path, Query, Request, State},
    http::{request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use pistack_store::{Store, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Caller, Id, ListQuery, Meta, Page};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    binding_name, credentials, paginate, policy,
    token::{AuthCaller, DEFAULT_TTL, MAX_TTL},
    types::*,
    AppState, TryGetByName,
};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/tokens", post(login))
        .route("/tokens/self", get(whoami))
        .route("/projects", get(list_projects).post(create_project))
        .route(
            "/projects/{id}",
            get(get_project).put(update_project).delete(delete_project),
        )
        .route("/projects/{id}/quotas", get(get_quotas).put(set_quotas))
        .route("/principals", get(list_principals).post(create_principal))
        .route(
            "/principals/{id}",
            get(get_principal)
                .put(update_principal)
                .delete(delete_principal),
        )
        .route("/signing-keys", get(list_signing_keys))
        .route("/signing-keys/rotate", post(rotate_signing_key))
        .route("/signing-keys/retire", post(retire_signing_keys))
        .route("/roles", get(list_roles).post(create_role))
        .route(
            "/roles/{id}",
            get(get_role).put(update_role).delete(delete_role),
        )
        .route("/bindings", get(list_bindings).post(create_binding))
        .route("/bindings/{id}", get(get_binding).delete(delete_binding))
        .route("/keys", get(list_keys).post(create_key))
        .route("/keys/{id}", get(get_key).delete(delete_key))
        .route("/policy", get(get_policy))
        .route("/audit", get(list_audit))
        .fallback(|| async { ApiError::not_found("no such identity endpoint") })
        .with_state(state)
}

// ---- extractors that fail as ApiError -------------------------------------

/// `Json<T>` whose rejection is an `ApiError`, so every error has one shape.
pub struct Body<T>(pub T);

impl<S, T> FromRequest<S> for Body<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = ApiError;
    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(v)| Body(v))
            .map_err(|e| ApiError::bad_request(format!("invalid body: {}", e.body_text())))
    }
}

/// `Query<ListQuery>` with an `ApiError` rejection.
pub struct ListParams(pub ListQuery);

impl<S: Send + Sync> FromRequestParts<S> for ListParams {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<ListQuery>::from_request_parts(parts, state)
            .await
            .map(|Query(q)| ListParams(q))
            .map_err(|e| ApiError::bad_request(format!("invalid query: {}", e.body_text())))
    }
}

fn if_match(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    match headers.get("if-match") {
        None => Ok(None),
        Some(v) => v
            .to_str()
            .ok()
            .map(|s| s.trim().trim_matches('"'))
            .and_then(|s| s.parse::<u64>().ok())
            .map(Some)
            .ok_or_else(|| ApiError::bad_request("If-Match must be a revision number")),
    }
}

fn created<T: Serialize>(v: T) -> Response {
    (StatusCode::CREATED, Json(v)).into_response()
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

fn validate_name(name: &str) -> ApiResult<()> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !Id::parse_any(name);
    if !ok {
        return Err(ApiError::bad_request(
            "name must be 1-63 chars of [a-zA-Z0-9._-] and must not look like an id",
        ));
    }
    Ok(())
}

trait IdExt {
    fn parse_any(s: &str) -> bool;
}
impl IdExt for Id {
    /// Does `s` look like `<kind>-<uuid>`? Names must not, or `resolve` is ambiguous.
    fn parse_any(s: &str) -> bool {
        s.split_once('-')
            .is_some_and(|(_, rest)| uuid::Uuid::parse_str(rest).is_ok())
    }
}

// ---- tokens ----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    /// Principal name (password login).
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// API-key secret (`pik_...`); alternative to name+password.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Project name or id to scope the token to. Defaults to the principal's
    /// default project (or the key's project).
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub ttl_secs: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub token: String,
    pub token_type: String,
    pub expires_at: DateTime<Utc>,
    pub caller: Caller,
}

/// A login has no caller, so the handler says who it was for. The audit
/// layer reads this off the response: the principal and key once they are
/// known, or the name that was tried. Never the password or the key secret.
async fn login(State(st): State<AppState>, Body(req): Body<TokenRequest>) -> Response {
    let mut subject = crate::audit::Subject {
        principal: req.name.clone().filter(|_| req.api_key.is_none()),
        ..Default::default()
    };
    let mut response = match issue_login(&st, req, &mut subject) {
        Ok(token) => Json(token).into_response(),
        Err(err) => err.into_response(),
    };
    response.extensions_mut().insert(subject);
    response
}

fn issue_login(
    st: &AppState,
    req: TokenRequest,
    subject: &mut crate::audit::Subject,
) -> ApiResult<TokenResponse> {
    let store = &st.store;
    let invalid = || ApiError::unauthorized("invalid credentials");

    let (principal, key_project, key) = if let Some(secret) = req.api_key.as_deref() {
        let hash = credentials::hash_api_key(secret);
        let key = store
            .collection::<ApiKey>()
            .list(None)?
            .into_iter()
            .find(|k| k.spec.hash == hash)
            .ok_or_else(invalid)?;
        subject.api_key = Some(key.meta.id.clone());
        subject.principal_id = Some(key.spec.principal.clone());
        if key.is_expired(Utc::now()) {
            return Err(ApiError::unauthorized("api key expired"));
        }
        let p = store
            .collection::<Principal>()
            .try_get(&key.spec.principal)?
            .ok_or_else(invalid)?;
        (p, key.spec.project.clone(), Some(key.meta.id))
    } else {
        let name = req
            .name
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("name or api_key required"))?;
        let password = req
            .password
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("password required"))?;
        let p = store
            .collection::<Principal>()
            .try_get_by_name(name)?
            .ok_or_else(invalid)?;
        let hash = p.spec.password_hash.as_deref().ok_or_else(invalid)?;
        if !credentials::verify_password(password, hash) {
            tracing::info!(principal = name, "failed password login");
            return Err(invalid());
        }
        (p, None, None)
    };

    let project = match req.project.as_deref() {
        Some(p) => Some(crate::resolve_project(store, p)?.meta.id),
        None => key_project.or_else(|| principal.spec.default_project.clone()),
    };
    let ttl = match req.ttl_secs {
        Some(s) if s <= 0 => return Err(ApiError::bad_request("ttl_secs must be positive")),
        Some(s) => Duration::seconds(s).min(MAX_TTL),
        None => DEFAULT_TTL,
    };
    subject.principal = Some(principal.meta.name.clone());
    subject.principal_id = Some(principal.meta.id.clone());
    subject.project = project.clone();
    issue_token(st, &principal, project, key, ttl)
}

/// Resolve roles from bindings and sign a token. Library entry point so
/// other modules (service-account issuance) can mint tokens too.
pub fn issue_token(
    st: &AppState,
    principal: &Principal,
    project: Option<Id>,
    key: Option<Id>,
    ttl: Duration,
) -> ApiResult<TokenResponse> {
    if !principal.spec.enabled {
        return Err(ApiError::unauthorized("principal is disabled"));
    }
    let caller = caller_for(&st.store, principal, project)?;
    let (token, claims) = st.keys.sign_for_key(&caller, key, ttl)?;
    Ok(TokenResponse {
        token,
        token_type: "Bearer".into(),
        expires_at: claims.expires_at(),
        caller,
    })
}

/// Build the `Caller` for a principal: system roles always apply; project
/// roles apply when the token is scoped to that project.
pub fn caller_for(store: &Store, principal: &Principal, project: Option<Id>) -> ApiResult<Caller> {
    if let Some(p) = &project {
        let proj = store.collection::<Project>().get(p)?;
        if !proj.spec.enabled {
            return Err(ApiError::forbidden(format!(
                "project '{}' is disabled",
                proj.meta.name
            )));
        }
    }
    let bindings = store.collection::<RoleBinding>().list(None)?;
    let mut roles: Vec<String> = Vec::new();
    let mut system = false;
    for b in bindings
        .iter()
        .filter(|b| b.spec.principal == principal.meta.id)
    {
        let applies = match &b.spec.scope {
            Scope::System => {
                if b.spec.role == ROLE_ADMIN {
                    system = true;
                }
                true
            }
            Scope::Project { project: p } => project.as_ref() == Some(p),
        };
        if applies && !roles.contains(&b.spec.role) {
            roles.push(b.spec.role.clone());
        }
    }
    roles.sort();
    Ok(Caller {
        principal: principal.meta.id.clone(),
        principal_name: principal.meta.name.clone(),
        project,
        roles,
        system,
    })
}

async fn whoami(AuthCaller(caller): AuthCaller) -> Json<Caller> {
    Json(caller)
}

// ---- projects --------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateProject {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Parent project name or id.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub quotas: BTreeMap<String, u64>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateProject {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Replaces the whole quota map when present.
    #[serde(default)]
    pub quotas: Option<BTreeMap<String, u64>>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
}

async fn list_projects(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<Project>>> {
    policy::authorize(&caller, "projects.list", None)?;
    Ok(Json(paginate(
        st.store.collection::<Project>().list(None)?,
        &q,
    )))
}

async fn create_project(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Body(req): Body<CreateProject>,
) -> ApiResult<Response> {
    policy::authorize(&caller, "projects.create", None)?;
    validate_name(&req.name)?;
    let parent = match req.parent.as_deref() {
        Some(p) => Some(crate::resolve_project(&st.store, p)?.meta.id),
        None => None,
    };
    let mut meta = Meta::new(kind::PROJECT, req.name, None);
    meta.description = req.description.unwrap_or_default();
    meta.labels = req.labels;
    let p = st.store.collection::<Project>().create(Project {
        meta,
        spec: ProjectSpec {
            parent,
            enabled: req.enabled.unwrap_or(true),
            quotas: req.quotas,
        },
        status: ProjectStatus::default(),
    })?;
    Ok(created(p))
}

async fn get_project(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Json<Project>> {
    let p = crate::resolve_project(&st.store, &id)?;
    policy::authorize(&caller, "projects.get", Some(&p.meta.id))?;
    Ok(Json(p))
}

async fn update_project(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(req): Body<UpdateProject>,
) -> ApiResult<Json<Project>> {
    policy::authorize(&caller, "projects.update", None)?;
    let rev = if_match(&headers)?;
    let id = crate::resolve_project(&st.store, &id)?.meta.id;
    // Applied to the row as it is at the write, so usage a module recorded
    // in the meantime is not put back to what this request first read.
    let p = st.store.collection::<Project>().modify(&id, |p| {
        check_revision(p, rev)?;
        if let Some(d) = &req.description {
            p.meta.description = d.clone();
        }
        if let Some(l) = &req.labels {
            p.meta.labels = l.clone();
        }
        if let Some(e) = req.enabled {
            p.spec.enabled = e;
        }
        if let Some(q) = &req.quotas {
            p.spec.quotas = q.clone();
        }
        Ok(())
    })?;
    Ok(Json(p))
}

fn check_revision(p: &Project, expected: Option<u64>) -> Result<(), StoreError> {
    match expected {
        Some(exp) if exp != p.meta.revision => Err(StoreError::Revision {
            expected: exp,
            found: p.meta.revision,
        }),
        _ => Ok(()),
    }
}

/// One resource in a project's quota report. `limit` is `null` when the
/// resource is unlimited.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotaLine {
    pub limit: Option<u64>,
    pub used: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QuotaReport {
    pub project: Id,
    pub name: String,
    pub quotas: BTreeMap<String, QuotaLine>,
}

fn quota_report(p: &Project) -> QuotaReport {
    let names = crate::quota::RESOURCES
        .iter()
        .copied()
        .chain(p.spec.quotas.keys().map(String::as_str))
        .chain(p.status.usage.keys().map(String::as_str));
    let quotas = names
        .map(|r| {
            let line = QuotaLine {
                limit: p.spec.quotas.get(r).copied(),
                used: p.status.usage.get(r).copied().unwrap_or(0),
            };
            (r.to_owned(), line)
        })
        .collect();
    QuotaReport {
        project: p.meta.id.clone(),
        name: p.meta.name.clone(),
        quotas,
    }
}

async fn get_quotas(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Json<QuotaReport>> {
    let p = crate::resolve_project(&st.store, &id)?;
    policy::authorize(&caller, "projects.get", Some(&p.meta.id))?;
    Ok(Json(quota_report(&p)))
}

/// Limits to change. A number sets the limit, `null` removes it, and a
/// resource that is not named keeps what it has.
#[derive(Debug, Deserialize)]
pub struct SetQuotas {
    pub quotas: BTreeMap<String, Option<u64>>,
}

async fn set_quotas(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
    Body(req): Body<SetQuotas>,
) -> ApiResult<Json<QuotaReport>> {
    policy::authorize(&caller, "projects.update", None)?;
    // A misspelt name would be stored and never enforced.
    if let Some(unknown) = req
        .quotas
        .keys()
        .find(|r| !crate::quota::RESOURCES.contains(&r.as_str()))
    {
        return Err(ApiError::bad_request(format!(
            "unknown quota resource '{unknown}'; known: {}",
            crate::quota::RESOURCES.join(", ")
        )));
    }
    let id = crate::resolve_project(&st.store, &id)?.meta.id;
    let p = st.store.collection::<Project>().modify(&id, |p| {
        for (resource, limit) in &req.quotas {
            match limit {
                Some(n) => p.spec.quotas.insert(resource.clone(), *n),
                None => p.spec.quotas.remove(resource),
            };
        }
        Ok(())
    })?;
    Ok(Json(quota_report(&p)))
}

async fn delete_project(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    policy::authorize(&caller, "projects.delete", None)?;
    let p = crate::resolve_project(&st.store, &id)?;
    if crate::admin_project(&st.store)?.as_ref() == Some(&p.meta.id) {
        return Err(ApiError::forbidden("the admin project cannot be deleted"));
    }
    let children = st
        .store
        .collection::<Project>()
        .list(None)?
        .into_iter()
        .any(|c| c.spec.parent.as_ref() == Some(&p.meta.id));
    if children {
        return Err(ApiError::conflict("project has child projects"));
    }
    // Bindings and keys scoped to the project go with it.
    let bindings = st.store.collection::<RoleBinding>();
    for b in bindings.list(Some(&p.meta.id))? {
        bindings.delete(&b.meta.id)?;
    }
    let keys = st.store.collection::<ApiKey>();
    for k in keys.list(Some(&p.meta.id))? {
        keys.delete(&k.meta.id)?;
    }
    st.store.collection::<Project>().delete(&p.meta.id)?;
    Ok(no_content())
}

// ---- principals ------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreatePrincipal {
    pub name: String,
    #[serde(default)]
    pub kind: PrincipalKind,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    /// Optional for service accounts (they log in with API keys).
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Project name or id.
    #[serde(default)]
    pub default_project: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdatePrincipal {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub default_project: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
}

fn resolve_principal(store: &Store, name_or_id: &str) -> ApiResult<Principal> {
    Ok(store.collection::<Principal>().resolve(None, name_or_id)?)
}

async fn list_principals(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<Principal>>> {
    policy::authorize(&caller, "principals.list", None)?;
    let items = st
        .store
        .collection::<Principal>()
        .list(None)?
        .iter()
        .map(Principal::redacted)
        .collect();
    Ok(Json(paginate(items, &q)))
}

async fn create_principal(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Body(req): Body<CreatePrincipal>,
) -> ApiResult<Response> {
    policy::authorize(&caller, "principals.create", None)?;
    validate_name(&req.name)?;
    let default_project = match req.default_project.as_deref() {
        Some(p) => Some(crate::resolve_project(&st.store, p)?.meta.id),
        None => None,
    };
    let password_hash = match req.password.as_deref() {
        Some(pw) => Some(credentials::hash_password(pw)?),
        None => None,
    };
    let mut meta = Meta::new(kind::PRINCIPAL, req.name, None);
    meta.description = req.description.unwrap_or_default();
    meta.labels = req.labels;
    let p = st.store.collection::<Principal>().create(Principal {
        meta,
        spec: PrincipalSpec {
            kind: req.kind,
            email: req.email,
            password_hash,
            enabled: req.enabled.unwrap_or(true),
            default_project,
        },
        status: PrincipalStatus::default(),
    })?;
    Ok(created(p.redacted()))
}

async fn get_principal(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Json<Principal>> {
    let p = resolve_principal(&st.store, &id)?;
    if p.meta.id != caller.principal {
        policy::authorize(&caller, "principals.get", None)?;
    }
    Ok(Json(p.redacted()))
}

async fn update_principal(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(req): Body<UpdatePrincipal>,
) -> ApiResult<Json<Principal>> {
    let mut p = resolve_principal(&st.store, &id)?;
    let is_self = p.meta.id == caller.principal;
    // A principal may change its own password and email; anything else is admin.
    let only_self_fields = req.enabled.is_none()
        && req.default_project.is_none()
        && req.labels.is_none()
        && req.description.is_none();
    if !(is_self && only_self_fields) {
        policy::authorize(&caller, "principals.update", None)?;
    }
    let rev = if_match(&headers)?;
    if let Some(d) = req.description {
        p.meta.description = d;
    }
    if let Some(l) = req.labels {
        p.meta.labels = l;
    }
    if let Some(e) = req.email {
        p.spec.email = Some(e);
    }
    if let Some(pw) = req.password.as_deref() {
        p.spec.password_hash = Some(credentials::hash_password(pw)?);
    }
    if let Some(e) = req.enabled {
        if is_self && !e {
            return Err(ApiError::bad_request("cannot disable yourself"));
        }
        p.spec.enabled = e;
    }
    if let Some(dp) = req.default_project.as_deref() {
        p.spec.default_project = Some(crate::resolve_project(&st.store, dp)?.meta.id);
    }
    Ok(Json(
        st.store
            .collection::<Principal>()
            .update(p, rev)?
            .redacted(),
    ))
}

async fn delete_principal(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    policy::authorize(&caller, "principals.delete", None)?;
    let p = resolve_principal(&st.store, &id)?;
    if p.meta.id == caller.principal {
        return Err(ApiError::bad_request("cannot delete yourself"));
    }
    let bindings = st.store.collection::<RoleBinding>();
    for b in bindings
        .list(None)?
        .into_iter()
        .filter(|b| b.spec.principal == p.meta.id)
    {
        bindings.delete(&b.meta.id)?;
    }
    let keys = st.store.collection::<ApiKey>();
    for k in keys
        .list(None)?
        .into_iter()
        .filter(|k| k.spec.principal == p.meta.id)
    {
        keys.delete(&k.meta.id)?;
    }
    st.store.collection::<Principal>().delete(&p.meta.id)?;
    Ok(no_content())
}

// ---- signing keys ----------------------------------------------------------

/// The signing keys as the API shows them: never a secret.
#[derive(Debug, Serialize, Deserialize)]
pub struct SigningKeys {
    pub keys: Vec<crate::token::SigningKeyInfo>,
    /// Ids dropped by this call (`retire` only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<String>,
}

fn internal(e: anyhow::Error) -> ApiError {
    ApiError::internal(e.to_string())
}

async fn list_signing_keys(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
) -> ApiResult<Json<SigningKeys>> {
    policy::authorize(&caller, "signing_keys.list", None)?;
    let set = crate::token::KeySet::load(&st.store)
        .map_err(internal)?
        .unwrap_or_default();
    Ok(Json(SigningKeys {
        keys: set.describe(),
        retired: Vec::new(),
    }))
}

async fn rotate_signing_key(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
) -> ApiResult<Json<SigningKeys>> {
    policy::authorize(&caller, "signing_keys.rotate", None)?;
    let set = crate::token::rotate(&st.store, Utc::now()).map_err(internal)?;
    tracing::info!(
        current = set.current_kid(),
        keys = set.describe().len(),
        by = %caller.principal_name,
        "token signing key rotated"
    );
    Ok(Json(SigningKeys {
        keys: set.describe(),
        retired: Vec::new(),
    }))
}

async fn retire_signing_keys(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
) -> ApiResult<Json<SigningKeys>> {
    policy::authorize(&caller, "signing_keys.retire", None)?;
    let (set, retired) = crate::token::retire(&st.store).map_err(internal)?;
    tracing::info!(
        current = set.current_kid(),
        retired = ?retired,
        by = %caller.principal_name,
        "token signing keys retired"
    );
    Ok(Json(SigningKeys {
        keys: set.describe(),
        retired,
    }))
}

// ---- roles -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateRole {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateRole {
    #[serde(default)]
    pub description: Option<String>,
}

fn resolve_role(store: &Store, name_or_id: &str) -> ApiResult<Role> {
    Ok(store.collection::<Role>().resolve(None, name_or_id)?)
}

async fn list_roles(
    State(st): State<AppState>,
    AuthCaller(_caller): AuthCaller,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<Role>>> {
    Ok(Json(paginate(
        st.store.collection::<Role>().list(None)?,
        &q,
    )))
}

async fn create_role(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Body(req): Body<CreateRole>,
) -> ApiResult<Response> {
    policy::authorize(&caller, "roles.create", None)?;
    validate_name(&req.name)?;
    let mut meta = Meta::new(kind::ROLE, req.name, None);
    meta.description = req.description.unwrap_or_default();
    let r = st.store.collection::<Role>().create(Role {
        meta,
        spec: RoleSpec { builtin: false },
        status: RoleStatus::default(),
    })?;
    Ok(created(r))
}

async fn get_role(
    State(st): State<AppState>,
    AuthCaller(_caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Json<Role>> {
    Ok(Json(resolve_role(&st.store, &id)?))
}

async fn update_role(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(req): Body<UpdateRole>,
) -> ApiResult<Json<Role>> {
    policy::authorize(&caller, "roles.update", None)?;
    let rev = if_match(&headers)?;
    let mut r = resolve_role(&st.store, &id)?;
    if let Some(d) = req.description {
        r.meta.description = d;
    }
    Ok(Json(st.store.collection::<Role>().update(r, rev)?))
}

async fn delete_role(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    policy::authorize(&caller, "roles.delete", None)?;
    let r = resolve_role(&st.store, &id)?;
    if r.spec.builtin {
        return Err(ApiError::forbidden(format!(
            "role '{}' is built in",
            r.meta.name
        )));
    }
    let in_use = st
        .store
        .collection::<RoleBinding>()
        .list(None)?
        .iter()
        .any(|b| b.spec.role == r.meta.name);
    if in_use {
        return Err(ApiError::conflict(format!(
            "role '{}' is still bound",
            r.meta.name
        )));
    }
    st.store.collection::<Role>().delete(&r.meta.id)?;
    Ok(no_content())
}

// ---- bindings --------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateBinding {
    /// Principal name or id.
    pub principal: String,
    /// Role name.
    pub role: String,
    /// Project name or id. Absent = system scope.
    #[serde(default)]
    pub project: Option<String>,
}

fn resolve_binding(store: &Store, name_or_id: &str) -> ApiResult<RoleBinding> {
    let col = store.collection::<RoleBinding>();
    if let Some(id) = Id::parse(KIND_BINDING, name_or_id) {
        return Ok(col.get(&id)?);
    }
    // Binding names are `principal:role:scope`; they may live under a project,
    // so search rather than rely on the (project-keyed) name index.
    col.list(None)?
        .into_iter()
        .find(|b| b.meta.name == name_or_id)
        .ok_or_else(|| ApiError::not_found(format!("binding '{name_or_id}' not found")))
}

async fn list_bindings(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<RoleBinding>>> {
    policy::authorize(&caller, "bindings.list", None)?;
    Ok(Json(paginate(
        st.store.collection::<RoleBinding>().list(None)?,
        &q,
    )))
}

async fn create_binding(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Body(req): Body<CreateBinding>,
) -> ApiResult<Response> {
    policy::authorize(&caller, "bindings.create", None)?;
    let principal = resolve_principal(&st.store, &req.principal)?;
    let role = resolve_role(&st.store, &req.role)?;
    let (scope, project_name) = match req.project.as_deref() {
        Some(p) => {
            let proj = crate::resolve_project(&st.store, p)?;
            (
                Scope::Project {
                    project: proj.meta.id,
                },
                Some(proj.meta.name),
            )
        }
        None => (Scope::System, None),
    };
    let name = binding_name(
        &principal.meta.name,
        &role.meta.name,
        &scope,
        project_name.as_deref(),
    );
    let b = st.store.collection::<RoleBinding>().create(RoleBinding {
        meta: Meta::new(KIND_BINDING, name, scope.project().cloned()),
        spec: RoleBindingSpec {
            principal: principal.meta.id,
            role: role.meta.name,
            scope,
        },
        status: RoleBindingStatus::default(),
    })?;
    Ok(created(b))
}

async fn get_binding(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Json<RoleBinding>> {
    let b = resolve_binding(&st.store, &id)?;
    if b.spec.principal != caller.principal {
        policy::authorize(&caller, "bindings.get", None)?;
    }
    Ok(Json(b))
}

async fn delete_binding(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    policy::authorize(&caller, "bindings.delete", None)?;
    let b = resolve_binding(&st.store, &id)?;
    st.store.collection::<RoleBinding>().delete(&b.meta.id)?;
    Ok(no_content())
}

// ---- api keys --------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateKey {
    pub name: String,
    /// Principal name or id. Defaults to the caller; only admins may set it.
    #[serde(default)]
    pub principal: Option<String>,
    /// Project name or id the key's tokens are scoped to.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreatedKey {
    #[serde(flatten)]
    pub key: ApiKey,
    /// Shown exactly once.
    pub secret: String,
}

async fn list_keys(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<ApiKey>>> {
    let mut items = st.store.collection::<ApiKey>().list(None)?;
    if policy::authorize(&caller, "keys.list", None).is_err() {
        items.retain(|k| k.spec.principal == caller.principal);
    }
    Ok(Json(paginate(items, &q)))
}

async fn create_key(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Body(req): Body<CreateKey>,
) -> ApiResult<Response> {
    validate_name(&req.name)?;
    let principal = match req.principal.as_deref() {
        Some(p) => {
            let p = resolve_principal(&st.store, p)?;
            if p.meta.id != caller.principal {
                policy::authorize(&caller, "keys.create", None)?;
            }
            p
        }
        None => resolve_principal(&st.store, &caller.principal.to_string())?,
    };
    let project = match req.project.as_deref() {
        Some(p) => Some(crate::resolve_project(&st.store, p)?.meta.id),
        None => principal.spec.default_project.clone(),
    };
    if let Some(exp) = req.expires_at {
        if exp <= Utc::now() {
            return Err(ApiError::bad_request("expires_at is in the past"));
        }
    }
    let (secret, hash) = credentials::new_api_key();
    // Keys are named per principal: `alice/laptop`, so two users may both have "laptop".
    let name = format!("{}/{}", principal.meta.name, req.name);
    let key = st.store.collection::<ApiKey>().create(ApiKey {
        meta: Meta::new(KIND_KEY, name, project.clone()),
        spec: ApiKeySpec {
            principal: principal.meta.id,
            hash,
            expires_at: req.expires_at,
            project,
        },
        status: ApiKeyStatus::default(),
    })?;
    Ok(created(CreatedKey { key, secret }))
}

fn resolve_key(store: &Store, caller: &Caller, name_or_id: &str) -> ApiResult<ApiKey> {
    let col = store.collection::<ApiKey>();
    let key = if let Some(id) = Id::parse(KIND_KEY, name_or_id) {
        col.get(&id)?
    } else {
        // Accept `laptop` (own key) or `alice/laptop`.
        let full = if name_or_id.contains('/') {
            name_or_id.to_owned()
        } else {
            format!("{}/{}", caller.principal_name, name_or_id)
        };
        col.list(None)?
            .into_iter()
            .find(|k| k.meta.name == full)
            .ok_or_else(|| ApiError::not_found(format!("key '{name_or_id}' not found")))?
    };
    if key.spec.principal != caller.principal {
        policy::authorize(caller, "keys.get", None)?;
    }
    Ok(key)
}

async fn get_key(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Json<ApiKey>> {
    Ok(Json(resolve_key(&st.store, &caller, &id)?))
}

async fn delete_key(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let key = resolve_key(&st.store, &caller, &id)?;
    st.store.collection::<ApiKey>().delete(&key.meta.id)?;
    Ok(no_content())
}

// ---- policy ----------------------------------------------------------------

/// The audit log. Cloud admin only: a record carries the project of the
/// token that made the request, not of everything the request touched, so
/// a per-project view would leave out what an admin did to the project.
async fn list_audit(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    query: Result<Query<crate::audit::AuditQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Json<Page<crate::audit::AuditEvent>>> {
    policy::authorize(&caller, "audit.list", None)?;
    let Query(mut q) = query.map_err(|e| ApiError::bad_request(e.body_text()))?;
    // A project may be given by name.
    if let Some(project) = q.project.as_deref() {
        if let Ok(found) = crate::resolve_project(&st.store, project) {
            q.project = Some(found.meta.id.to_string());
        }
    }
    crate::audit::list(&st.store, &q)
        .map(Json)
        .map_err(ApiError::bad_request)
}

async fn get_policy(AuthCaller(_caller): AuthCaller) -> Json<policy::PolicyDocument> {
    Json(policy::document())
}
