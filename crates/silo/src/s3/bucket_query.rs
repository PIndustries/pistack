// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

/// Query fields of the bucket path that choose a subresource rather than a
/// list. `ListQuery` carries them too, for `GET /{bucket}`.
#[derive(Debug, Deserialize, Default)]
pub struct BucketQuery {
    /// Presence of `versioning` (even empty) means the bucket's versioning
    /// configuration.
    pub versioning: Option<String>,
}
