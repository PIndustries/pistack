// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ServerConfig`: the `[server]` section.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// `host:port` the API listener binds.
    pub listen: String,
    /// Externally reachable base URL; empty means derive from `listen`.
    pub public_url: String,
    pub insecure_http: bool,
    pub long_poll_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8443".into(),
            public_url: String::new(),
            insecure_http: false,
            long_poll_secs: 30,
        }
    }
}
