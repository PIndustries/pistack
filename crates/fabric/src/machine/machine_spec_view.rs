// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Spec};
use serde::{Deserialize, Serialize};

use super::rest::Rest;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) struct MachineSpecView {
    #[serde(flatten)]
    pub(crate) rest: Rest,
}

impl Spec for MachineSpecView {
    const KIND: &'static str = kind::MACHINE;
}
