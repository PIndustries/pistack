// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::SandboxMount;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Serialize, Deserialize)]
pub struct PlannedMounts {
    pub(in crate::runtime::youki) index: usize,
    pub(in crate::runtime::youki) mounts: Vec<SandboxMount>,
}
