// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/balancers`.
//!
//! `GET|POST /balancers`, `GET|PUT|DELETE /balancers/{name-or-id}`,
//! the same for `/listeners` and `/pools`, members as a sub-resource
//! `GET|POST /pools/{id}/members`, `GET|PUT|DELETE /pools/{id}/members/{id}`,
//! and a flat `GET /members`. Handlers only validate the caller's scope and
//! hand off to `Client`; they never write `status`.
//!
//! The `Caller` comes from request extensions (set by the gateway).
//! Without one, the request is rejected.
//!
//! One file per thing the surface needs: [`router`] wires the routes,
//! [`require_caller`] is the gate in front of them, [`info`] answers `/`,
//! [`body`] the shapes a caller sends, [`scope`] and [`visible`] who may
//! see what, [`if_match`] the revision header, [`name`] the name rule,
//! [`meta`] the metadata a body writes, [`page`] the list window,
//! [`created`] the 201 reply, [`crud`] the handler set every kind shares,
//! and [`member`] the sub-resource that does not use it.

mod body;
mod created;
mod if_match;
mod meta;
mod name;
mod page;
mod scope;
mod visible;
// Declares the `crud!` macro the kinds below expand, so it is declared
// before them.
#[macro_use]
mod crud;
mod create_member;
mod delete_member;
mod get_member;
mod info;
mod list_members;
mod list_pool_members;
mod member_scope;
mod require_caller;
mod router;
mod update_member;

pub use router::router;
