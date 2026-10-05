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
//! under a root key stored in store meta `keep/root_key`. That is the
//! *software* backend (see `crypto.rs`); PKCS#11 and Vault are the planned
//! replacements and slot in behind [`crypto::RootKey`].
//!
//! HTTP lives under the `secrets` prefix; see `api.rs`.

pub mod api;
pub mod ca;
pub mod client;
pub mod crypto;
pub mod error;
pub mod join;
pub mod tls;
pub mod types;

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{ApiError, Kind, ListQuery, Module, ModuleContext, ModuleInfo, Page};
use std::sync::OnceLock;

pub use ca::{Ca, Issued};
pub use client::Client;
pub use crypto::{RootKey, Sealed};
pub use error::{KeepError, Result};
pub use join::{authorize_join_token, create_join_token, redeem_join_token};
pub use tls::{PeerIdentity, Pki};
pub use types::*;

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
