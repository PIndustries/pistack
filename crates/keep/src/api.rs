// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP handlers, mounted by the gateway at `/secrets`.
//!
//! ```text
//! GET|POST  /secrets                      list (payload never included) / create with payload
//! GET|PUT|DELETE /secrets/{id}            get / update spec+metadata (If-Match) / delete
//! GET|PUT   /secrets/{id}/payload         fetch decrypted payload / rotate (version + 1)
//! GET|POST  /certificates                 list / issue immediately
//! GET|DELETE /certificates/{id}           get (cert_pem in status) / delete with its key
//! GET       /certificates/{id}/key        private key PEM (project admin)
//! POST      /certificates/{id}/revoke     revoke on its CA
//! GET       /cas                          (admin) list CAs
//! GET       /cas/{id}                     (admin)
//! GET       /cas/{id}/certificate         CA certificate PEM (any caller)
//! GET       /cas/{id}/crl                 revoked serials, JSON (any caller)
//! POST      /cas/{id}/revoke              (admin) { "serial": n }
//! GET|POST  /access-keys                  list (no secrets) / create (secret shown once)
//! GET|DELETE /access-keys/{id}            get / revoke
//! GET|POST  /join-tokens                  (admin) list / mint (clear text shown once)
//! GET|DELETE /join-tokens/{id}            (admin)
//! POST      /join-tokens/redeem           (no auth) { "token": "pij_..." } -> JoinGrant
//! ```
//!
//! `{id}` accepts a name or an id. Non-admins see only their project;
//! readers may list and get but not read payloads or keys; members may
//! create, rotate and delete; the key PEM of a certificate needs the admin
//! role. CA and join-token operations are admin only except where noted.
//!
//! An access key's `{id}` is its secret's id (`sec-…`) or its access key id
//! (`PIAK…`). Members create keys for themselves; naming another principal
//! needs the admin role. Members may revoke any key in their project, as
//! with secrets.
//!
//! `keep` has no reconciler: issuance is synchronous, so these handlers are
//! the ones that fill `status`.

use axum::{
    extract::{FromRequest, FromRequestParts, Path, Query, Request, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Duration, Utc};
use pistack_types::{ApiError, ApiResult, Caller, Id, Kind, ListQuery, Page};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    access_key::{is_access_key_id, AccessKey, CreatedAccessKey},
    ca::PEM_CONTENT_TYPE,
    join::{create_join_token_with, redeem_join_token, NewJoinToken},
    paginate,
    types::*,
    Client,
};

pub fn router(client: Client) -> Router {
    Router::new()
        .route("/secrets", get(list_secrets).post(create_secret))
        .route(
            "/secrets/{id}",
            get(get_secret).put(update_secret).delete(delete_secret),
        )
        .route(
            "/secrets/{id}/payload",
            get(get_payload).put(rotate_payload),
        )
        .route(
            "/certificates",
            get(list_certificates).post(create_certificate),
        )
        .route(
            "/certificates/{id}",
            get(get_certificate).delete(delete_certificate),
        )
        .route("/certificates/{id}/key", get(get_certificate_key))
        .route("/certificates/{id}/revoke", post(revoke_certificate))
        .route("/cas", get(list_cas))
        .route("/cas/{id}", get(get_ca))
        .route("/cas/{id}/certificate", get(get_ca_certificate))
        .route("/cas/{id}/crl", get(get_ca_crl))
        .route("/cas/{id}/revoke", post(revoke_on_ca))
        .route(
            "/access-keys",
            get(list_access_keys).post(create_access_key),
        )
        .route(
            "/access-keys/{id}",
            get(get_access_key).delete(revoke_access_key),
        )
        .route(
            "/join-tokens",
            get(list_join_tokens).post(create_join_token_h),
        )
        .route("/join-tokens/redeem", post(redeem_join_token_h))
        .route(
            "/join-tokens/{id}",
            get(get_join_token).delete(delete_join_token),
        )
        .fallback(|| async { ApiError::not_found("no such secrets endpoint") })
        .with_state(client)
}

// ---- extractors --------------------------------------------------------------

/// Who is asking. The gateway's auth layer puts a `Caller` in request
/// extensions; without one the request is refused.
#[derive(Clone, Debug)]
pub struct Scope(pub Caller);

impl<S: Send + Sync> FromRequestParts<S> for Scope {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Caller>()
            .cloned()
            .map(Scope)
            .ok_or_else(|| ApiError::unauthorized("no credentials"))
    }
}

impl Scope {
    fn caller(&self) -> &Caller {
        &self.0
    }
    /// Cloud admin without a project scope: sees everything.
    fn is_cloud_admin(&self) -> bool {
        self.0.is_admin() && self.0.project.is_none()
    }
    /// Project filter for list calls.
    fn filter(&self) -> Option<&Id> {
        self.0.project.as_ref()
    }
    fn can_see(&self, project: Option<&Id>) -> bool {
        self.is_cloud_admin() || (self.0.project.is_some() && project == self.0.project.as_ref())
    }
    fn check_visible<K: Kind>(&self, item: &K) -> ApiResult<()> {
        if self.can_see(item.meta().project.as_ref()) {
            Ok(())
        } else {
            Err(ApiError::not_found(format!(
                "{} {} not found",
                K::KIND,
                item.meta().id
            )))
        }
    }
    /// Member or admin: may create, change, rotate, delete, read payloads.
    fn require_write(&self) -> ApiResult<()> {
        if self.0.is_admin() || self.0.has_role("member") {
            Ok(())
        } else {
            Err(ApiError::forbidden("member role required"))
        }
    }
    fn require_admin(&self) -> ApiResult<()> {
        if self.0.is_admin() {
            Ok(())
        } else {
            Err(ApiError::forbidden("admin role required"))
        }
    }
    /// Project a create lands in. Admins may name one; others get their own.
    fn target_project(&self, requested: Option<Id>) -> ApiResult<Option<Id>> {
        match (requested, &self.0.project) {
            (Some(p), Some(own)) if &p != own && !self.0.is_admin() => Err(ApiError::forbidden(
                "cannot create resources in another project",
            )),
            (Some(p), _) => Ok(Some(p)),
            (None, own) => Ok(own.clone()),
        }
    }
}

/// `Json<T>` whose rejection is an `ApiError`.
pub struct Body<T>(pub T);

impl<S: Send + Sync, T: serde::de::DeserializeOwned> FromRequest<S> for Body<T> {
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
    let looks_like_id = name
        .split_once('-')
        .is_some_and(|(_, rest)| uuid::Uuid::parse_str(rest).is_ok());
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !looks_like_id;
    if !ok {
        return Err(ApiError::bad_request(
            "name must be 1-63 chars of [a-zA-Z0-9._-] and must not look like an id",
        ));
    }
    Ok(())
}

/// Resolve `{id}` (name or id) within the caller's scope and check visibility.
fn resolve<K: Kind>(client: &Client, scope: &Scope, name_or_id: &str) -> ApiResult<K> {
    let item = client
        .store()
        .collection::<K>()
        .resolve(scope.filter(), name_or_id)?;
    scope.check_visible(&item)?;
    Ok(item)
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    #[default]
    Utf8,
    Base64,
}

fn decode_payload(payload: &str, encoding: Encoding) -> ApiResult<Vec<u8>> {
    match encoding {
        Encoding::Utf8 => Ok(payload.as_bytes().to_vec()),
        Encoding::Base64 => B64
            .decode(payload.trim())
            .map_err(|e| ApiError::bad_request(format!("payload is not base64: {e}"))),
    }
}

// ---- secrets -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateSecret {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub kind: SecretKind,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub rotate_after_days: Option<u32>,
    /// The payload, as text or base64 per `encoding`.
    pub payload: String,
    #[serde(default)]
    pub encoding: Encoding,
    /// Project id (admins only; others are scoped to their own).
    #[serde(default)]
    pub project: Option<Id>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateSecret {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub rotate_after_days: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct PayloadBody {
    pub payload: String,
    #[serde(default)]
    pub encoding: Encoding,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PayloadResponse {
    pub id: Id,
    pub version: u64,
    pub content_type: String,
    /// Always `base64`.
    pub encoding: Encoding,
    pub payload: String,
}

async fn list_secrets(
    State(c): State<Client>,
    scope: Scope,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<Secret>>> {
    if !scope.is_cloud_admin() && scope.filter().is_none() {
        return Err(ApiError::forbidden("a project scope is required"));
    }
    Ok(Json(paginate(c.list_secrets(scope.filter())?, &q)))
}

async fn create_secret(
    State(c): State<Client>,
    scope: Scope,
    Body(req): Body<CreateSecret>,
) -> ApiResult<Response> {
    scope.require_write()?;
    validate_name(&req.name)?;
    let project = scope.target_project(req.project)?;
    let payload = decode_payload(&req.payload, req.encoding)?;
    let mut secret = Secret::new(
        req.name,
        project,
        SecretSpec {
            kind: req.kind,
            content_type: req.content_type.unwrap_or_else(|| match req.kind {
                SecretKind::Certificate | SecretKind::SshKey => PEM_CONTENT_TYPE.to_owned(),
                SecretKind::Password | SecretKind::Token => "text/plain".to_owned(),
                SecretKind::Opaque => SecretSpec::default().content_type,
            }),
            expires_at: req.expires_at,
            rotate_after_days: req.rotate_after_days,
        },
    );
    secret.meta.description = req.description.unwrap_or_default();
    secret.meta.labels = req.labels;
    Ok(created(c.create_secret(secret, &payload)?))
}

async fn get_secret(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Secret>> {
    Ok(Json(resolve::<Secret>(&c, &scope, &id)?))
}

async fn update_secret(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(req): Body<UpdateSecret>,
) -> ApiResult<Json<Secret>> {
    scope.require_write()?;
    let rev = if_match(&headers)?;
    let mut s = resolve::<Secret>(&c, &scope, &id)?;
    if let Some(d) = req.description {
        s.meta.description = d;
    }
    if let Some(l) = req.labels {
        s.meta.labels = l;
    }
    if let Some(ct) = req.content_type {
        s.spec.content_type = ct;
    }
    if req.expires_at.is_some() {
        s.spec.expires_at = req.expires_at;
    }
    if req.rotate_after_days.is_some() {
        s.spec.rotate_after_days = req.rotate_after_days;
    }
    Ok(Json(c.update_secret(s, rev)?))
}

async fn delete_secret(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    scope.require_write()?;
    let s = resolve::<Secret>(&c, &scope, &id)?;
    match crate::client::managed_role(&s.meta.labels) {
        Some(crate::ca::ROLE_PRIVATE_KEY) => {
            return Err(ApiError::conflict(
                "this secret holds a certificate's key; delete the certificate instead",
            ))
        }
        Some(crate::access_key::ROLE_ACCESS_KEY) => {
            return Err(ApiError::conflict(
                "this secret is an access key; revoke it with DELETE /secrets/access-keys/{id}",
            ))
        }
        Some(_) => return Err(ApiError::conflict("this secret is managed by keep")),
        None => {}
    }
    c.delete_secret(&s.meta.id)?;
    Ok(no_content())
}

// ---- access keys -------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct CreateAccessKey {
    #[serde(default)]
    pub description: Option<String>,
    /// Project id (admins only; others are scoped to their own).
    #[serde(default)]
    pub project: Option<Id>,
    /// Principal the key acts as. Defaults to the caller; naming anyone
    /// else needs the admin role.
    #[serde(default)]
    pub principal: Option<Id>,
}

/// `{id}` is a `sec-…` id or a `PIAK…` access key id.
fn resolve_access_key(c: &Client, scope: &Scope, id: &str) -> ApiResult<AccessKey> {
    let not_found = || ApiError::not_found(format!("access key {id} not found"));
    let key = if is_access_key_id(id) {
        c.find_access_key(id)?
    } else {
        let sid = Id::parse(pistack_types::kind::SECRET, id).ok_or_else(not_found)?;
        c.get_access_key(&sid)?
    };
    if !scope.can_see(Some(&key.project)) {
        return Err(not_found());
    }
    Ok(key)
}

async fn list_access_keys(
    State(c): State<Client>,
    scope: Scope,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<AccessKey>>> {
    if !scope.is_cloud_admin() && scope.filter().is_none() {
        return Err(ApiError::forbidden("a project scope is required"));
    }
    let keys: Vec<Secret> = c
        .list_secrets(scope.filter())?
        .into_iter()
        .filter(crate::access_key::is_access_key)
        .collect();
    let page = paginate(keys, &q);
    Ok(Json(Page {
        items: page
            .items
            .iter()
            .map(crate::access_key::view)
            .collect::<crate::Result<_>>()?,
        next: page.next,
        total: page.total,
    }))
}

async fn create_access_key(
    State(c): State<Client>,
    scope: Scope,
    Body(req): Body<CreateAccessKey>,
) -> ApiResult<Response> {
    scope.require_write()?;
    let project = scope
        .target_project(req.project)?
        .ok_or_else(|| ApiError::bad_request("an access key needs a project"))?;
    let principal = match req.principal {
        Some(p) if p != scope.caller().principal => {
            scope.require_admin()?;
            p
        }
        _ => scope.caller().principal.clone(),
    };
    let key: CreatedAccessKey = c.create_access_key(
        &project,
        &principal,
        req.description.as_deref().unwrap_or(""),
    )?;
    Ok(created(key))
}

async fn get_access_key(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<AccessKey>> {
    Ok(Json(resolve_access_key(&c, &scope, &id)?))
}

async fn revoke_access_key(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    scope.require_write()?;
    let key = resolve_access_key(&c, &scope, &id)?;
    c.revoke_access_key(&key.id)?;
    Ok(no_content())
}

async fn get_payload(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<PayloadResponse>> {
    scope.require_write()?;
    let s = resolve::<Secret>(&c, &scope, &id)?;
    let bytes = c.get_secret_payload(&s.meta.id)?;
    Ok(Json(PayloadResponse {
        id: s.meta.id,
        version: s.status.version,
        content_type: s.spec.content_type,
        encoding: Encoding::Base64,
        payload: B64.encode(bytes),
    }))
}

async fn rotate_payload(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(req): Body<PayloadBody>,
) -> ApiResult<Json<Secret>> {
    scope.require_write()?;
    let rev = if_match(&headers)?;
    let s = resolve::<Secret>(&c, &scope, &id)?;
    let payload = decode_payload(&req.payload, req.encoding)?;
    Ok(Json(c.rotate_secret(&s.meta.id, &payload, rev)?))
}

// ---- certificates ------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateCertificate {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub common_name: String,
    #[serde(default)]
    pub sans: Vec<String>,
    #[serde(default)]
    pub usage: CertUsage,
    #[serde(default)]
    pub ttl_days: Option<u32>,
    /// CA id; defaults to the issuing CA.
    #[serde(default)]
    pub issuer: Option<Id>,
    #[serde(default)]
    pub project: Option<Id>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CertificateKey {
    pub id: Id,
    pub cert_pem: String,
    pub key_pem: String,
    pub ca_pem: String,
}

async fn list_certificates(
    State(c): State<Client>,
    scope: Scope,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<Certificate>>> {
    if !scope.is_cloud_admin() && scope.filter().is_none() {
        return Err(ApiError::forbidden("a project scope is required"));
    }
    Ok(Json(paginate(c.list_certificates(scope.filter())?, &q)))
}

async fn create_certificate(
    State(c): State<Client>,
    scope: Scope,
    Body(req): Body<CreateCertificate>,
) -> ApiResult<Response> {
    scope.require_write()?;
    validate_name(&req.name)?;
    let project = scope.target_project(req.project)?;
    if req.issuer.is_some() && !scope.caller().is_admin() {
        return Err(ApiError::forbidden("choosing the issuer requires admin"));
    }
    let spec = CertificateSpec {
        common_name: req.common_name,
        sans: req.sans,
        usage: req.usage,
        ttl_days: req.ttl_days.unwrap_or_else(default_ttl_days),
        issuer: req.issuer,
    };
    let cert = c.issue_certificate_with(
        project,
        &req.name,
        spec,
        req.description.unwrap_or_default(),
        req.labels,
    )?;
    Ok(created(cert))
}

async fn get_certificate(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Certificate>> {
    Ok(Json(resolve::<Certificate>(&c, &scope, &id)?))
}

async fn get_certificate_key(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<CertificateKey>> {
    scope.require_admin()?;
    let cert = resolve::<Certificate>(&c, &scope, &id)?;
    let key_pem = c.certificate_key_pem(&cert)?;
    Ok(Json(CertificateKey {
        id: cert.meta.id,
        cert_pem: cert.status.cert_pem,
        key_pem,
        ca_pem: c.ca_bundle_pem(),
    }))
}

async fn revoke_certificate(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Certificate>> {
    scope.require_write()?;
    let cert = resolve::<Certificate>(&c, &scope, &id)?;
    Ok(Json(c.revoke_certificate(&cert.meta.id)?))
}

async fn delete_certificate(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    scope.require_write()?;
    let cert = resolve::<Certificate>(&c, &scope, &id)?;
    c.delete_certificate(&cert.meta.id)?;
    Ok(no_content())
}

// ---- certificate authorities -------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RevokeSerial {
    pub serial: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Crl {
    pub issuer: Id,
    pub issuer_name: String,
    pub serials: Vec<u64>,
    pub revoked: Vec<Revocation>,
    pub generated_at: DateTime<Utc>,
}

fn resolve_ca(c: &Client, name_or_id: &str) -> ApiResult<CertificateAuthority> {
    Ok(c.store()
        .collection::<CertificateAuthority>()
        .resolve(None, name_or_id)?)
}

async fn list_cas(
    State(c): State<Client>,
    scope: Scope,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<CertificateAuthority>>> {
    scope.require_admin()?;
    Ok(Json(paginate(c.list_cas()?, &q)))
}

async fn get_ca(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<CertificateAuthority>> {
    scope.require_admin()?;
    Ok(Json(resolve_ca(&c, &id)?))
}

async fn get_ca_certificate(
    State(c): State<Client>,
    _scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let ca = resolve_ca(&c, &id)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, PEM_CONTENT_TYPE)],
        ca.status.cert_pem,
    )
        .into_response())
}

async fn get_ca_crl(
    State(c): State<Client>,
    _scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Crl>> {
    let ca = resolve_ca(&c, &id)?;
    let revoked = c.ca().crl(&ca.meta.id)?;
    Ok(Json(Crl {
        issuer: ca.meta.id,
        issuer_name: ca.meta.name,
        serials: revoked.iter().map(|r| r.serial).collect(),
        revoked,
        generated_at: Utc::now(),
    }))
}

async fn revoke_on_ca(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
    Body(req): Body<RevokeSerial>,
) -> ApiResult<Json<CertificateAuthority>> {
    scope.require_admin()?;
    let ca = resolve_ca(&c, &id)?;
    let updated = c.ca().revoke(&ca.meta.id, req.serial)?;
    // Mirror onto the certificate resource when we have it.
    let certs = c.store().collection::<Certificate>();
    for cert in certs.list(None)? {
        if cert.status.serial == req.serial && cert.status.issuer.as_ref() == Some(&ca.meta.id) {
            certs.modify(&cert.meta.id, |x| {
                x.status.revoked = true;
                Ok(())
            })?;
        }
    }
    Ok(Json(updated))
}

// ---- join tokens -------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateJoinToken {
    pub name: String,
    #[serde(default)]
    pub multi_use: bool,
    /// Redemptions a multi-use token allows; default and maximum
    /// `[modules.keep] max_join_token_uses`.
    #[serde(default)]
    pub max_uses: Option<u64>,
    /// Lifetime in seconds; default and maximum 24h.
    #[serde(default)]
    pub ttl_secs: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinTokenResponse {
    /// Shown once. Give it to `pistack agent --join`.
    pub token: String,
    pub join_token: JoinToken,
}

#[derive(Debug, Deserialize)]
pub struct RedeemJoinToken {
    pub token: String,
}

async fn list_join_tokens(
    State(c): State<Client>,
    scope: Scope,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<JoinToken>>> {
    scope.require_admin()?;
    Ok(Json(paginate(
        c.store().collection::<JoinToken>().list(None)?,
        &q,
    )))
}

async fn create_join_token_h(
    State(c): State<Client>,
    scope: Scope,
    Body(req): Body<CreateJoinToken>,
) -> ApiResult<Response> {
    scope.require_admin()?;
    validate_name(&req.name)?;
    let ttl = match req.ttl_secs {
        Some(s) if s <= 0 => return Err(ApiError::bad_request("ttl_secs must be positive")),
        Some(s) => Some(Duration::seconds(s)),
        None => None,
    };
    let (join_token, token) = create_join_token_with(
        c.store(),
        &NewJoinToken {
            name: &req.name,
            created_by: &scope.caller().principal_name,
            multi_use: req.multi_use,
            max_uses: req.max_uses,
            ttl,
        },
        c.limits(),
    )?;
    Ok(created(JoinTokenResponse { token, join_token }))
}

async fn get_join_token(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<JoinToken>> {
    scope.require_admin()?;
    Ok(Json(
        c.store().collection::<JoinToken>().resolve(None, &id)?,
    ))
}

async fn delete_join_token(
    State(c): State<Client>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    scope.require_admin()?;
    let col = c.store().collection::<JoinToken>();
    let t = col.resolve(None, &id)?;
    col.delete(&t.meta.id)?;
    Ok(no_content())
}

async fn redeem_join_token_h(
    State(c): State<Client>,
    Body(req): Body<RedeemJoinToken>,
) -> ApiResult<Json<JoinGrant>> {
    Ok(Json(redeem_join_token(c.store(), &req.token)?))
}
