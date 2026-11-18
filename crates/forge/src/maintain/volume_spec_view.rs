// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Spec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct VolumeSpecView {
    #[serde(default)]
    pub(crate) volume_type: String,
}

impl Spec for VolumeSpecView {
    const KIND: &'static str = kind::VOLUME;
}
