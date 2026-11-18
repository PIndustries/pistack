// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

use super::done_upload::DoneUpload;
use super::staged_upload::StagedUpload;

/// What one server stages, as `GET /silo/peer/uploads` returns it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Staged {
    #[serde(default)]
    pub uploads: Vec<StagedUpload>,
    /// Uploads this server knows were completed or aborted.
    #[serde(default)]
    pub done: Vec<DoneUpload>,
}
