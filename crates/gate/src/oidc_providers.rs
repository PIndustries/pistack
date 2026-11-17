// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Managing the stored identity providers: create, list, get, delete. Cloud
//! admins only — every action is a system-scoped permission, so `authorize`
//! with no project admits a cloud admin and nobody else.
//!
//! A provider is stored whole. Its issuer is checked to be a URL gate may
//! fetch (HTTPS, or HTTP only to loopback) at create time, so a typo that would
//! make the provider unusable is refused then rather than at first sign-in.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::api::{validate_name, Body, ListParams};
use crate::oidc_discovery;
use crate::token::AuthCaller;
use crate::types::{
    GroupBinding, IdentityProvider, IdentityProviderSpec, IdentityProviderStatus, KIND_PROVIDER,
};
use crate::{paginate, policy, AppState, TryGetByName};
use pistack_types::{ApiError, ApiResult, Caller, Meta, Page};

/// The issuer and audience a provider record stores, trimmed of the surrounding
/// whitespace an operator tends to paste in.
#[derive(Debug, Deserialize)]
pub struct CreateProvider {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub issuer: String,
    pub client_id: String,
    /// The claim that names the user. Defaults to `email`.
    #[serde(default)]
    pub name_claim: Option<String>,
    /// The claim holding group names, when groups map to roles.
    #[serde(default)]
    pub groups_claim: Option<String>,
    #[serde(default)]
    pub group_map: BTreeMap<String, Vec<GroupBinding>>,
    /// Create an account on first sign-in when none matches. Default false.
    #[serde(default)]
    pub create_unknown: Option<bool>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// `POST /identity/providers`.
pub async fn create(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Body(req): Body<CreateProvider>,
) -> ApiResult<Response> {
    authorize(&caller, "providers.create")?;
    validate_name(&req.name)?;
    let name_claim = req.name_claim.unwrap_or_else(|| "email".to_owned());
    if name_claim.is_empty() {
        return Err(ApiError::bad_request("name_claim must not be empty"));
    }
    let issuer = req.issuer.trim();
    let client_id = req.client_id.trim();
    if issuer.is_empty() || client_id.is_empty() {
        return Err(ApiError::bad_request("issuer and client_id are required"));
    }
    // Refuse an issuer gate could never reach, rather than store one that
    // fails every sign-in with the uniform answer.
    if !oidc_discovery::issuer_is_reachable(issuer) {
        return Err(ApiError::bad_request(
            "issuer must be an https URL (http only for 127.0.0.1 or localhost)",
        ));
    }
    let providers = st.store.collection::<IdentityProvider>();
    if providers.try_get_by_name(&req.name)?.is_some() {
        return Err(ApiError::conflict(format!(
            "provider '{}' already exists",
            req.name
        )));
    }
    let mut meta = Meta::new(KIND_PROVIDER, req.name, None);
    meta.description = req.description.unwrap_or_default();
    meta.labels = req.labels;
    let provider = providers.create(IdentityProvider {
        meta,
        spec: IdentityProviderSpec {
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
            name_claim,
            groups_claim: req.groups_claim.filter(|g| !g.is_empty()),
            group_map: req.group_map,
            create_unknown: req.create_unknown.unwrap_or(false),
        },
        status: IdentityProviderStatus::default(),
    })?;
    Ok((StatusCode::CREATED, Json(provider)).into_response())
}

/// `GET /identity/providers`.
pub async fn list(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    ListParams(q): ListParams,
) -> ApiResult<Json<Page<IdentityProvider>>> {
    authorize(&caller, "providers.list")?;
    Ok(Json(paginate(
        st.store.collection::<IdentityProvider>().list(None)?,
        &q,
    )))
}

/// `GET /identity/providers/{id}`.
pub async fn get(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Json<IdentityProvider>> {
    authorize(&caller, "providers.get")?;
    Ok(Json(resolve(&st, &id)?))
}

/// `DELETE /identity/providers/{id}`. Cached keys go with the row.
///
/// Refused while accounts are still bound to it. Deleting the provider would
/// leave those pairs pointing at nothing: the accounts could never be signed in
/// through a provider again, and there would be no record left of which
/// provider they came from. Unlink them first, by name, while the provider is
/// still there to be named.
pub async fn remove(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    authorize(&caller, "providers.delete")?;
    let provider = resolve(&st, &id)?;
    let linked = crate::oidc_identities::for_provider(&st.store, &provider.meta.id)?;
    if !linked.is_empty() {
        return Err(ApiError::conflict(format!(
            "provider '{}' still has {} account(s) linked to it; unlink them first",
            provider.meta.name,
            linked.len()
        )));
    }
    oidc_discovery::evict(&provider);
    st.store
        .collection::<IdentityProvider>()
        .delete(&provider.meta.id)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// A cloud-admin permission. Every provider action is system-scoped.
fn authorize(caller: &Caller, action: &str) -> ApiResult<()> {
    policy::authorize(caller, action, None)
}

/// A provider by name or id. Shared with the sign-in handler.
pub fn resolve(st: &AppState, name_or_id: &str) -> ApiResult<IdentityProvider> {
    Ok(st
        .store
        .collection::<IdentityProvider>()
        .resolve(None, name_or_id)?)
}
