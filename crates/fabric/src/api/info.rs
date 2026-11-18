// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{extract::State, Json};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Info {
    pub(crate) module: &'static str,
    pub(crate) backend: &'static str,
    pub(crate) resources: &'static [&'static str],
}

pub(crate) async fn info(State(c): State<Client>) -> Json<Info> {
    Json(Info {
        module: "fabric",
        backend: c.backend().name(),
        resources: KINDS,
    })
}
