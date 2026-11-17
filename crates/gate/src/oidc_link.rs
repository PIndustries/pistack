// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `POST /identity/identities` and `POST /identity/identities/remove`: an
//! administrator putting an existing account under an external identity, and
//! taking it off again. Both take the pair in the body — the second names the
//! pair being released rather than a user, so the record says which identity was
//! let go of, which is the thing worth having in the log.
//!
//! This is the only way an existing account becomes reachable by an OIDC
//! sign-in. Sign-in itself never adopts one: a name is a label an account may
//! hold for any reason, and an email address is something the provider does not
//! own, so neither can be evidence that a token belongs to somebody already in
//! the cloud. The administrator, who can see both sides, is the one who says so
//! — and both directions of the change are audited, since one of them grants
//! outside-vouched access to an account that already has roles.
//!
//! Cloud admins only, like the provider records these point at.

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use crate::api::Body;
use crate::oidc_identities;
use crate::oidc_providers;
use crate::token::AuthCaller;
use crate::types::Principal;
use crate::AppState;
use pistack_types::{ApiError, ApiResult, Caller};

/// The pair an administrator is putting an account under, or releasing from it.
#[derive(Debug, Deserialize)]
pub struct IdentityRequest {
    /// Principal name or id. Only the link names one: the unlink finds the
    /// account from the pair being let go of.
    #[serde(default)]
    pub user: Option<String>,
    /// Provider name or id.
    pub provider: String,
    /// The `sub` the provider's tokens carry for this person.
    pub subject: String,
}

impl IdentityRequest {
    /// The subject, trimmed. Refused when nothing is left: the pair is what an
    /// account is bound to, so an empty half of it binds nothing.
    fn subject(&self) -> ApiResult<&str> {
        let subject = self.subject.trim();
        if subject.is_empty() {
            return Err(ApiError::bad_request("subject is required"));
        }
        Ok(subject)
    }
}

/// The account a link request names, by name or id.
fn resolve_principal(st: &AppState, name_or_id: &str) -> ApiResult<Principal> {
    Ok(st
        .store
        .collection::<Principal>()
        .resolve(None, name_or_id)?)
}

/// `POST /identity/identities`.
pub async fn link(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Body(req): Body<IdentityRequest>,
) -> ApiResult<Response> {
    authorize(&caller, "identities.link")?;
    let subject = req.subject()?;
    let user = req
        .user
        .as_deref()
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("user is required"))?;
    let provider = oidc_providers::resolve(&st, &req.provider)?;
    let principal = resolve_principal(&st, user)?;
    let linked = oidc_identities::link(
        &st.store,
        &principal,
        &provider.meta.name,
        &provider.meta.id,
        subject,
    )?;
    let mut response = Json(linked.redacted()).into_response();
    response.extensions_mut().insert(crate::audit_note::noted(&[
        ("action", "link external identity"),
        ("provider", provider.meta.name.as_str()),
        ("subject", subject),
        ("user", linked.meta.name.as_str()),
    ]));
    Ok(response)
}

/// `POST /identity/identities/remove`.
pub async fn unlink(
    State(st): State<AppState>,
    AuthCaller(caller): AuthCaller,
    Body(req): Body<IdentityRequest>,
) -> ApiResult<Response> {
    authorize(&caller, "identities.unlink")?;
    let subject = req.subject()?;
    let provider = oidc_providers::resolve(&st, &req.provider)?;
    let holder = oidc_identities::find(&st.store, &provider.meta.id, subject)?;
    let unlinked = match holder {
        Some(p) => oidc_identities::unlink(&st.store, &p)?,
        // Nobody holds the pair. Answered as the no-op it is rather than as a
        // missing resource, so a retried unlink does not look like a failure. The
        // answer is the redacted account shape with the pair left off, not an
        // account: there is no account this changed anything about.
        None => {
            return Ok(
                Json(serde_json::json!({"external": null, "unlinked": false})).into_response(),
            )
        }
    };
    let mut response = Json(unlinked.redacted()).into_response();
    response.extensions_mut().insert(crate::audit_note::noted(&[
        ("action", "unlink external identity"),
        ("provider", provider.meta.name.as_str()),
        ("subject", subject),
        ("user", unlinked.meta.name.as_str()),
    ]));
    Ok(response)
}

/// A cloud-admin permission, like a provider's.
fn authorize(caller: &Caller, action: &str) -> ApiResult<()> {
    crate::policy::authorize(caller, action, None)
}
