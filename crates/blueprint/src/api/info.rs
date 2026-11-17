// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Info`: the GET / discovery document.

use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Info {
    module: &'static str,
    resources: &'static [&'static str],
}

pub(super) async fn info() -> Json<Info> {
    Json(Info {
        module: "blueprint",
        resources: &["stacks", "kits", "workflows"],
    })
}
