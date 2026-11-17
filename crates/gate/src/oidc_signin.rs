// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `POST /identity/oidc/login`: take an ID token a client already holds, verify
//! it, and sign the matching user in — answering exactly what a password login
//! answers, and writing the same kind of audit record.
//!
//! The flow, in order: find the provider, get its keys, verify the token
//! (signature then claims), read the `sub` the token carries, find the account
//! bound to that provider and subject, refuse a disabled one, apply the group
//! map, then sign a token with the shared `issue_token`. Every failure is the
//! same answer; only the audit note says which check failed. A token naming a
//! key the cache does not hold refetches the key set once and retries
//! verification once.

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use crate::api::{issue_token, Body};
use crate::audit::SigninAddr;
use crate::audit::Subject;
use crate::oidc_account;
use crate::oidc_bindings;
use crate::oidc_discovery;
use crate::oidc_error::{note, refused, Reason};
use crate::oidc_providers;
use crate::oidc_verify;
use crate::types::{IdentityProvider, Principal};
use crate::AppState;
use pistack_store::Store;
use pistack_types::{ApiResult, Id};

/// The login body: which provider, and the ID token it issued.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub provider: String,
    pub id_token: String,
}

pub async fn login(
    State(st): State<AppState>,
    from: SigninAddr,
    Body(req): Body<LoginRequest>,
) -> Response {
    let mut subject = crate::audit::Subject::default();
    // A sign-in over OIDC is not counted against the password wall: the token
    // is not a guessable secret, as an API key is not. `from` is kept for the
    // audit record the login layer writes.
    let _ = from;
    let mut response = match sign_in(&st, &req.provider, &req.id_token, &mut subject).await {
        Ok(token) => Json(token).into_response(),
        Err(err) => err.into_response(),
    };
    response.extensions_mut().insert(subject);
    response
}

async fn sign_in(
    st: &AppState,
    provider_name: &str,
    id_token: &str,
    subject: &mut Subject,
) -> ApiResult<crate::api::TokenResponse> {
    let store = &st.store;
    let provider = match oidc_providers::resolve(st, provider_name) {
        Ok(p) => p,
        Err(_) => {
            note(subject, None, None, None, Some(Reason::UnknownProvider));
            return Err(refused());
        }
    };

    // Keys, with one refetch when the token names a key the cache does not hold.
    let mut keys = match oidc_discovery::keys_for(&provider).await {
        Ok(k) => k,
        Err(reason) => {
            note(subject, Some(&provider), None, None, Some(reason));
            return Err(refused());
        }
    };
    let token_kid = kid_of(id_token);
    let mut verified = oidc_verify::verify(id_token, &keys, &provider);
    if wants_refetch(&verified, token_kid.as_deref(), &keys) {
        keys = match oidc_discovery::refresh(&provider).await {
            Ok(k) => k,
            Err(reason) => {
                note(subject, Some(&provider), None, None, Some(reason));
                return Err(refused());
            }
        };
        verified = oidc_verify::verify(id_token, &keys, &provider);
    }
    let claims = match verified {
        Ok(c) => c,
        Err(reason) => {
            note(subject, Some(&provider), None, None, Some(reason));
            return Err(refused());
        }
    };

    // The pair the account is bound to. A token that carries no `sub` names no
    // account and is refused; it is never matched by the name claim instead.
    let sub = match claims.subject() {
        Some(s) => s.to_owned(),
        None => {
            note(
                subject,
                Some(&provider),
                None,
                None,
                Some(Reason::NoSubject),
            );
            return Err(refused());
        }
    };
    // The name claim's value, for the new account's label and email field. A
    // provider configured with a name claim it does not send can still sign in
    // an account already bound to its subject, so this is not fatal.
    let raw = oidc_bindings::raw_subject(&claims, &provider).ok();
    note(subject, Some(&provider), Some(&sub), None, None);

    let principal = match oidc_account::find_or_create(
        store,
        &provider,
        &sub,
        raw.clone(),
        email_for_new_account(&provider, raw.as_deref()),
    ) {
        Ok(m) => m,
        Err(reason) => {
            note(subject, Some(&provider), Some(&sub), None, Some(reason));
            return Err(refused());
        }
    };
    // The group map is applied before the token is minted, so a newly granted
    // role is in the token the user leaves with.
    let groups = oidc_bindings::groups_in(&claims, &provider);
    oidc_bindings::apply(store, &provider, &principal, &groups)?;

    // `issue_token` refuses a disabled principal and a project the principal has
    // no role in, exactly as the password login does.
    let project = default_project(store, &principal)?;
    match issue_token(st, &principal, project, None, crate::token::DEFAULT_TTL) {
        Ok(token) => {
            subject.principal = Some(principal.meta.name.clone());
            subject.principal_id = Some(principal.meta.id.clone());
            subject.project = token.caller.project.clone();
            Ok(token)
        }
        // A disabled principal reaching here was disabled between the lookup and
        // the mint; a no-role project is a stale default. Name the reason.
        Err(err) => {
            note(
                subject,
                Some(&provider),
                Some(&sub),
                None,
                Some(Reason::Disabled),
            );
            Err(err)
        }
    }
}

/// The address a newly created account records, when the provider names its
/// users by email. Display on the account record and nothing more: an account is
/// never found by the address it carries, because a provider does not own the
/// addresses it asserts and an administrator may have typed any of them in.
fn email_for_new_account<'a>(provider: &IdentityProvider, raw: Option<&'a str>) -> Option<&'a str> {
    (provider.spec.name_claim == "email")
        .then_some(raw)
        .flatten()
}

/// A signature failure for a token naming a `kid` not in the set: worth one
/// refetch, in case the provider just rotated its keys.
fn wants_refetch(
    verified: &Result<crate::oidc_claims::IdTokenClaims, Reason>,
    kid: Option<&str>,
    keys: &oidc_discovery::ProviderKeys,
) -> bool {
    matches!(verified, Err(Reason::Signature)) && kid.is_some_and(|k| !keys.has_kid(k))
}

/// The project the token is scoped to: the principal's default project when
/// they have a role in it, else none — the password login's rule for a login
/// that names no project.
fn default_project(store: &Store, principal: &Principal) -> ApiResult<Option<Id>> {
    match principal.spec.default_project.clone() {
        Some(p) if crate::api::has_role_in(store, principal, &p)? => Ok(Some(p)),
        _ => Ok(None),
    }
}

/// The `kid` in a token header, when it names one. Read before verification to
/// decide whether a refetch is warranted.
fn kid_of(token: &str) -> Option<String> {
    jsonwebtoken::decode_header(token)
        .ok()
        .and_then(|h| h.kid)
        .filter(|k| !k.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oidc_claims::IdTokenClaims;
    use crate::oidc_discovery::ProviderKeys;
    use std::time::Instant;

    fn empty_keys() -> ProviderKeys {
        // A set with no keys: `has_kid` is false for everything in it.
        crate::oidc_discovery::parse_jwks_for_test(r#"{"keys":[]}"#, Instant::now())
    }

    fn claims() -> IdTokenClaims {
        serde_json::from_str(r#"{"iss":"i","sub":"s","aud":"c","exp":9}"#).unwrap()
    }

    #[test]
    fn a_missing_key_is_the_only_reason_to_refetch() {
        let keys = empty_keys();
        // Signature failure + a kid the set lacks: refetch.
        let sig = Err(Reason::Signature);
        assert!(wants_refetch(&sig, Some("new"), &keys));
        // The kid is present: nothing to refetch.
        assert!(!wants_refetch(&sig, None, &keys));
        // A claims failure never triggers a refetch.
        let claims_err = Err(Reason::Claims);
        assert!(!wants_refetch(&claims_err, Some("new"), &keys));
        // A success never triggers one.
        assert!(!wants_refetch(&Ok(claims()), Some("new"), &keys));
    }
}
