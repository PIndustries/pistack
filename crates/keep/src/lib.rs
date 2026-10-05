// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Secrets, certificates and the cloud CA.
//!
//! What other modules and the binary use from here:
//! * [`Client`] — issue server/client certificates, get the CA bundle,
//!   verify client certificates, read secret payloads.
//! * [`tls::server_config`] / [`tls::client_config`] — rustls configs built
//!   from `keep` material, so the gateway and agents can speak (m)TLS.
//! * [`redeem_join_token`] — turn a join token into a grant and mark one use.
//! * The resource types in [`types`].
//!
//! Secret payloads and private keys are encrypted at rest with AES-256-GCM
//! under one root key ([`crypto::RootKey`]). A [`backend::KeyBackend`],
//! chosen with `PISTACK_KEEP_BACKEND`, keeps that key between restarts:
//! * `software` (default): base64 in store meta `keep/root_key`.
//! * `vault`: **partial**. Wrapped by a Vault Transit key over HTTP; only
//!   the ciphertext is stored (`keep/root_key_wrapped`). Round-trips for
//!   real and is tested against a mock Transit server; migrating an existing
//!   software key, re-wrapping after Transit key rotation and a lab run
//!   against real Vault are not done.
//! * `pkcs11`: **parked**. Refused with an error; nothing pretends to work.
//!
//! Every path that needs the key, including [`read_payload`], goes through
//! the configured backend; there is no fallback between backends.
//!
//! Count limits (`quota.rs`): `secrets` and `certificates` per project
//! through gate's project quotas, with `[modules.keep]` defaults when a
//! project names none; cloud-wide caps on live join tokens and on a
//! multi-use token's redemptions.
//!
//! HTTP lives under the `secrets` prefix; see `api.rs`.

pub mod api;
pub mod backend;
pub mod ca;
pub mod client;
pub mod crypto;
pub mod error;
pub mod join;
pub mod quota;
pub mod tls;
pub mod types;
pub mod vault;

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{ApiError, Id, Kind, ListQuery, Module, ModuleContext, ModuleInfo, Page};
use std::sync::OnceLock;

pub use backend::KeyBackend;
pub use ca::{Ca, Issued};
pub use client::Client;
pub use crypto::{RootKey, Sealed};
pub use error::{KeepError, Result};
pub use join::{
    authorize_join_token, create_join_token, create_join_token_with, redeem_join_token,
    NewJoinToken,
};
pub use quota::KeepConfig;
pub use tls::{PeerIdentity, Pki};
pub use types::*;

/// Decrypt a secret payload. Does not create a root key or a CA. Missing
/// key material or a missing payload is [`KeepError::NotFound`]; a backend
/// that is misconfigured or refuses is [`KeepError::Backend`].
pub fn read_payload(store: &Store, id: &Id) -> Result<Vec<u8>> {
    let Some(key) = crypto::RootKey::load_existing(store)? else {
        return Err(KeepError::NotFound("root key is not initialised".into()));
    };
    ca::open_secret(store, &key, id)
}

#[derive(Default)]
pub struct Keep {
    client: OnceLock<Client>,
}

impl Keep {
    /// Build (once) the client from the context's store.
    fn init(&self, ctx: &ModuleContext) -> Result<Client> {
        if let Some(c) = self.client.get() {
            return Ok(c.clone());
        }
        let store = ctx.store::<Store>();
        let c = Client::open(&store)?;
        let _ = self.client.set(c.clone());
        Ok(self.client.get().cloned().unwrap_or(c))
    }

    /// The client, once `router` or `start` has run.
    pub fn client(&self) -> Option<Client> {
        self.client.get().cloned()
    }
}

#[async_trait]
impl Module for Keep {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "keep",
            prefix: "secrets",
            summary: "Secrets, keys, certificates and the cloud CA",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        match self.init(&ctx) {
            Ok(client) => api::router(client),
            Err(e) => {
                tracing::error!(error = %e, "keep: cannot initialise; serving 503");
                Router::new().fallback(|| async {
                    ApiError::unavailable("secrets service failed to initialise")
                })
            }
        }
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        self.init(&ctx)?;
        Ok(())
    }
}

/// Apply `ListQuery` filters and paging to an in-memory list. Items are
/// ordered by id (UUIDv7, so creation order); `after` is the last id seen.
pub fn paginate<K: Kind>(mut items: Vec<K>, q: &ListQuery) -> Page<K> {
    let labels = q.label_selector();
    items.retain(|k| {
        let m = k.meta();
        q.name.as_deref().is_none_or(|n| m.name == n)
            && labels.iter().all(|(lk, lv)| m.labels.get(lk) == Some(lv))
    });
    items.sort_by_key(|k| k.meta().id.to_string());
    if let Some(after) = q.after.as_deref() {
        items.retain(|k| k.meta().id.to_string().as_str() > after);
    }
    let total = items.len();
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let next = (items.len() > limit).then(|| items[limit - 1].meta().id.to_string());
    items.truncate(limit);
    Page { items, next, total }
}
