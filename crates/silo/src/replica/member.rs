// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

/// One row of the cluster membership list in store meta.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Member {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) api: String,
    /// The server's TLS listener, when it has one.
    #[serde(default)]
    pub(crate) agent_api: Option<String>,
}
