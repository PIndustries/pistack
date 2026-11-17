// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use serde_json::Value;

use super::default_true::default_true;

#[derive(Deserialize)]
pub(crate) struct ListenerDoc {
    #[serde(default)]
    pub(crate) balancer: String,
    #[serde(default)]
    pub(crate) vip_network: String,
    #[serde(default)]
    pub(crate) provider: Option<relay::Provider>,
    pub(crate) protocol: Value,
    pub(crate) port: u16,
    #[serde(default)]
    pub(crate) connection_limit: u32,
    #[serde(default = "default_true")]
    pub(crate) admin_up: bool,
}
