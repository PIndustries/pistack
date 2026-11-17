// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Choosing which account a verified token signs in to.
//!
//! The token's `(provider, sub)` pair, and nothing else. An account bound to
//! that pair is the account. With no account bound to it, a provider that
//! creates unknown users gets a new one bound to the pair from the moment it
//! exists.
//!
//! The name a new account takes comes from the provider's configured name
//! claim, so it is a name a human can read. It is never a lookup and never a
//! claim on somebody else's account: when the name is already held by any
//! account, the new one takes `name-2`, `name-3`, and so on. Sign-in therefore
//! cannot hand out an account the token does not name, which is what matching a
//! subject by name or by email address used to do.
//!
//! A created account is bound to the pair at the moment it is created. An
//! account the cloud already had is only ever reached through an administrator's
//! link — see `oidc_link`.

use crate::oidc_error::Reason;
use crate::types::{
    ExternalIdentity, IdentityProvider, Principal, PrincipalKind, PrincipalSpec, PrincipalStatus,
};
use pistack_store::Store;
use pistack_types::{kind, Meta};

/// Find the account bound to the pair, or create one when the provider allows
/// it. A service account and a disabled user are refused.
pub fn find_or_create(
    store: &Store,
    provider: &IdentityProvider,
    subject: &str,
    raw_claim: Option<String>,
    email: Option<&str>,
) -> Result<Principal, Reason> {
    let principals = store.collection::<Principal>();
    let existing = crate::oidc_identities::find(store, &provider.meta.id, subject)
        .map_err(|_| Reason::StoreFault)?;
    if let Some(p) = existing {
        if p.spec.kind != PrincipalKind::User {
            return Err(Reason::NotAUser);
        }
        if !p.spec.enabled {
            return Err(Reason::Disabled);
        }
        return Ok(p);
    }
    if !provider.spec.create_unknown {
        return Err(Reason::UnknownUser);
    }
    // The label comes from the provider's configured name claim, falling back
    // to the subject when the provider names its claim with something the token
    // does not carry. Either way it labels the account and picks nothing.
    let label = raw_claim.or(Some(subject.to_owned()));
    let name = label
        .as_deref()
        .and_then(|raw| crate::oidc_bindings::account_name(store, raw))
        .ok_or(Reason::NameTaken)?;
    // A created account is a user with no password of its own. `auth_source`
    // is what refuses password login for it; `external` is what signs it in.
    let created = principals
        .create(Principal {
            meta: Meta::new(kind::PRINCIPAL, name, None),
            spec: PrincipalSpec {
                kind: PrincipalKind::User,
                // Display only. Never read back to decide who a token is for.
                email: email.map(str::to_owned),
                password_hash: None,
                enabled: true,
                default_project: None,
                // Marks the account as provider-owned, which is what refuses a
                // password login for it. A later link of a local account leaves
                // its own marker alone, so an account with a password keeps it.
                auth_source: Some(provider.meta.name.clone()),
                external: Some(ExternalIdentity {
                    provider: provider.meta.id.clone(),
                    subject: subject.to_owned(),
                }),
            },
            status: PrincipalStatus::default(),
        })
        .map_err(|_| Reason::NameTaken)?;
    Ok(created)
}
