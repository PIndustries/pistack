// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/blueprint`.
//!
//! `GET|POST /stacks`, `GET|PUT|DELETE /stacks/{id}`,
//! `POST /stacks/{id}/apply`, `POST /stacks/{id}/rollback`, and `GET|POST`,
//! `GET|DELETE /{id}`, `POST /{id}/apply` for `/kits` and `/workflows`.
//!
//! `PUT` stores a new spec (honouring `If-Match`); apply realizes it,
//! substituting the values given for the document's `"{{ param.NAME }}"`
//! references first. Apply re-validates the stored document, creates missing
//! network, subnet, machine, router, pod, listener, and record set rows,
//! updates the rows an earlier apply of the same stack created, and deletes
//! managed rows the document no longer names. `status.phase` is `applying`
//! while it runs, then `ready` or `failed`. Each successful stack apply of a
//! new spec is kept in `status.history`; rollback puts an earlier revision's
//! spec back and applies it. A kit is that same document without history.
//! A workflow applies named stacks in order and stops on the first error.
//! Unknown kinds are skipped.
//!
//! The `Caller` comes from request extensions (set by the gateway). Without
//! one, the request is rejected.

mod apply_kit;
mod apply_stack;
mod apply_workflow;
mod create_kit;
mod create_kit_body;
mod create_project;
mod create_stack;
mod create_stack_body;
mod create_workflow;
mod create_workflow_body;
mod created;
mod delete_kit;
mod delete_stack;
mod delete_workflow;
mod get_kit;
mod get_stack;
mod get_workflow;
mod if_match;
mod info;
mod list_kits;
mod list_project;
mod list_stacks;
mod list_workflows;
mod new_meta;
mod page;
mod require_caller;
mod rollback_stack;
mod rollback_stack_body;
mod router;
mod update_stack;
mod update_stack_body;
mod visible;

pub use router::router;
