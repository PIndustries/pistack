// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Join tokens for `pistack agent --join`.
//!
//! An admin mints a token (`POST /join-tokens`); only its SHA-256 is stored.
//! The agent presents the token once; [`redeem_join_token`] checks it and
//! burns it (unless multi-use). The grant tells the caller which token was
//! used so the agent's certificate request can be tied back to it.

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

/// Validate a clear-text join token and consume one use of it.
pub fn redeem_join_token(store: &Store, token: &str) -> Result<JoinGrant> {
    let token = token.trim();
    if token.is_empty() {
        return Err(KeepError::Unauthorized("empty join token".into()));
    }
    let hash = sha256_hex(token.as_bytes());
    let col = store.collection::<JoinToken>();
    let found = col
        .list(None)?
        .into_iter()
        .find(|t| t.spec.hash == hash)
        .ok_or_else(|| KeepError::Unauthorized("unknown join token".into()))?;
    let now = Utc::now();
    if join_token_expired(&found, now) {
        return Err(KeepError::Unauthorized("join token expired".into()));
    }
    if join_token_spent(&found) {
        return Err(KeepError::Unauthorized("join token already used".into()));
    }
    let id: Id = found.meta.id.clone();
    let updated = col.modify(&id, |t| {
        if join_token_spent(t) {
            // Lost a race with another redeemer; surface as conflict and map
            // to unauthorized below.
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
    // A single-use token is deleted once redeemed; nothing else can use it.
    if !updated.spec.multi_use {
        col.delete(&id)?;
    }
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

    #[test]
    fn single_use_token_burns() {
        let store = Store::in_memory().unwrap();
        let (tok, clear) = create_join_token(&store, "rack-a", "admin", false, None).unwrap();
        assert!(clear.starts_with(JOIN_TOKEN_PREFIX));
        assert_ne!(tok.spec.hash, clear);
        let grant = redeem_join_token(&store, &clear).unwrap();
        assert_eq!(grant.token, tok.meta.id);
        assert_eq!(grant.token_name, "rack-a");
        assert!(matches!(
            redeem_join_token(&store, &clear),
            Err(KeepError::Unauthorized(_))
        ));
        assert!(store
            .collection::<JoinToken>()
            .list(None)
            .unwrap()
            .is_empty());
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
        col.modify(&g.token, |t| {
            t.spec.expires_at = Utc::now() - Duration::seconds(1);
            Ok(())
        })
        .unwrap();
        assert!(redeem_join_token(&store, &clear).is_err());
        assert!(redeem_join_token(&store, "pij_nope").is_err());
        assert!(create_join_token(&store, "x", "admin", true, Some(Duration::days(2))).is_err());
    }
}
