// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use serde::{Deserialize, Serialize};
/// Wire view of a slab volume. Loom does not depend on the slab crate.
/// Unknown spec fields are ignored so a volume written by slab still reads.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(in crate::controllers::pod) struct SlabVolSpec {
    #[serde(default)]
    pub(in crate::controllers::pod) size_gib: u64,
}
