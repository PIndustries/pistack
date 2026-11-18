// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct ConsoleTicket {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) websocket: String,
}
