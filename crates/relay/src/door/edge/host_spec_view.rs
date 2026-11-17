// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The part of a `Host` spec the edge path reads: its node name and drivers.

use pistack_types::{kind, Spec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct HostSpecView {
    #[serde(default)]
    pub(crate) node_name: String,
    #[serde(default)]
    pub(crate) drivers: Vec<String>,
}

impl Spec for HostSpecView {
    const KIND: &'static str = kind::HOST;
}
