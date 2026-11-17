// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What a token's claims say about the account, once the token has been
//! verified: the name claim's value, the account name a new account takes from
//! it, and the group map applied at sign-in.
//!
//! Nothing here chooses an account. The `(provider, sub)` pair does that, in
//! `oidc_account`. What this module produces is a label a human reads and a set
//! of role bindings — neither of which a token can use to reach an account it
//! was not issued for.
//!
//! A group named in the token and present in the provider's map grants its
//! bindings. Only bindings the principal does not already hold are added:
//! existing bindings, however they were made, are left exactly as they are.
//! This step never removes a binding. A group that maps to a role or a project
//! the cloud does not have is skipped rather than failing the sign-in — a
//! provider that renames a group must not lock a user out.

use crate::types::{
    GroupBinding, IdentityProvider, Principal, Role, RoleBinding, RoleBindingSpec,
    RoleBindingStatus, Scope, KIND_BINDING,
};
use crate::TryGetByName;
use pistack_store::Store;
use pistack_types::{ApiResult, Meta};

/// The groups a token names, by the provider's configured claim. Empty when no
/// claim is configured.
pub fn groups_in(
    claims: &crate::oidc_claims::IdTokenClaims,
    provider: &IdentityProvider,
) -> Vec<String> {
    match provider.spec.groups_claim.as_deref() {
        Some(claim) => claims.groups(claim),
        None => Vec::new(),
    }
}

/// The next free account name after `base`, walking the suffixes a held name
/// forces: `alice`, `alice-2`, `alice-3`. `None` when `base` is not a name the
/// store would accept, which no suffix can fix.
///
/// A name is a label a human reads, never a key: whichever one comes out, the
/// account carrying it is bound to one `(provider, sub)` pair and is only ever
/// reached through that pair. A held name is walked past, never taken over.
pub fn free_name(store: &Store, base: &str) -> Option<String> {
    if base.is_empty() || crate::api::validate_name(base).is_err() {
        return None;
    }
    let held = |name: &str| match crate::principal_named(store, name) {
        // A store fault reads as "held", so nothing is created on top of a name
        // the store may be about to hand to somebody else.
        Ok(p) => p.is_some(),
        Err(_) => true,
    };
    // A name is 63 characters at most, so a suffix has to fit alongside the part
    // it follows; shortening the base is what leaves room for it.
    let stem = &base[..base.len().min(60)];
    let mut name = base.to_owned();
    for n in 2.. {
        if !held(&name) {
            return Some(name);
        }
        name = format!("{stem}-{n}");
    }
    unreachable!("a suffix past every held name is free");
}

/// The name a new account takes for `raw`: the claim's value cleaned to a legal
/// name, with the next free suffix when that name is held. A value with nothing
/// legal in it yields no name, and then no account is created — which beats
/// naming an account for nothing.
pub fn account_name(store: &Store, raw: &str) -> Option<String> {
    free_name(store, &crate::oidc_verify::account_name(raw))
}

/// The name claim's value exactly as the token carries it. For an `email`
/// claim this is the whole address, kept for the created account's email field.
/// It is display only: it never chooses an account.
pub fn raw_subject(
    claims: &crate::oidc_claims::IdTokenClaims,
    provider: &IdentityProvider,
) -> ApiResult<String> {
    crate::oidc_verify::subject_name(claims, &provider.spec.name_claim)
        .map_err(|_| pistack_types::ApiError::unauthorized("the identity token was not accepted"))
}

/// One binding a group map asks for, resolved to a store scope.
struct Wanted {
    role: String,
    scope: Scope,
    project_name: Option<String>,
}

fn holds(bindings: &[RoleBinding], principal: &Principal, want: &Wanted) -> bool {
    bindings.iter().any(|b| {
        b.spec.principal == principal.meta.id
            && b.spec.role == want.role
            && b.spec.scope == want.scope
    })
}

/// Resolve the role and project a map entry names. A role absent from the
/// cloud, or a project scope naming no project, is skipped (returns `None`).
fn resolve(store: &Store, gb: &GroupBinding) -> ApiResult<Option<Wanted>> {
    let role: Role = match store.collection::<Role>().try_get_by_name(&gb.role)? {
        Some(r) => r,
        None => return Ok(None),
    };
    match gb.project.as_deref() {
        Some(name) => match crate::resolve_project(store, name) {
            Ok(p) => Ok(Some(Wanted {
                role: role.meta.name,
                scope: Scope::Project { project: p.meta.id },
                project_name: Some(p.meta.name),
            })),
            // A bad project name is a store error turned into a skip: a group
            // must not fail the sign-in.
            Err(_) => Ok(None),
        },
        None => Ok(Some(Wanted {
            role: role.meta.name,
            scope: Scope::System,
            project_name: None,
        })),
    }
}

/// Add the bindings the token's groups grant, skipping any the principal
/// already has or that repeat within this token. Returns what was created.
pub fn apply(
    store: &Store,
    provider: &IdentityProvider,
    principal: &Principal,
    groups: &[String],
) -> ApiResult<Vec<RoleBinding>> {
    if groups.is_empty() || provider.spec.group_map.is_empty() {
        return Ok(Vec::new());
    }
    let existing = store.collection::<RoleBinding>().list(None)?;
    let mut created = Vec::new();
    let mut added: Vec<Wanted> = Vec::new();
    for group in groups {
        let Some(mapped) = provider.spec.group_map.get(group) else {
            continue;
        };
        for gb in mapped {
            let Some(want) = resolve(store, gb)? else {
                continue;
            };
            if holds(&existing, principal, &want)
                || added
                    .iter()
                    .any(|a| a.role == want.role && a.scope == want.scope)
            {
                continue;
            }
            let name = crate::binding_name(
                &principal.meta.name,
                &want.role,
                &want.scope,
                want.project_name.as_deref(),
            );
            let binding = store.collection::<RoleBinding>().create(RoleBinding {
                meta: Meta::new(KIND_BINDING, name, want.scope.project().cloned()),
                spec: RoleBindingSpec {
                    principal: principal.meta.id.clone(),
                    role: want.role.clone(),
                    scope: want.scope.clone(),
                },
                status: RoleBindingStatus::default(),
            })?;
            added.push(want);
            created.push(binding);
        }
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oidc_claims::IdTokenClaims;
    use crate::types::*;
    use pistack_types::kind;
    use std::collections::BTreeMap;

    fn provider_with(groups: BTreeMap<String, Vec<GroupBinding>>) -> IdentityProvider {
        IdentityProvider {
            meta: Meta::new(KIND_PROVIDER, "corp", None),
            spec: IdentityProviderSpec {
                issuer: "https://id.example".into(),
                client_id: "cid".into(),
                name_claim: "email".into(),
                groups_claim: Some("groups".into()),
                group_map: groups,
                create_unknown: false,
            },
            status: IdentityProviderStatus::default(),
        }
    }

    fn seed(name: &str) -> (Store, Principal) {
        let store = Store::in_memory().unwrap();
        crate::bootstrap(&store).unwrap();
        store
            .collection::<Project>()
            .create(Project {
                meta: Meta::new(kind::PROJECT, name, None),
                spec: ProjectSpec::default(),
                status: ProjectStatus::default(),
            })
            .unwrap();
        let alice = store
            .collection::<Principal>()
            .create(Principal {
                meta: Meta::new(kind::PRINCIPAL, "alice", None),
                spec: PrincipalSpec::default(),
                status: PrincipalStatus::default(),
            })
            .unwrap();
        (store, alice)
    }

    fn devs() -> BTreeMap<String, Vec<GroupBinding>> {
        let mut map = BTreeMap::new();
        map.insert(
            "devs".to_string(),
            vec![GroupBinding {
                role: "member".into(),
                project: Some("web".into()),
            }],
        );
        map
    }

    #[test]
    fn a_group_adds_only_the_binding_it_names() {
        let (store, alice) = seed("web");
        let before = store.collection::<RoleBinding>().list(None).unwrap().len();
        let created = apply(&store, &provider_with(devs()), &alice, &["devs".into()]).unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].spec.role, "member");
        assert_eq!(
            store.collection::<RoleBinding>().list(None).unwrap().len(),
            before + 1
        );
    }

    #[test]
    fn a_group_repeated_or_already_held_adds_nothing() {
        let (store, alice) = seed("web");
        let provider = provider_with(devs());
        apply(&store, &provider, &alice, &["devs".into()]).unwrap();
        let after = store.collection::<RoleBinding>().list(None).unwrap().len();
        // The same group again, and it named twice in one token, add nothing.
        apply(&store, &provider, &alice, &["devs".into()]).unwrap();
        apply(&store, &provider, &alice, &["devs".into(), "devs".into()]).unwrap();
        assert_eq!(
            store.collection::<RoleBinding>().list(None).unwrap().len(),
            after
        );
    }

    #[test]
    fn an_unknown_role_or_project_is_skipped() {
        let (store, alice) = seed("web");
        let before = store.collection::<RoleBinding>().list(None).unwrap().len();
        let mut map = BTreeMap::new();
        map.insert(
            "ghosts".to_string(),
            vec![
                GroupBinding {
                    role: "no-such-role".into(),
                    project: Some("web".into()),
                },
                GroupBinding {
                    role: "member".into(),
                    project: Some("no-such-project".into()),
                },
            ],
        );
        let created = apply(&store, &provider_with(map), &alice, &["ghosts".into()]).unwrap();
        assert!(created.is_empty());
        assert_eq!(
            store.collection::<RoleBinding>().list(None).unwrap().len(),
            before
        );
    }

    fn claims(text: &str) -> IdTokenClaims {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn groups_are_read_from_the_configured_claim() {
        let mut p = provider_with(BTreeMap::new());
        let c = claims(r#"{"iss":"i","sub":"s","aud":"c","exp":9,"groups":["a","b"]}"#);
        assert_eq!(groups_in(&c, &p), ["a".to_string(), "b".to_string()]);
        p.spec.groups_claim = None;
        assert!(groups_in(&c, &p).is_empty());
    }

    fn provider() -> IdentityProvider {
        provider_with(BTreeMap::new())
    }

    /// A store holding accounts with these names: what a name is chosen against.
    fn store_with(held: &[&str]) -> pistack_store::Store {
        let store = pistack_store::Store::in_memory().unwrap();
        {
            let col = store.collection::<Principal>();
            for n in held {
                col.create(Principal {
                    meta: Meta::new(kind::PRINCIPAL, *n, None),
                    spec: PrincipalSpec::default(),
                    status: PrincipalStatus::default(),
                })
                .unwrap();
            }
        }
        store
    }

    fn empty_store() -> pistack_store::Store {
        store_with(&[])
    }

    fn name_in(store: &pistack_store::Store, raw: &str) -> String {
        account_name(store, raw).unwrap()
    }

    #[test]
    fn a_name_claim_value_becomes_a_clean_label_and_keeps_the_address() {
        let p = provider();
        let c = claims(r#"{"iss":"i","sub":"s","aud":"c","exp":9,"email":"Alice@Ex.com"}"#);
        // The label drops the `@` (which a name may not carry); the raw subject
        // keeps the address whole, for the created account's email field alone.
        let store = empty_store();
        assert_eq!(name_in(&store, "Alice@Ex.com"), "alice");
        assert_eq!(raw_subject(&c, &p).unwrap(), "Alice@Ex.com");
        let mut p2 = provider();
        p2.spec.name_claim = "preferred_username".into();
        let c2 = claims(r#"{"iss":"i","sub":"s","aud":"c","exp":9,"preferred_username":"Al ice"}"#);
        assert_eq!(raw_subject(&c2, &p2).unwrap(), "Al ice");
    }

    #[test]
    fn a_held_name_is_walked_past_never_taken() {
        let store = store_with(&["alice", "alice-2", "admin"]);
        // Neither of the taken names is reused or attached to: the new account
        // gets the next free suffix.
        assert_eq!(name_in(&store, "alice@x.y"), "alice-3");
        assert_eq!(name_in(&store, "admin@evil.example"), "admin-2");
    }

    #[test]
    fn a_long_name_still_fits_once_suffixed() {
        let long = "a".repeat(63);
        let store = store_with(&[long.as_str()]);
        let named = account_name(&store, &format!("{long}@x.y")).unwrap();
        assert!(named.len() <= 63, "{named}");
        assert_eq!(named, format!("{}-2", "a".repeat(60)));
    }

    #[test]
    fn a_claim_value_with_nothing_legal_in_it_names_no_one() {
        // Nothing usable survives the cleaning, so no account is created rather
        // than one made under a label nobody asked for.
        let store = empty_store();
        assert!(account_name(&store, "!!!@x").is_none());
        assert!(account_name(&store, "@x").is_none());
    }

    #[test]
    fn a_missing_name_claim_is_the_uniform_refusal() {
        let p = provider();
        let c = claims(r#"{"iss":"i","sub":"s","aud":"c","exp":9}"#);
        let e = raw_subject(&c, &p).unwrap_err();
        assert_eq!(e.message, "the identity token was not accepted");
    }
}
