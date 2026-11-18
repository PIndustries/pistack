// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The names a `crud!` expansion needs in scope.
//!
//! `crud!` is declared in `mod.rs` and expands in the modules that invoke it,
//! so its body resolves names where each call sits rather than where the
//! macro lives. Before the split those names shared one file with the calls.
//! This module re-exports exactly that set, so a caller can `use` it whole
//! instead of repeating the list above every `crud!`.
//!
//! It is a view over the real declarations, which stay in their own files:
//! nothing here defines anything.

pub(crate) use super::apply_meta::apply_meta;
pub(crate) use super::create::Create;
pub(crate) use super::create_project::create_project;
pub(crate) use super::created::created;
pub(crate) use super::guarded::Guarded;
pub(crate) use super::if_match::if_match;
pub(crate) use super::in_which::InWhich;
pub(crate) use super::net_filter::NetFilter;
pub(crate) use super::new_meta::new_meta;
pub(crate) use super::no_vpc::no_vpc;
pub(crate) use super::page_by::page_by;
pub(crate) use super::scope::scope;
pub(crate) use super::scope_query::ScopeQuery;
pub(crate) use super::update::Update;
pub(crate) use super::visible::visible;
pub(crate) use crate::client::Client;
pub(crate) use axum::extract::{Extension, Path, Query, State};
pub(crate) use axum::http::HeaderMap;
pub(crate) use axum::http::StatusCode;
pub(crate) use axum::response::Response;
pub(crate) use axum::Json;
pub(crate) use pistack_types::ApiResult;
pub(crate) use pistack_types::Caller;
pub(crate) use pistack_types::ListQuery;
pub(crate) use pistack_types::Page;
