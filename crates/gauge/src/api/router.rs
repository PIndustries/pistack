// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    middleware::{self},
    routing::get,
};

use super::create_alarm::create_alarm;
use super::delete_alarm::delete_alarm;
use super::get_alarm::get_alarm;
use super::list_alarms::list_alarms;
use super::list_samples::list_samples;
use super::metrics::metrics;
use super::record_sample::record_sample;
use super::require_caller::require_caller;
use super::update_alarm::update_alarm;

pub fn router(client: Client) -> axum::Router {
    axum::Router::new()
        .route("/samples", get(list_samples).post(record_sample))
        .route("/alarms", get(list_alarms).post(create_alarm))
        .route(
            "/alarms/{id}",
            get(get_alarm).delete(delete_alarm).put(update_alarm),
        )
        .route("/metrics", get(metrics))
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}
