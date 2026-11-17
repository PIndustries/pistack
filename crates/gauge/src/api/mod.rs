// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/gauge`.
//!
//! `POST /samples` `{metric, value}` records a point and re-evaluates the
//! alarms that watch it. Names that start with `pistack_` are PiStack's own
//! and cannot be posted. `GET /samples?metric=` lists the window: all of it
//! for a cloud admin, and for anyone else what was posted plus PiStack's
//! series for their own project.
//! `GET|POST /alarms`, `GET|PUT|DELETE /alarms/{id}`. A `PUT` takes
//! `{description, labels, spec}` and keeps what it does not name.
//! `GET /metrics` is the Prometheus text for a cloud admin. A scraper uses
//! `/metrics` at the root of the API, which also takes a scrape token.
//!
//! The `Caller` comes from request extensions (set by the gateway). Without
//! one, the request is rejected.

mod create_alarm;
mod create_body;
mod create_project;
mod created;
mod delete_alarm;
mod get_alarm;
mod limits;
mod list_alarms;
mod list_samples;
mod metrics;
mod new_meta;
mod page;
mod record_sample;
mod require_caller;
mod router;
mod sample_in;
mod sample_query;
mod scope;
mod scope_query;
mod text_response;
mod update_alarm;
mod update_alarm_body;
mod visible;

pub use router::router;
pub use text_response::text_response;
