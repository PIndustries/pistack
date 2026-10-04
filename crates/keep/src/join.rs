// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Join tokens for `pistack agent --join`.
//!
//! An admin mints a token (`POST /join-tokens`); only its SHA-256 is stored.
//! [`redeem_join_token`] checks the clear text and marks one use. A
//! single-use token then stays in the store as spent, so a second redeem is
//! unauthorized. The grant names the token record. It does not carry the
//! clear text.

use chrono::{Duration, Utc};
use pistack_store::Store;
use pistack_types::Id;

use crate::crypto::{random_token, sha256_hex};
use crate::error::{KeepError, Result};
use crate::types::{join_token_expired, join_token_spent, JoinGrant, JoinToken, JoinTokenSpec};

/// Default and maximum lifetime of a join token.
pub const JOIN_TOKEN_TTL: Duration = Duration::hours(24);
/// Prefix on the clear-text token so it is recognisable in logs and configs.
pub const JOIN_TOKEN_PREFIX: &str = "pij_";

/// Mint a join token. Returns the resource and the one-time clear text.
pub fn create_join_token(
    store: &Store,
    name: &str,
    created_by: &str,
    multi_use: bool,
    ttl: Option<Duration>,
) -> Result<(JoinToken, String)> {
    let ttl = ttl.unwrap_or(JOIN_TOKEN_TTL);
    if ttl <= Duration::zero() || ttl > JOIN_TOKEN_TTL {
        return Err(KeepError::Invalid(format!(
            "ttl must be between 1 second and {} hours",
            JOIN_TOKEN_TTL.num_hours()
        )));
    }
    let clear = format!("{JOIN_TOKEN_PREFIX}{}", random_token());
    let tok = JoinToken::new(
        name,
        None,
        JoinTokenSpec {
            hash: sha256_hex(clear.as_bytes()),
            expires_at: Utc::now() + ttl,
            multi_use,
            created_by: created_by.to_owned(),
        },
    );
    let tok = store.collection::<JoinToken>().create(tok)?;
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
        return Err(KeepError::Unauthorized("join token already used".into()));
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
            return Err(pistack_store::StoreError::Conflict(
                "join token already used".into(),
            ));
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
    Ok(JoinGrant {
        token: id,
        token_name: updated.meta.name,
        created_by: updated.spec.created_by,
        expires_at: updated.spec.expires_at,
        remaining_uses: (!updated.spec.multi_use).then_some(0),
    })
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
        assert_eq!(g.remaining_uses, None);
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
}
