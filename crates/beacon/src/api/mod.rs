// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/dns`.
//!
//! `GET|POST /zones`, `GET|PUT|DELETE /zones/{name-or-id}`,
//! `GET /zones/{id}/export` (master-file text),
//! `GET|POST /recordsets`, `GET|PUT|DELETE /recordsets/{name-or-id}`.
//! Handlers only validate the caller's scope and hand off to `Client`;
//! they never write `status`.
//!
//! The `Caller` comes from request extensions (set by the gateway).
//! Without one, the request is rejected.

mod apex_of;
mod api_state;
mod apply_meta;
mod asked;
mod create;
mod create_project;
mod create_recordset;
mod create_zone;
mod created;
mod default_limit;
mod delete_recordset;
mod delete_zone;
mod export_zone_text;
mod get_recordset;
mod get_zone;
mod if_match;
mod info;
mod list_recordsets;
mod list_zones;
mod max_limit;
mod new_meta;
mod page;
mod require_caller;
mod resolve_zone;
mod router;
mod scope;
mod scope_query;
mod update;
mod update_recordset;
mod update_zone;
mod visible;
pub use api_state::ApiState;
pub use create::Create;
pub use router::router;
pub use scope_query::ScopeQuery;
pub use update::Update;
