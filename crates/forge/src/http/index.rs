// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::Json;

pub(crate) async fn index() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "module": "forge",
        "resources": ["machines", "hosts", "sizes", "keypairs"],
    }))
}
