// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IngressTls {
    #[serde(default)]
    pub hosts: Vec<String>,
    /// A `keep` certificate name (or id) in the ingress's project. When set,
    /// the ingress also serves HTTPS with that certificate; every entry on
    /// one ingress must name the same certificate.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_name: String,
}
