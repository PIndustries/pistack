// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resources;
use serde::{Deserialize, Serialize};

/// A flavor. The name is the handle users pass (`--size m.2x4`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SizeSpec {
    #[serde(flatten)]
    pub resources: Resources,
}
