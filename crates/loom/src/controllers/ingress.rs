// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Ingress controller. Each ingress is one HTTP listener and one pool of
//! its backend endpoints. A missing backend stays unavailable. Delete
//! removes that listener.

use super::{delete_quietly, patch_status, Controller};
use crate::realizer::RelayRealizer;
use crate::types::{delete_requested, set_condition, Ingress};
use pistack_store::{Collection, Store, StoreError};
use std::sync::Arc;

pub struct IngressController {
    store: Store,
    relay: Option<Arc<RelayRealizer>>,
}

impl IngressController {
    pub fn new(store: Store, relay: Option<Arc<RelayRealizer>>) -> Self {
        Self { store, relay }
    }

    fn ingresses(&self) -> Collection<Ingress> {
        self.store.collection()
    }

    async fn mark(&self, ing: &Ingress, addrs: &[String], ok: bool, message: &str) {
        let id = ing.meta.id.clone();
        let addrs = addrs.to_vec();
        let message = message.to_owned();
        let patched = patch_status(&self.ingresses(), &id, |status| {
            if ok {
                status.load_balancer = addrs.clone();
                set_condition(
                    &mut status.conditions,
                    "Ready",
                    true,
                    "Programmed",
                    &message,
                );
            } else {
                status.load_balancer.clear();
                set_condition(
                    &mut status.conditions,
                    "Ready",
                    false,
                    "Unavailable",
                    &message,
                );
            }
        });
        if let Err(e) = patched {
            if !matches!(e, StoreError::NotFound(_)) {
                tracing::warn!(ingress = %id, error = %e, "loom: ingress status write failed");
            }
        }
    }
}

#[async_trait::async_trait]
impl Controller for IngressController {
    fn name(&self) -> &'static str {
        "ingress"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[crate::types::kinds::INGRESS, crate::types::kinds::SERVICE]
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let Some(relay) = &self.relay else {
            for ing in self.ingresses().list(None)? {
                if delete_requested(&ing) {
                    delete_quietly(&self.ingresses(), &ing.meta.id)?;
                    continue;
                }
                self.mark(&ing, &[], false, "ingress relay is not configured")
                    .await;
            }
            return Ok(());
        };
        if let Err(e) = relay.gc_ingresses().await {
            tracing::warn!(error = %e, "loom: ingress listener gc failed");
        }
        for ing in self.ingresses().list(None)? {
            if delete_requested(&ing) {
                if let Err(e) = relay.remove_ingress(&ing).await {
                    tracing::warn!(ingress = %ing.meta.id, error = %e, "loom: ingress remove failed");
                }
                delete_quietly(&self.ingresses(), &ing.meta.id)?;
                continue;
            }
            match relay.ensure_ingress(&ing).await {
                Ok(addrs) => self.mark(&ing, &addrs, true, "listener programmed").await,
                Err(e) => self.mark(&ing, &[], false, &e.message).await,
            }
        }
        Ok(())
    }
}
