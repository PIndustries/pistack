// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Spec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) struct VolumeSpecView {}

impl Spec for VolumeSpecView {
    const KIND: &'static str = kind::VOLUME;
}
