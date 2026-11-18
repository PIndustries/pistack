// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Spec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BucketSpec {
    /// When true, a put keeps the previous generation's bytes and record.
    /// The key path always serves the latest bytes either way.
    #[serde(default)]
    pub versioning: bool,
}

impl Spec for BucketSpec {
    const KIND: &'static str = kind::BUCKET;
}
