// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

/// Body of `POST /machines/{id}/rebuild`.
#[derive(Debug, Deserialize, Default)]
pub(crate) struct RebuildBody {
    /// Image the new root disk is a copy of: name or id. Default: the
    /// machine's current image.
    #[serde(default)]
    pub(crate) image: Option<String>,
}
