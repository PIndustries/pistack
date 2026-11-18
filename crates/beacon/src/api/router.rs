// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::notify::Notifier;
use axum::{
    middleware::{self},
    routing::get,
};

use super::api_state::ApiState;
use super::create_recordset::create_recordset;
use super::create_zone::create_zone;
use super::delete_recordset::delete_recordset;
use super::delete_zone::delete_zone;
use super::export_zone_text::export_zone_text;
use super::get_recordset::get_recordset;
use super::get_zone::get_zone;
use super::info::info;
use super::list_recordsets::list_recordsets;
use super::list_zones::list_zones;
use super::require_caller::require_caller;
use super::update_recordset::update_recordset;
use super::update_zone::update_zone;

pub fn router(client: Client, notifier: Option<Notifier>) -> axum::Router {
    let state = ApiState { client, notifier };
    axum::Router::new()
        .route("/", get(info))
        .route("/zones", get(list_zones).post(create_zone))
        .route(
            "/zones/{id}",
            get(get_zone).put(update_zone).delete(delete_zone),
        )
        .route("/zones/{id}/export", get(export_zone_text))
        .route("/recordsets", get(list_recordsets).post(create_recordset))
        .route(
            "/recordsets/{id}",
            get(get_recordset)
                .put(update_recordset)
                .delete(delete_recordset),
        )
        .layer(middleware::from_fn(require_caller))
        .with_state(state)
}
