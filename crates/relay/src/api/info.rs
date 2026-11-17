// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `GET /balancers`: what this module is, which providers it serves, and
//! which kinds it owns.

use crate::client::Client;
use crate::types::KINDS;
use axum::extract::State;
use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Info {
    module: &'static str,
    providers: Vec<&'static str>,
    resources: &'static [&'static str],
}

pub(super) async fn info(State(c): State<Client>) -> Json<Info> {
    Json(Info {
        module: "relay",
        providers: c.providers().names(),
        resources: KINDS,
    })
}
