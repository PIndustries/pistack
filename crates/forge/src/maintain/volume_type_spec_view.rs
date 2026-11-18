// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Spec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct VolumeTypeSpecView {
    #[serde(default)]
    pub(crate) backend: String,
}

impl Spec for VolumeTypeSpecView {
    /// slab names its volume types `voltype`; there is no `kind` entry for it.
    const KIND: &'static str = "voltype";
}
