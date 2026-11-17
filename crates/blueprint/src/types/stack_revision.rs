// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `StackRevision`: one applied stack spec kept for rollback.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::stack_spec::StackSpec;

/// One applied spec. Rollback re-applies `spec` as a new revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackRevision {
    pub revision: u64,
    pub spec: StackSpec,
    pub applied_at: DateTime<Utc>,
    /// Set when a rollback produced this revision: the revision it restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<u64>,
}
