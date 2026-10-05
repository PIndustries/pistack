// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Declarative stacks, workflows and kits.
//!
//! A stack is a PiStack resource (`Resource<StackSpec, StackStatus>`): YAML
//! or JSON text in `spec.document`, plus `spec.parameters`.
//!
//! [`Client::apply_stack`] re-validates the document and, when it is a YAML
//! stream of resources (a single JSON object is a one-document stream),
//! creates `network`, `subnet`, `machine`, `router`, `pod`, `listener`, and
//! record set rows in the same store. A row an earlier apply of the same
//! stack created (`status.managed`) is updated to the document's spec
//! through its module's own update rules. Any other name that already
//! exists in the project is left in place. Unknown kinds are skipped.
//! `status.phase` is `applying` while rows are written, then `ready` or
//! `failed`. `status.created` and `status.updated` list what that apply
//! changed.
//!
//! [`Client::update_stack`] stores a new spec. Each successful apply of a
//! new spec is kept in `status.history` (the newest
//! [`REVISION_HISTORY_LIMIT`] revisions). [`Client::rollback_stack`] puts
//! an earlier revision's spec back and applies it.
//!
//! A kit is a stored document. [`Client::apply_kit`] runs that same path
//! without history. A workflow is an ordered list of stack names in one
//! project. [`Client::apply_workflow`] applies those stacks in order and
//! stops on the first error.
//!
//! HTTP is mounted at `/blueprint`. Stacks, kits, and workflows each have
//! `GET|POST`, `GET|DELETE /{id}`, and `POST /{id}/apply`. Stacks also have
//! `PUT /{id}` and `POST /{id}/rollback`.

pub mod api;
pub mod client;
pub mod types;

pub use client::Client;
pub use types::*;

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};

/// The blueprint module. Apply writes and updates resource rows; it does not
/// run a reconciler.
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
