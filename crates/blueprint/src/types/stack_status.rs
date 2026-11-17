// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `StackStatus`: a stack's observed status, and the `Stack` resource.
use serde::{Deserialize, Serialize};

use super::accepted_message::ACCEPTED_MESSAGE;
use super::stack_phase::StackPhase;
use super::stack_revision::StackRevision;
use super::stack_spec::StackSpec;
use pistack_types::Resource;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackStatus {
    pub phase: StackPhase,
    pub message: String,
    /// Ids inserted by the last apply. Already-present names are not listed.
    #[serde(default)]
    pub created: Vec<String>,
    /// Ids whose spec the last apply changed.
    #[serde(default)]
    pub updated: Vec<String>,
    /// Managed ids the last apply's document no longer named that could not
    /// be deleted (unknown kind). Successfully deleted managed ids are not
    /// listed here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left: Vec<String>,
    /// Every id an apply of this resource inserted. Later applies update
    /// these rows when the document changes; rows it did not insert are
    /// left as they are.
    #[serde(default)]
    pub managed: Vec<String>,
    /// Revision of the last successful apply. `0` before the first one.
    #[serde(default)]
    pub revision: u64,
    /// Applied specs, oldest first, at most [`REVISION_HISTORY_LIMIT`].
    /// Only stacks keep history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<StackRevision>,
}

impl StackStatus {
    pub fn accepted() -> Self {
        Self {
            phase: StackPhase::Accepted,
            message: ACCEPTED_MESSAGE.to_owned(),
            created: Vec::new(),
            updated: Vec::new(),
            left: Vec::new(),
            managed: Vec::new(),
            revision: 0,
            history: Vec::new(),
        }
    }
}

impl Default for StackStatus {
    fn default() -> Self {
        Self::accepted()
    }
}

pub type Stack = Resource<StackSpec, StackStatus>;
