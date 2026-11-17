// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `(provider, sub)` pair as the only thing that binds a token to an
//! account.
//!
//! An account carries at most one pair; a pair belongs to at most one account.
//! Both halves of that rule are checked here, against the accounts the store
//! holds — the pairs are few (one per external user) and are read from the
//! account records rather than a second index, so there is no index to fall out
//! of step with the accounts themselves.
//!
//! Matching on anything else is what this replaces. A name is something an
//! account may already hold for quite another reason, and an email address is
//! something the identity provider does not own: a provider that vouches for
//! `admin@evil.example` would otherwise hand out the built-in `admin`.

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};

use crate::types::{ExternalIdentity, Principal};

/// The account already bound to this provider and subject, if any.
pub fn find(store: &Store, provider: &Id, subject: &str) -> ApiResult<Option<Principal>> {
    Ok(store
        .collection::<Principal>()
        .list(None)?
        .into_iter()
        .find(|p| {
            p.spec
                .external
                .as_ref()
                .is_some_and(|e| e.is(provider, subject))
        }))
}

/// The account, other than `except`, already holding a pair.
fn held_by(
    store: &Store,
    provider: &Id,
    subject: &str,
    except: Option<&Id>,
) -> ApiResult<Option<Principal>> {
    Ok(store
        .collection::<Principal>()
        .list(None)?
        .into_iter()
        .find(|p| {
            Some(&p.meta.id) != except
                && p.spec
                    .external
                    .as_ref()
                    .is_some_and(|e| e.is(provider, subject))
        }))
}

/// The pair's own description of itself, for a refusal that names it.
fn pair(provider_name: &str, subject: &str) -> String {
    format!("provider {provider_name} subject {subject}")
}

/// Bind `principal` to the pair. Refused when the account already carries a
/// different pair, or the pair already belongs to another account.
///
/// A repeat of a link already in place is returned as it stands: the
/// administrator's intent is satisfied and nothing about the account changes.
pub fn link(
    store: &Store,
    principal: &Principal,
    provider_name: &str,
    provider_id: &Id,
    subject: &str,
) -> ApiResult<Principal> {
    if let Some(held) = &principal.spec.external {
        if held.is(provider_id, subject) {
            return Ok(principal.clone());
        }
        return Err(ApiError::conflict(format!(
            "principal {} is already linked to provider {} subject {}",
            principal.meta.name, held.provider, held.subject
        )));
    }
    if let Some(other) = held_by(store, provider_id, subject, Some(&principal.meta.id))? {
        return Err(ApiError::conflict(format!(
            "{} is already linked to principal {}",
            pair(provider_name, subject),
            other.meta.name
        )));
    }
    let wanted = ExternalIdentity {
        provider: provider_id.clone(),
        subject: subject.to_owned(),
    };
    let principals = store.collection::<Principal>();
    principals
        .modify(&principal.meta.id, |p| {
            p.spec.external = Some(wanted.clone());
            Ok(())
        })
        .map_err(ApiError::from)
}

/// Remove the account's pair. An account carrying no pair is already unlinked.
pub fn unlink(store: &Store, principal: &Principal) -> ApiResult<Principal> {
    let principals = store.collection::<Principal>();
    principals
        .modify(&principal.meta.id, |p| {
            p.spec.external = None;
            Ok(())
        })
        .map_err(ApiError::from)
}

/// Accounts bound to `provider`, so its deletion can be refused and counted.
pub fn for_provider(store: &Store, provider: &Id) -> ApiResult<Vec<Principal>> {
    Ok(store
        .collection::<Principal>()
        .list(None)?
        .into_iter()
        .filter(|p| {
            p.spec
                .external
                .as_ref()
                .is_some_and(|e| e.provider == *provider)
        })
        .collect())
}
