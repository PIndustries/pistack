// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Spec};
use serde::{Deserialize, Serialize};

/// The parts of a compute host record that say whether it can carry a
/// router's gateway port. The compute module owns the record; this view
/// only reads it and must never be written back, since it drops every
/// other field.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HostView {
    #[serde(default)]
    pub node_name: String,
}

impl Spec for HostView {
    const KIND: &'static str = kind::HOST;
}
