// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Object storage.
//!
//! Not a full S3 server. Buckets are store resources; object bytes live on
//! local disk under `$PISTACK_DATA_DIR/silo/{bucket-id}/{key}`, and
//! `status.sha256` is the content address of those bytes.
//!
//! * [`types`] — `Bucket`, `Object`.
//! * [`Client`] — the library API.
//! * [`http`] — routes mounted at `/silo`.

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::path::PathBuf;

pub mod client;
pub mod http;
pub mod types;

pub use client::Client;
pub use types::*;

/// Object storage module. `silo::Silo` is a unit value so the gateway can
/// keep constructing it that way. The data directory is read when the
/// router is built.
#[derive(Default)]
pub struct Silo;

impl Silo {
    /// `$PISTACK_DATA_DIR/silo`, or `./data/silo` when the variable is unset.
    pub fn data_root() -> PathBuf {
        std::env::var_os("PISTACK_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./data"))
            .join("silo")
    }

    pub fn client(&self, store: Store) -> Client {
        Client::new(store, Self::data_root())
    }
}

#[async_trait]
impl Module for Silo {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "silo",
            prefix: "silo",
            summary: "Object storage, S3 API",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        let store = ctx.store::<Store>();
        http::router(Client::new((*store).clone(), Self::data_root()))
    }
}
