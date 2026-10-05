// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Join tokens for `pistack agent --join`.
//!
//! An admin mints a token (`POST /join-tokens`); only its SHA-256 is stored.
//! [`redeem_join_token`] checks the clear text and marks one use. A
//! single-use token then stays in the store as spent, so a second redeem is
//! unauthorized. The grant names the token record. It does not carry the
//! clear text.
//!
//! Limits ([`KeepConfig`]): at most `max_live_join_tokens` unexpired,
//! unspent tokens exist at once, and a multi-use token allows at most
//! `max_join_token_uses` redemptions (its default when none is asked for).
//! A redeem re-checks the count inside the store write, so concurrent
//! redemptions cannot overshoot `max_uses` (see `quota.rs` for the bound
//! across a leadership split).
//!
//! Expired and used-up tokens are deleted by the leader's sweep after
//! `join_token_grace_secs` (see `sweep.rs`).

use chrono::{Duration, Utc};
use pistack_store::Store;
use pistack_types::Id;

use crate::crypto::{random_token, sha256_hex};
use crate::error::{KeepError, Result};
use crate::quota::{self, KeepConfig};
use crate::types::{
    join_token_expired, join_token_max_uses, join_token_spent, JoinGrant, JoinToken, JoinTokenSpec,
};

/// Default and maximum lifetime of a join token.
pub const JOIN_TOKEN_TTL: Duration = Duration::hours(24);
/// Prefix on the clear-text token so it is recognisable in logs and configs.
pub const JOIN_TOKEN_PREFIX: &str = "pij_";

/// What to mint.
#[derive(Clone, Debug, Default)]
pub struct NewJoinToken<'a> {
    pub name: &'a str,
    pub created_by: &'a str,
    pub multi_use: bool,
    /// Redemptions a multi-use token allows; `None` takes the configured
    /// cap. Must be 1..=cap. Only valid with `multi_use`.
    pub max_uses: Option<u64>,
    /// Lifetime; `None` is [`JOIN_TOKEN_TTL`], which is also the maximum.
    pub ttl: Option<Duration>,
}

/// Mint a join token under the process's [`KeepConfig`]. Returns the
/// resource and the one-time clear text.
pub fn create_join_token(
    store: &Store,
    name: &str,
    created_by: &str,
    multi_use: bool,
    ttl: Option<Duration>,
) -> Result<(JoinToken, String)> {
    create_join_token_with(
        store,
        &NewJoinToken {
            name,
            created_by,
            multi_use,
            max_uses: None,
            ttl,
        },
        KeepConfig::configured(),
    )
}

/// Mint a join token under explicit limits.
pub fn create_join_token_with(
    store: &Store,
    req: &NewJoinToken<'_>,
    limits: &KeepConfig,
) -> Result<(JoinToken, String)> {
    let ttl = req.ttl.unwrap_or(JOIN_TOKEN_TTL);
    if ttl <= Duration::zero() || ttl > JOIN_TOKEN_TTL {
        return Err(KeepError::Invalid(format!(
            "ttl must be between 1 second and {} hours",
            JOIN_TOKEN_TTL.num_hours()
        )));
    }
    let cap = limits.max_join_token_uses;
    let max_uses = match (req.multi_use, req.max_uses) {
        (false, None) => None,
        (false, Some(_)) => {
            return Err(KeepError::Invalid(
                "max_uses applies only to a multi_use token".into(),
            ))
        }
        (true, None) => Some(cap),
        (true, Some(n)) if n == 0 || n > cap => {
            return Err(KeepError::Invalid(format!(
                "max_uses must be between 1 and {cap}"
            )))
        }
        (true, Some(n)) => Some(n),
    };
    let clear = format!("{JOIN_TOKEN_PREFIX}{}", random_token());
    let tok = JoinToken::new(
        req.name,
        None,
        JoinTokenSpec {
            hash: sha256_hex(clear.as_bytes()),
            expires_at: Utc::now() + ttl,
            multi_use: req.multi_use,
            max_uses,
            created_by: req.created_by.to_owned(),
        },
    );
    let col = store.collection::<JoinToken>();
    let _admission = quota::admission();
    let now = Utc::now();
    let live = col
        .list(None)?
        .iter()
        .filter(|t| !join_token_expired(t, now) && !join_token_spent(t))
        .count() as u64;
    if live >= limits.max_live_join_tokens {
        return Err(quota::cloud_limit(
            "join_tokens",
            live,
            limits.max_live_join_tokens,
            "delete unused tokens or let them expire",
        ));
    }
    let tok = col.create(tok)?;
    Ok((tok, clear))
}

/// Confirm a clear-text join token is known, unexpired, and not spent.
///
/// Does not write the store. The clear text is not logged and is not copied
/// into the error.
pub fn authorize_join_token(store: &Store, token: &str) -> Result<JoinToken> {
    let token = token.trim();
    if token.is_empty() {
        return Err(KeepError::Unauthorized("empty join token".into()));
    }
    let hash = sha256_hex(token.as_bytes());
    let found = store
        .collection::<JoinToken>()
        .list(None)?
        .into_iter()
        .find(|t| t.spec.hash == hash)
        .ok_or_else(|| KeepError::Unauthorized("unknown join token".into()))?;
    if join_token_expired(&found, Utc::now()) {
        return Err(KeepError::Unauthorized("join token expired".into()));
    }
    if join_token_spent(&found) {
        return Err(KeepError::Unauthorized(spent_message(&found).into()));
    }
    Ok(found)
}

/// Validate a clear-text join token and mark one use of it.
///
/// A single-use token is spent after this returns. A second call is
/// unauthorized (`join token already used`). An expired token stays expired
/// and is not marked spent. The clear text is not stored, logged, or returned.
pub fn redeem_join_token(store: &Store, token: &str) -> Result<JoinGrant> {
    let found = authorize_join_token(store, token)?;
    let now = Utc::now();
    let id: Id = found.meta.id.clone();
    let col = store.collection::<JoinToken>();
    let updated = col.modify(&id, |t| {
        // Re-check under the write. A race must not spend an expired token
        // or spend a token twice.
        if join_token_expired(t, now) {
            return Err(pistack_store::StoreError::Conflict(
                "join token expired".into(),
            ));
        }
        if join_token_spent(t) {
            return Err(pistack_store::StoreError::Conflict(spent_message(t).into()));
        }
        t.status.uses += 1;
        t.status.last_used = Some(now);
        Ok(())
    });
    let updated = match updated {
        Ok(t) => t,
        Err(pistack_store::StoreError::Conflict(m)) => return Err(KeepError::Unauthorized(m)),
        Err(e) => return Err(e.into()),
    };
    let remaining_uses =
        join_token_max_uses(&updated).map(|max| max.saturating_sub(updated.status.uses));
    Ok(JoinGrant {
        token: id,
        token_name: updated.meta.name,
        created_by: updated.spec.created_by,
        expires_at: updated.spec.expires_at,
        remaining_uses,
    })
}

fn spent_message(t: &JoinToken) -> &'static str {
    if t.spec.multi_use {
        "join token has no uses left"
    } else {
        "join token already used"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unauthorized(result: Result<JoinGrant>) -> String {
        match result {
            Err(KeepError::Unauthorized(message)) => message,
            other => panic!("expected unauthorized, got {other:?}"),
        }
    }

    #[test]
    fn single_use_token_is_spent_once() {
        let store = Store::in_memory().unwrap();
        let (tok, clear) = create_join_token(&store, "rack-a", "admin", false, None).unwrap();
        assert!(clear.starts_with(JOIN_TOKEN_PREFIX));
        assert_ne!(tok.spec.hash, clear);
        let grant = redeem_join_token(&store, &clear).unwrap();
        assert_eq!(grant.token, tok.meta.id);
        assert_eq!(grant.token_name, "rack-a");
        assert_eq!(grant.remaining_uses, Some(0));
        let grant_json = serde_json::to_string(&grant).unwrap();
        assert!(!grant_json.contains(&clear));

        let stored = store.collection::<JoinToken>().get(&tok.meta.id).unwrap();
        assert_eq!(stored.status.uses, 1);
        assert!(stored.status.last_used.is_some());
        assert!(crate::join_token_spent(&stored));
        assert_eq!(stored.spec.hash, tok.spec.hash);
        let stored_json = serde_json::to_string(&stored).unwrap();
        assert!(!stored_json.contains(&clear));

        let again = unauthorized(redeem_join_token(&store, &clear));
        assert_eq!(again, "join token already used");
        assert!(!again.contains(&clear));
        let stored = store.collection::<JoinToken>().get(&tok.meta.id).unwrap();
        assert_eq!(stored.status.uses, 1);
    }

    #[test]
    fn expired_token_is_not_marked_spent() {
        let store = Store::in_memory().unwrap();
        let (tok, clear) = create_join_token(&store, "old", "admin", false, None).unwrap();
        store
            .collection::<JoinToken>()
            .modify(&tok.meta.id, |t| {
                t.spec.expires_at = Utc::now() - Duration::seconds(1);
                Ok(())
            })
            .unwrap();
        let message = unauthorized(redeem_join_token(&store, &clear));
        assert_eq!(message, "join token expired");
        assert!(!message.contains(&clear));
        let stored = store.collection::<JoinToken>().get(&tok.meta.id).unwrap();
        assert_eq!(stored.status.uses, 0);
        assert!(stored.status.last_used.is_none());

        // A token that was spent and then expires still reports expiry.
        let (spent, spent_clear) =
            create_join_token(&store, "spent", "admin", false, None).unwrap();
        redeem_join_token(&store, &spent_clear).unwrap();
        store
            .collection::<JoinToken>()
            .modify(&spent.meta.id, |t| {
                t.spec.expires_at = Utc::now() - Duration::seconds(1);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            unauthorized(redeem_join_token(&store, &spent_clear)),
            "join token expired"
        );
        assert_eq!(
            store
                .collection::<JoinToken>()
                .get(&spent.meta.id)
                .unwrap()
                .status
                .uses,
            1
        );
    }

    #[test]
    fn two_redeems_spend_a_single_use_token_once() {
        let store = Store::in_memory().unwrap();
        let (tok, clear) = create_join_token(&store, "race", "admin", false, None).unwrap();
        let left_store = store.clone();
        let right_store = store.clone();
        let left_clear = clear.clone();
        let right_clear = clear;
        let left = std::thread::spawn(move || redeem_join_token(&left_store, &left_clear));
        let right = std::thread::spawn(move || redeem_join_token(&right_store, &right_clear));
        let results = [left.join().unwrap(), right.join().unwrap()];
        let spent = results.iter().filter(|result| result.is_ok()).count();
        let denied = results
            .iter()
            .filter(|result| {
                matches!(
                    result,
                    Err(KeepError::Unauthorized(message)) if message == "join token already used"
                )
            })
            .count();
        assert_eq!(spent, 1);
        assert_eq!(denied, 1);
        assert_eq!(
            store
                .collection::<JoinToken>()
                .get(&tok.meta.id)
                .unwrap()
                .status
                .uses,
            1
        );
    }

    #[test]
    fn multi_use_and_expiry() {
        let store = Store::in_memory().unwrap();
        let (_tok, clear) = create_join_token(&store, "fleet", "admin", true, None).unwrap();
        redeem_join_token(&store, &clear).unwrap();
        let g = redeem_join_token(&store, &clear).unwrap();
        assert_eq!(
            g.remaining_uses,
            Some(KeepConfig::configured().max_join_token_uses - 2)
        );
        let col = store.collection::<JoinToken>();
        let t = col.get(&g.token).unwrap();
        assert_eq!(t.status.uses, 2);
        assert!(!crate::join_token_spent(&t));
        col.modify(&g.token, |t| {
            t.spec.expires_at = Utc::now() - Duration::seconds(1);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            unauthorized(redeem_join_token(&store, &clear)),
            "join token expired"
        );
        assert_eq!(col.get(&g.token).unwrap().status.uses, 2);
        assert_eq!(
            unauthorized(redeem_join_token(&store, "pij_nope")),
            "unknown join token"
        );
        assert!(create_join_token(&store, "x", "admin", true, Some(Duration::days(2))).is_err());
    }

    fn limits(live: u64, uses: u64) -> KeepConfig {
        KeepConfig {
            max_live_join_tokens: live,
            max_join_token_uses: uses,
            ..KeepConfig::default()
        }
    }

    fn mint(
        store: &Store,
        name: &str,
        multi: bool,
        max: Option<u64>,
        l: &KeepConfig,
    ) -> Result<(JoinToken, String)> {
        create_join_token_with(
            store,
            &NewJoinToken {
                name,
                created_by: "admin",
                multi_use: multi,
                max_uses: max,
                ttl: None,
            },
            l,
        )
    }

    #[test]
    fn max_uses_is_validated_against_the_cap() {
        let store = Store::in_memory().unwrap();
        let l = limits(100, 10);
        // Exactly the cap is accepted; one more is not.
        let (t, _) = mint(&store, "at-cap", true, Some(10), &l).unwrap();
        assert_eq!(t.spec.max_uses, Some(10));
        assert!(matches!(
            mint(&store, "over", true, Some(11), &l),
            Err(KeepError::Invalid(_))
        ));
        assert!(matches!(
            mint(&store, "zero", true, Some(0), &l),
            Err(KeepError::Invalid(_))
        ));
        assert!(matches!(
            mint(&store, "single", false, Some(2), &l),
            Err(KeepError::Invalid(_))
        ));
        // No max_uses on a multi-use token takes the cap.
        let (t, _) = mint(&store, "default", true, None, &l).unwrap();
        assert_eq!(t.spec.max_uses, Some(10));
        let (t, _) = mint(&store, "once", false, None, &l).unwrap();
        assert_eq!(t.spec.max_uses, None);
    }

    #[test]
    fn multi_use_token_stops_at_max_uses() {
        let store = Store::in_memory().unwrap();
        let (_t, clear) = mint(&store, "rack", true, Some(3), &limits(100, 10)).unwrap();
        for left in [2, 1, 0] {
            assert_eq!(
                redeem_join_token(&store, &clear).unwrap().remaining_uses,
                Some(left)
            );
        }
        assert_eq!(
            unauthorized(redeem_join_token(&store, &clear)),
            "join token has no uses left"
        );
        assert!(authorize_join_token(&store, &clear).is_err());
        let t = store
            .collection::<JoinToken>()
            .list(None)
            .unwrap()
            .remove(0);
        assert_eq!(t.status.uses, 3);
    }

    #[test]
    fn concurrent_redeems_cannot_overshoot_max_uses() {
        let store = Store::in_memory().unwrap();
        let (tok, clear) = mint(&store, "burst", true, Some(5), &limits(100, 10)).unwrap();
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let store = store.clone();
                let clear = clear.clone();
                std::thread::spawn(move || redeem_join_token(&store, &clear).is_ok())
            })
            .collect();
        let ok = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(ok, 5);
        let t = store.collection::<JoinToken>().get(&tok.meta.id).unwrap();
        assert_eq!(t.status.uses, 5);
    }

    #[test]
    fn records_from_before_the_cap_stay_usable_until_they_expire() {
        let store = Store::in_memory().unwrap();
        let (tok, clear) = create_join_token(&store, "old", "admin", true, None).unwrap();
        let col = store.collection::<JoinToken>();
        col.modify(&tok.meta.id, |t| {
            t.spec.max_uses = None;
            t.status.uses = 10_000;
            Ok(())
        })
        .unwrap();
        let g = redeem_join_token(&store, &clear).unwrap();
        assert_eq!(g.remaining_uses, None);
    }

    #[test]
    fn live_join_tokens_are_capped() {
        let store = Store::in_memory().unwrap();
        let l = limits(3, 10);
        let mut made = Vec::new();
        for i in 0..3 {
            made.push(mint(&store, &format!("t{i}"), false, None, &l).unwrap());
        }
        let e = mint(&store, "t3", false, None, &l).unwrap_err();
        let api: pistack_types::ApiError = e.into();
        assert_eq!(api.code, pistack_types::error::ErrorCode::QuotaExceeded);
        assert_eq!(api.details["limit"], 3);
        assert_eq!(api.details["used"], 3);

        // A spent token no longer counts...
        redeem_join_token(&store, &made[0].1).unwrap();
        mint(&store, "t3", false, None, &l).unwrap();
        assert!(mint(&store, "t4", false, None, &l).is_err());
        // ...nor does an expired one.
        store
            .collection::<JoinToken>()
            .modify(&made[1].0.meta.id, |t| {
                t.spec.expires_at = Utc::now() - Duration::seconds(1);
                Ok(())
            })
            .unwrap();
        mint(&store, "t4", false, None, &l).unwrap();
        assert!(mint(&store, "t5", false, None, &l).is_err());
    }
}
