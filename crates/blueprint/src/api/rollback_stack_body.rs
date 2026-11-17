// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `RollbackStack`: the POST /stacks/{id}/rollback body.

use serde::Deserialize;

/// `POST /stacks/{id}/rollback`. An empty body means the previous revision.
#[derive(Deserialize, Default)]
pub struct RollbackStack {
    /// Revision from `status.history` to restore.
    #[serde(default)]
    pub revision: Option<u64>,
}
