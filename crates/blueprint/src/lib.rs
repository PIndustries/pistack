// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Declarative stacks, workflows and kits.
//!
//! A stack is a PiStack resource (`Resource<StackSpec, StackStatus>`): YAML
//! or JSON text in `spec.document`, plus `spec.parameters`. It is not Heat
//! HOT and not OpenTofu. OpenTofu can be a client of this API later; it is
//! not the engine inside the binary.
//!
//! This slice accepts documents and stores them. [`Client::apply_stack`]
//! re-validates the document and, when it is a YAML stream of resources
//! (a single JSON object is a one-document stream), creates `network`,
//! `subnet` and `machine` rows in the same store. Unknown kinds are
//! skipped. `status.phase` stays `accepted`. `status.created` lists the
//! ids that apply inserted.
//!
//! HTTP is mounted at `/blueprint`: `GET|POST /stacks`,
//! `GET|DELETE /stacks/{id}`, `POST /stacks/{id}/apply`.

pub mod api;
pub mod client;
pub mod types;

pub use client::Client;
pub use types::*;

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};

/// The blueprint module. Apply writes resource rows; it does not run a reconciler.
#[derive(Default)]
pub struct Blueprint;

#[async_trait]
impl Module for Blueprint {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "blueprint",
            prefix: "blueprint",
            summary: "Declarative stacks, workflows and kits",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        let store = ctx.store::<Store>();
        api::router(Client::new((*store).clone()))
    }
}
