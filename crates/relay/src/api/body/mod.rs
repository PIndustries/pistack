// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The shapes a caller sends. `Create` and `Update` wrap any spec; a member
//! body carries no name because the pool and address name it; `ScopeQuery`
//! is the query string every handler reads.
//!
//! One file per shape. Callers see them here, as before.

mod create;
mod member_body;
mod scope_query;
mod update;

pub(super) use create::Create;
pub(super) use member_body::MemberBody;
pub(super) use scope_query::ScopeQuery;
pub(super) use update::Update;
