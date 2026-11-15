// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::VolumeSrc;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SandboxVolume {
    pub(in crate::runtime::youki) name: String,
    pub(in crate::runtime::youki) source: VolumeSrc,
}
