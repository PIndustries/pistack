// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::ingress_backend::IngressBackend;
use super::path_type::PathType;
use super::root::root;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngressPath {
    #[serde(default = "root")]
    pub path: String,
    #[serde(default)]
    pub path_type: PathType,
    pub backend: IngressBackend,
}
