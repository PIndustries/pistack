// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Client`: the one place the balancer model is written.
//!
//! HTTP handlers, the reconciler, the health checker and other modules
//! (`loom`) go through here. Every method validates and writes `spec`;
//! nothing here touches `status` except the reconciler and health checker,
//! which live in their own files but share this handle. Deletes tell the
//! provider first (best effort) so a Deleted event, which carries only an
//! id, needs no provider-side bookkeeping.
//!
//! One concern per file: the handle and its CRUD methods in [`handle`],
//! the balancer rules in [`balancer`], the listener and route rules in
//! [`listener`], the pool rules in [`pool`], the member rules in
//! [`member`], and the project check shared by every create in
//! [`project`]. Callers see only [`Client`], as before.

mod balancer;
mod handle;
mod listener;
mod member;
mod pool;
mod project;

pub use handle::Client;
