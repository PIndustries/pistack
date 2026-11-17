// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct RouterDoc {
    #[serde(default)]
    pub(crate) external_network: Option<String>,
    #[serde(default)]
    pub(crate) interfaces: Vec<String>,
    #[serde(default)]
    pub(crate) routes: Vec<fabric::StaticRoute>,
}
