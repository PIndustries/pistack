// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `loom` API: plain-noun CRUD over every resource plus a few verbs.
//!
//! Handlers write `spec` and `meta` only; controllers own `status`.
//! Deleting a resource that has children or a machine behind it sets
//! `DELETE_ANNOTATION` and returns `202 Accepted`; the controller tears it
//! down and removes it. ConfigMaps and Ingresses delete immediately (`204`).
//!
//! Project scoping reads `pistack_types::Caller` from request extensions
//! when the gateway's auth layer attached one. With no `Caller` the request
//! is treated as cloud admin in dev mode and rejected otherwise.

mod api;
mod app_state;
mod autoscaler_spec;
mod body;
mod col;
mod config_map_spec;
mod create;
mod cron_job_spec;
mod crud;
mod daemon_set_spec;
mod delete;
mod delete_mode;
mod deployment_spec;
mod exec_body;
mod get_one;
mod if_match;
mod index;
mod ingress_spec;
mod job_spec;
mod list;
mod load;
mod paginate;
mod parse_query;
mod percent_decode;
mod pod_exec;
mod pod_logs;
mod pod_sandbox;
mod pod_spec;
mod replica_set_spec;
mod res;
mod rollback_body;
mod rollback_deployment;
mod router;
mod scale_body;
mod scale_deployment;
mod scale_statefulset;
mod scope;
mod service_spec;
mod stateful_set_spec;
#[cfg(test)]
mod tests;
mod update;
mod validate_name;
mod validate_pod_spec;
mod validate_probe;
mod validate_selector;
pub use api::Api;
pub use app_state::AppState;
pub use body::Body;
pub use create::Create;
pub use delete_mode::DeleteMode;
pub use list::List;
pub use paginate::paginate;
pub use rollback_body::RollbackBody;
pub use router::router;
pub use scale_body::ScaleBody;
pub use scope::Scope;
pub use update::Update;
pub use validate_name::validate_name;
