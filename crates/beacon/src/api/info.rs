// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Info {
    pub(crate) module: &'static str,
    pub(crate) resources: &'static [&'static str],
}

pub(crate) async fn info() -> Json<Info> {
    Json(Info {
        module: "beacon",
        resources: KINDS,
    })
}
